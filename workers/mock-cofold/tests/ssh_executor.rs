//! The SSH executor (crates/jobs/src/ssh.rs) against a **loopback double**: the
//! `mock-ssh` and `mock-scp` binaries stand in for the clients, so the whole path —
//! staging out, the remote wrapper script, stdio over the "connection", the remote
//! process-group kill, staging back, cleanup and every failure mode — runs with no
//! network, no `sshd` and no second machine. These tests always run in CI.
//!
//! **What the double does and does not prove.** It is only the transport: the
//! "remote" command is executed by a real POSIX shell on this machine, in a real
//! directory named by the profile's `run_root`, so job control, the pgid file, the
//! cancel sentinel and `kill -9 -<pgid>` are the real ones. On Unix the double also
//! puts that shell in **its own process group**, so killing the client (which the
//! executor's local process-tree kill does) cannot end the "remote" side: only the
//! second `ssh` can, which is the point of the design. On Windows the local kill is a
//! Job Object, which reaches the double's children too, so there the cancel test
//! proves that the remote kill *ran and did its job*, not that it was the only thing
//! that could have. What no double can prove — that a real `sshd` accepts this
//! command line, and that a real remote shell behaves as scripted — needs a real
//! host; see crates/jobs/CLAUDE.md.
//!
//! The graph, the worker node and the fixtures are [`common`], shared with
//! `local_executor.rs`: both flavours are measured the same way.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::*;
use mold_graph::*;
use mold_jobs::{Executors, Profiles, Workers, pid_alive};
use mold_mock_cofold::double::posix_shell;

// ---- The double as a profile --------------------------------------------------------

/// A path as a TOML string's content (`\` is an escape in TOML, and Windows takes
/// `/` everywhere).
fn toml_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `[executor.box]`: an ssh profile whose clients are the doubles. `host` is the
/// double's switch (`unreachable…`, `denied…`, `drop…`, anything else runs), and
/// `run_root` is a real directory standing in for the host's filesystem.
fn ssh_profile(host: &str, remote_root: &Path, slots: u32, extra: &str) -> Profiles {
    profiles(&format!(
        "[executor.box]\n\
         type = \"ssh\"\n\
         host = \"{host}\"\n\
         slots = {slots}\n\
         run_root = \"{root}\"\n\
         ssh_program = \"\\\"{ssh}\\\"\"\n\
         scp_program = \"\\\"{scp}\\\"\"\n\
         connect_timeout_s = 5\n\
         {extra}",
        root = toml_path(remote_root),
        ssh = toml_path(Path::new(env!("CARGO_BIN_EXE_mock-ssh"))),
        scp = toml_path(Path::new(env!("CARGO_BIN_EXE_mock-scp"))),
    ))
}

/// The host's copy of a local run directory: the same `<item key>/<attempt>` under
/// the profile's `run_root`.
fn remote_dir(local: &Path, remote_root: &Path) -> PathBuf {
    let attempt = local.file_name().expect("a run directory has a name");
    let key = local
        .parent()
        .and_then(Path::file_name)
        .expect("a run directory sits under its item key");
    remote_root.join(key).join(attempt)
}

/// The executor's `<attempt>.pgid` / `<attempt>.cancel` beside a remote run
/// directory. Not `with_extension`: an attempt may be named `0.1`.
fn sibling(remote: &Path, ext: &str) -> PathBuf {
    let name = remote.file_name().expect("a run directory has a name");
    remote
        .parent()
        .expect("a run directory has a parent")
        .join(format!("{}.{ext}", name.to_string_lossy()))
}

/// Open the gate the worker waits at, in its **remote** run directory (that is the
/// worker's working directory).
fn open_remote_gate(shared: &Shared, item: &str, remote_root: &Path) {
    for d in shared.dirs(item) {
        let dir = remote_dir(&d, remote_root);
        if dir.is_dir() {
            std::fs::write(dir.join("go"), b"").unwrap();
        }
    }
}

/// `false` (with a note) when this machine has no POSIX shell for the double to run
/// the remote command with. CI's required job is Linux, so these never skip there.
fn have_shell(test: &str) -> bool {
    if posix_shell().is_some() {
        return true;
    }
    if cfg!(unix) {
        panic!("{test}: every Unix machine has /bin/sh; these tests must not skip here");
    }
    eprintln!("{test}: skipped, no POSIX shell for the ssh double (set MOLD_TEST_SH)");
    false
}

