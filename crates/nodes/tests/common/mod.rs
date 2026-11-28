//! Test-only helpers: the repository's fixtures, the worker search path, temporary
//! directories, and a source node that hands a prepared `PoseSet` to a graph.
//! Not public API.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mold_graph::{
    CookContext, NodeError, NodeSpec, OutputRef, Outputs, PortType, Processor, Registry, Value,
};
use mold_jobs::Workers;
use mold_nodes::PoseSet;

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn fixture(relative: &str) -> PathBuf {
    repo().join("fixtures").join(relative)
}

/// The shipped worker manifests, with every ancestor of this test binary on the
/// search path — `target/<profile>/` is one of them, wherever cargo puts it, so the
/// freshly built `mock-cofold` is found.
///
/// `CARGO_BIN_EXE_mock-cofold` is only visible to that package's own tests, hence the
/// search. `cargo test --workspace` (or `cargo build -p mold-mock-cofold`) builds it;
/// without it the worker is missing and the end-to-end test says so.
pub fn workers() -> Arc<Workers> {
    let exe = std::env::current_exe().expect("the test binary's path");
    let path: Vec<PathBuf> = exe.ancestors().map(Path::to_path_buf).collect();
    Arc::new(
        Workers::load_dir(&repo().join("workers"))
            .expect("the shipped manifests")
            .with_search_path(path),
    )
}

/// A unique directory under the system temp dir, removed on drop.
#[derive(Debug)]
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock")
            .as_nanos();
        let p =
            std::env::temp_dir().join(format!("mold-nodes-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).expect("a temp directory");
        Self(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // On Windows a just-emptied run directory can stay "in use" for a moment after
        // its processes are gone (antivirus, indexer); retry briefly.
        for _ in 0..40 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// How many worker runs happened under `run_root`: one `job.json` per attempt.
pub fn worker_runs(run_root: &Path) -> usize {
    fn walk(dir: &Path, found: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, found);
            } else if p.file_name().is_some_and(|n| n == mold_jobs::SPEC_FILE) {
                *found += 1;
            }
        }
    }
    let mut found = 0;
    walk(run_root, &mut found);
    found
}

/// A source node handing a prepared [`PoseSet`] to the node under test, so a gate or
/// a profiler can be cooked without running a worker.
#[derive(Debug)]
pub struct Poses(pub PoseSet);

pub const POSES_KIND: &str = "test_poses";

impl Processor for Poses {
    fn spec(&self) -> NodeSpec {
        NodeSpec::new(POSES_KIND).output("poses", PortType::PoseSet)
    }

    fn cook(&self, _: &CookContext<'_>) -> Result<Outputs, NodeError> {
        Ok(Outputs::new().with("poses", Value::new(PortType::PoseSet, self.0.clone())))
    }
}

/// The [`Poses`] source's only output, to connect from.
pub fn poses_out() -> OutputRef {
    OutputRef::new(POSES_KIND, "poses")
}

/// [`mold_nodes::registry`] plus the [`Poses`] source.
pub fn registry_with_poses(set: PoseSet) -> Arc<Registry> {
    let mut r = mold_nodes::registry();
    r.processor(Poses(set)).expect("a valid test spec");
    Arc::new(r)
}

/// A receptor fixture as a one-pose arena.
pub fn receptor(relative: &str) -> mold_mol::Arena {
    let mut arena = mold_mol::Arena::new();
    mold_mol::io::read_file(
        &mut arena,
        fixture(relative),
        &mold_mol::LoadOptions::default(),
    )
    .unwrap_or_else(|e| panic!("{relative}: {e}"));
    arena
}

/// A ligand fixture (SDF, with bonds) as a one-pose arena.
pub fn ligand(relative: &str) -> mold_mol::Arena {
    let mut arena = mold_mol::Arena::new();
    let read = mold_mol::sdf::read_sdf_file(&mut arena, fixture(relative))
        .unwrap_or_else(|e| panic!("{relative}: {e}"));
    assert!(read.errors.is_empty(), "{relative}: {:?}", read.errors);
    arena
}

/// Two poses of the gefitinib fixture, one of them passed by the gate, with no
/// receptor: enough for a node that only reads the pose table.
pub fn two_poses() -> PoseSet {
    let lig = ligand("egfr/reference_ligand.sdf");
    let mut poses = lig.clone();
    mold_nodes::append_pose(&mut poses, &lig, mold_mol::PoseId(0), Some("b"), None)
        .expect("the fixture's pose");
    let rows: Vec<mold_nodes::table::PoseRow> = (0..2)
        .map(|i| mold_nodes::table::PoseRow {
            pose_id: i,
            ligand_id: "IRE".into(),
            gate_pass: Some(i == 0),
            ..mold_nodes::table::PoseRow::default()
        })
        .collect();
    PoseSet {
        receptor: Arc::new(mold_mol::Arena::new()),
        poses: Arc::new(poses),
        table: Arc::new(mold_nodes::table::to_frame(&rows)),
    }
}