// ---- The acceptance test: a staging round trip ---------------------------------------

#[test]
fn ssh_stages_inputs_out_runs_the_worker_and_stages_the_artifacts_back() {
    if !have_shell("ssh staging round trip") {
        return;
    }
    let tmp = TempDir::new("ssh-round-trip");
    let remote_root = tmp.0.join("host");
    std::fs::create_dir_all(&remote_root).unwrap();
    let shared = Arc::new(Shared::default());
    let reg = registry(2, &shared);
    let ex = Executors::new(
        ssh_profile("gpu-box", &remote_root, 4, ""),
        workers(),
        &tmp.0,
    );
    let g = graph(
        &reg,
        Some("box"),
        &[("samples", 3i64.into()), ("gate", "go".into())],
    );
    let cache = Arc::new(MemoryCache::new());
    let engine = engine(&ex, &reg, cache.clone(), &g);

    // Each worker holds after its first pose until the observer has seen that pose as
    // a partial output of a still-running item: events really do stream over the
    // connection. While it is held, the test looks at the host's side of the run.
    let log: Timeline = Arc::default();
    let staged: Arc<Mutex<Vec<(String, bool, bool)>>> = Arc::default();
    let (l, s, st, root) = (
        log.clone(),
        shared.clone(),
        staged.clone(),
        remote_root.clone(),
    );
    let observer: Observer = Box::new(move |it: &WorkItem| {
        let n = it.partial.as_ref().map_or(0, |p| poses_of(p).len());
        let name = it.id.to_string();
        if name.starts_with("cofold") && n == 1 && it.state == ItemState::Cooking {
            for d in s.dirs(&name) {
                let remote = remote_dir(&d, &root);
                st.lock().unwrap().push((
                    name.clone(),
                    remote.join("ligands.sdf").is_file(),
                    sibling(&remote, "pgid").is_file(),
                ));
            }
            open_remote_gate(&s, &name, &root);
        }
        l.lock().unwrap().push((name, it.state, n));
    });
    let items = engine
        .cook_with(
            &g,
            &[],
            CookOptions {
                observer: Some(observer),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        items.count(&id("cofold"), ItemState::Cooked),
        2,
        "{items:#?}"
    );

    // Inputs went out, and the wrapper recorded the worker's process group, before
    // the first pose came back.
    let staged = staged.lock().unwrap().clone();
    assert_eq!(staged.len(), 2, "{staged:?}");
    for (item, input, pgid) in &staged {
        assert!(input, "{item}: `ligands.sdf` was staged to the host");
        assert!(pgid, "{item}: the remote process group was recorded");
    }

    let log = cofold_only(&log.lock().unwrap());
    for it in items.items(&id("cofold")) {
        let name = it.id.to_string();
        let mine: Vec<_> = log.iter().filter(|(n, ..)| *n == name).collect();
        let counts: Vec<usize> = mine
            .iter()
            .filter(|(_, s, n)| *s == ItemState::Cooking && *n > 0)
            .map(|(_, _, n)| *n)
            .collect();
        assert_eq!(counts, [1, 2, 3], "{name}: poses stream one by one");

        // The artifacts are here, in the *local* run directory: the node's
        // `collect_finish` already refused to finish without them.
        let dir = &shared.dirs(&name)[0];
        assert_eq!(dir, &tmp.0.join("box").join(it.key.to_hex()).join("0"));
        let poses = poses_of(it.result.as_ref().unwrap());
        assert_eq!(poses.len(), 3);
        for p in &poses {
            let cif = std::fs::read_to_string(dir.join(&p.path)).unwrap();
            assert!(cif.contains("_atom_site"), "{} is mmCIF", p.path);
        }
        // The run's own record stays local, as for any flavour, plus what the
        // clients printed.
        for f in [
            mold_jobs::SPEC_FILE,
            mold_jobs::EVENTS_FILE,
            mold_jobs::STDERR_FILE,
            mold_jobs::SSH_LOG_FILE,
        ] {
            assert!(dir.join(f).is_file(), "{f} in the run dir");
        }
        assert!(
            !dir.join(mold_jobs::SSH_KILL_FILE).exists(),
            "a clean run kills nothing remotely"
        );
        // A finished item's remote run directory is gone; the artifacts are here.
        let remote = remote_dir(dir, &remote_root);
        assert!(!remote.exists(), "{} was cleaned up", remote.display());
        assert!(!sibling(&remote, "pgid").exists());
        assert!(cache.get(&it.key).is_some());
    }
}

// ---- Parity with the local flavour --------------------------------------------------

#[test]
fn the_same_graph_cooked_locally_and_over_ssh_agrees_byte_for_byte() {
    if !have_shell("ssh parity") {
        return;
    }
    let params = [("samples", ParamValue::from(2i64)), ("seed", 5i64.into())];
    let mut runs = Vec::new();
    for flavour in ["local", "ssh"] {
        let tmp = TempDir::new(&format!("parity-{flavour}"));
        let remote_root = tmp.0.join("host");
        std::fs::create_dir_all(&remote_root).unwrap();
        let shared = Arc::new(Shared::default());
        let reg = registry(3, &shared);
        let (key, profile) = match flavour {
            "local" => ("mock", mock_profile(3, 0)),
            _ => (
                "box",
                ssh_profile("gpu-box", &remote_root, 3, "keep_run_dir = true\n"),
            ),
        };
        let g = graph(&reg, Some(key), &params);
        let ex = Executors::new(profile, workers(), &tmp.0);
        let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
            .cook(&g, &[])
            .unwrap();
        assert_eq!(
            items.count(&id("cofold"), ItemState::Cooked),
            3,
            "{flavour}: {items:#?}"
        );
        runs.push((keys(&items), results(&items), run_files(&shared)));
    }
    let (local, ssh) = (&runs[0], &runs[1]);
    assert_eq!(local.0, ssh.0, "identical cache keys");
    assert_eq!(local.1, ssh.1, "identical outputs");
    assert_eq!(local.2.len(), 3);
    assert_eq!(
        local.2, ssh.2,
        "byte-identical JobSpec and event stream, local and over ssh"
    );
}

// ---- Cancellation: the remote process group ------------------------------------------

#[test]
fn cancel_kills_the_remote_process_group_including_a_grandchild() {
    if !have_shell("ssh cancel") {
        return;
    }
    let tmp = TempDir::new("ssh-cancel");
    let remote_root = tmp.0.join("host");
    std::fs::create_dir_all(&remote_root).unwrap();
    let shared = Arc::new(Shared::default());
    let reg = registry(2, &shared);
    let ex = Executors::new(
        // Keep the remote run directory: a cancelled attempt is for inspection.
        ssh_profile("gpu-box", &remote_root, 2, ""),
        workers(),
        &tmp.0,
    );
    // Each worker sends one pose, spawns a grandchild that sleeps for ever, records
    // both PIDs in its (remote) run directory, then hangs: nothing but a kill can end
    // it, and neither process ever writes again, so no broken pipe can do it for us.
    let g = graph(&reg, Some("box"), &[("fail", "orphan".into())]);
    let cache = Arc::new(MemoryCache::new());
    let engine = engine(&ex, &reg, cache.clone(), &g);

    let cancel = CancelToken::new();
    let (c, s, root) = (cancel.clone(), shared.clone(), remote_root.clone());
    let canceller = std::thread::spawn(move || {
        let up = eventually(PATIENCE, || {
            let dirs = s.all_dirs();
            dirs.len() == 2 && dirs.iter().all(|d| pids(&remote_dir(d, &root)).is_some())
        });
        c.cancel();
        up
    });
    let items = engine
        .cook_with(
            &g,
            &[],
            CookOptions {
                cancel,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        canceller.join().unwrap(),
        "both workers spawned their child on the host"
    );
    assert!(items.cancelled);

    let dirs = shared.all_dirs();
    let all: Vec<u32> = dirs
        .iter()
        .flat_map(|d| {
            let (w, c) = pids(&remote_dir(d, &remote_root)).unwrap();
            [w, c]
        })
        .collect();
    assert_eq!(all.len(), 4);
    assert!(
        eventually(PATIENCE, || all.iter().all(|&p| !pid_alive(p))),
        "the remote worker and its grandchild are gone: {:?}",
        all.iter().map(|&p| (p, pid_alive(p))).collect::<Vec<_>>()
    );
    for d in &dirs {
        // The kill ran over a second connection, and in the right order: the
        // sentinel first, so a worker that had not started yet cannot start.
        let kill = std::fs::read_to_string(d.join(mold_jobs::SSH_KILL_FILE)).unwrap();
        assert!(kill.contains("killed the remote process group"), "{kill}");
        let remote = remote_dir(d, &remote_root);
        assert!(
            sibling(&remote, "cancel").is_file(),
            "{}: the cancel sentinel is on the host",
            remote.display()
        );
        assert!(
            remote.is_dir(),
            "a cancelled attempt's remote directory is kept for inspection"
        );
    }
    for it in items.items(&id("cofold")) {
        assert_eq!(it.state, ItemState::Cancelled);
        assert!(cache.get(&it.key).is_none(), "{} was cached", it.id);
    }
    assert!(ex.executor("box", vec![]).unwrap().wait_idle(PATIENCE));
}

// ---- Failures ------------------------------------------------------------------------

/// Cook one cofold item on an ssh profile; return its state and its error message.
fn cook_one_remote(
    name: &str,
    host: &str,
    extra: &str,
    workers: Arc<Workers>,
    params: &[(&str, ParamValue)],
) -> (ItemState, String) {
    let tmp = TempDir::new(name);
    let remote_root = tmp.0.join("host");
    std::fs::create_dir_all(&remote_root).unwrap();
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(ssh_profile(host, &remote_root, 1, extra), workers, &tmp.0);
    let g = graph(&reg, Some("box"), params);
    let cache = Arc::new(MemoryCache::new());
    let items = engine(&ex, &reg, cache.clone(), &g).cook(&g, &[]).unwrap();
    let it = &items.items(&id("cofold"))[0];
    assert!(
        cache.get(&it.key).is_none(),
        "{name}: a failed item is never cached"
    );
    let e = it
        .error
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    (it.state, e)
}

#[test]
fn an_unreachable_host_and_a_refused_login_fail_before_the_worker_with_the_reason() {
    if !have_shell("ssh unreachable") {
        return;
    }
    for (host, want) in [
        ("unreachable-box", "Could not resolve hostname"),
        ("denied-box", "Permission denied"),
    ] {
        let (state, e) = cook_one_remote(
            &format!("ssh-{host}"),
            host,
            "",
            workers(),
            &[("samples", 1i64.into())],
        );
        assert_eq!(state, ItemState::Failed, "{host}: {e}");
        for w in [want, host, "remote run directory"] {
            assert!(e.contains(w), "{host}: {e:?} should mention {w:?}");
        }
    }
}

#[test]
fn a_worker_that_is_not_installed_on_the_host_says_so() {
    if !have_shell("ssh missing worker") {
        return;
    }
    // A manifest whose executable exists nowhere: the remote shell answers 127.
    let tmp = TempDir::new("ssh-ghost-workers");
    let dir = tmp.0.join("ghost");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(mold_jobs::MANIFEST_FILE),
        "name = \"ghost\"\nexecutable = \"mold-no-such-worker\"\n\
         environment = \"none\"\nneeds_gpu = false\nmodel = \"none\"\n",
    )
    .unwrap();
    let workers = Arc::new(Workers::load_dir(&tmp.0).unwrap());
    let (state, e) = cook_one_remote(
        "ssh-ghost",
        "gpu-box",
        "",
        workers,
        &[("worker", "ghost".into()), ("samples", 1i64.into())],
    );
    assert_eq!(state, ItemState::Failed, "{e}");
    for w in ["was not found on `gpu-box`", "remote PATH", "`ghost`"] {
        assert!(e.contains(w), "{e:?} should mention {w:?}");
    }
}

#[test]
fn a_connection_that_drops_mid_stream_fails_the_item() {
    if !have_shell("ssh dropped connection") {
        return;
    }
    // The double forwards two events and then dies like a dropped connection: the
    // item was already `Cooking`, and it must still fail, not hang or succeed.
    let (state, e) = cook_one_remote(
        "ssh-drop",
        "drop-box",
        "",
        workers(),
        &[("samples", 4i64.into()), ("delay_ms", 20i64.into())],
    );
    assert_eq!(state, ItemState::Failed, "{e}");
    for w in [
        "before `finished`",
        "could not run the worker on `drop-box`",
        "closed by remote host",
    ] {
        assert!(e.contains(w), "{e:?} should mention {w:?}");
    }
}

#[test]
fn a_failed_attempt_keeps_its_remote_run_directory_and_the_retry_gets_a_fresh_one() {
    if !have_shell("ssh retry") {
        return;
    }
    let tmp = TempDir::new("ssh-retry");
    let remote_root = tmp.0.join("host");
    std::fs::create_dir_all(&remote_root).unwrap();
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    let ex = Executors::new(
        ssh_profile("gpu-box", &remote_root, 1, ""),
        workers(),
        &tmp.0,
    );
    // The worker reports `failed` on its first attempt only.
    let mut g = graph(
        &reg,
        Some("box"),
        &[
            ("samples", 2i64.into()),
            ("fail", "error".into()),
            ("fail_attempts", 1i64.into()),
        ],
    );
    g.nodes.get_mut(&id("cofold")).unwrap().retries = 1;
    let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
        .cook(&g, &[])
        .unwrap();
    let it = &items.items(&id("cofold"))[0];
    assert_eq!(
        it.state,
        ItemState::Cooked,
        "{:?}",
        it.error.as_ref().map(ToString::to_string)
    );
    let dirs = shared.dirs(&it.id.to_string());
    assert_eq!(dirs.len(), 2, "attempt 0, then 1");
    let failed = remote_dir(&dirs[0], &remote_root);
    assert!(
        failed.join("ligands.sdf").is_file(),
        "a failed attempt's remote directory is kept, inputs and all: {}",
        failed.display()
    );
    assert!(
        sibling(&failed, "pgid").is_file(),
        "with what it was, for inspection"
    );
    let ok = remote_dir(&dirs[1], &remote_root);
    assert!(
        !ok.exists(),
        "the successful attempt's remote directory is gone: {}",
        ok.display()
    );
}

// ---- A shared filesystem skips staging ----------------------------------------------

#[test]
fn a_shared_filesystem_profile_runs_the_worker_in_the_run_directory_and_stages_nothing() {
    if !have_shell("ssh shared_fs") {
        return;
    }
    let tmp = TempDir::new("ssh-shared");
    let shared = Arc::new(Shared::default());
    let reg = registry(1, &shared);
    // `shared_fs` is the host's view of *this* executor's run root, so the remote run
    // directory is the local one. `scp_program` names a program that does not exist:
    // if anything tried to stage, the item would fail.
    let root = tmp.0.join("box");
    let mut p = ssh_profile("gpu-box", &tmp.0.join("unused"), 1, "");
    p.add_toml(
        &format!(
            "[executor.box]\ntype = \"ssh\"\nhost = \"gpu-box\"\nslots = 1\n\
             shared_fs = \"{}\"\n\
             ssh_program = \"\\\"{}\\\"\"\n\
             scp_program = \"mold-no-such-scp\"\nconnect_timeout_s = 5\n",
            toml_path(&root),
            toml_path(Path::new(env!("CARGO_BIN_EXE_mock-ssh"))),
        ),
        Path::new("shared.toml"),
    )
    .unwrap();
    let ex = Executors::new(p, workers(), &tmp.0);
    let g = graph(&reg, Some("box"), &[("samples", 2i64.into())]);
    let items = engine(&ex, &reg, Arc::new(MemoryCache::new()), &g)
        .cook(&g, &[])
        .unwrap();
    let it = &items.items(&id("cofold"))[0];
    assert_eq!(
        it.state,
        ItemState::Cooked,
        "{:?}",
        it.error.as_ref().map(ToString::to_string)
    );
    let dir = &shared.dirs(&it.id.to_string())[0];
    for p in poses_of(it.result.as_ref().unwrap()) {
        assert!(
            dir.join(&p.path).is_file(),
            "the worker wrote {} straight into the run directory",
            p.path
        );
    }
    assert!(
        !dir.join(mold_jobs::SSH_LOG_FILE).exists(),
        "no client ran: nothing was staged, nothing was cleaned up"
    );
    // The inputs are still there: a shared filesystem has nothing to trim.
    assert!(dir.join("ligands.sdf").is_file());
}
