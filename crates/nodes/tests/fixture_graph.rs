//! The fixture graph, cooked end to end with **no GPU and no Python**:
//!
//! ```text
//! load_structure (4WKQ receptor) ──┐
//!                                  ├─► cofold (mock-cofold, per ligand) ─► pose_gate
//! load_ligands (2 records) ────────┘
//! ```
//!
//! This is the cut-down version of `crates/nodes/CLAUDE.md`'s "full 11-node graph"
//! acceptance test: five of the eleven nodes exist, so what is proved here is the
//! spine — load, fan out per ligand, run a worker through `mold-jobs`' local
//! executor, collect streamed poses, gate them — plus the cache behaviour around it.
//!
//! The worker is `mock-cofold`, built by `cargo test --workspace`; see
//! [`common::workers`].

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mold_graph::{
    Cache, CodecRegistry, Editor, Engine, ItemState, MemoryCache, NodeId, OutputRef, PortType,
    Registry, RunDirCache,
};
use mold_jobs::{Executors, Profiles};
use mold_nodes::table::{self, PoseRow};
use mold_nodes::{PoseSet, kinds};

const SAMPLES: i64 = 3;
const SEED: i64 = 7;

/// Set to the test's root directory when this binary re-invokes itself as the first
/// process of the restart test.
const RESTART_CHILD: &str = "MOLD_NODES_RESTART_CHILD";

// ---- The graph ----------------------------------------------------------------------

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

/// Two ligand records, written into `root`: the gefitinib fixture and a copy under
/// another title. `mol` reads SDF, not SMILES (`ligands.smi` needs `chem`), and the
/// committed SDF holds one record — two makes the per-ligand fan-out visible.
fn ligands_file(root: &Path) -> PathBuf {
    let one = std::fs::read_to_string(common::fixture("egfr/reference_ligand.sdf")).unwrap();
    assert!(one.trim_end().ends_with("$$$$"), "a complete SDF record");
    let path = root.join("ligands.sdf");
    // The title is the only place the fixture spells `4WKQ_IRE`; its `comp_id` data
    // item is `IRE` alone, so renaming the title renames just the second record.
    std::fs::write(
        &path,
        format!("{one}{}", one.replacen("4WKQ_IRE", "IRE-COPY", 1)),
    )
    .unwrap();
    path
}

/// `load_structure + load_ligands -> cofold -> pose_gate`, on executor `local`.
fn fixture_graph(reg: &Arc<Registry>, root: &Path) -> mold_graph::Graph {
    let mut ed = Editor::new(reg.clone());
    ed.add_node_with_id("receptor", kinds::LOAD_STRUCTURE)
        .unwrap();
    ed.add_node_with_id("ligands", kinds::LOAD_LIGANDS).unwrap();
    ed.add_node_with_id("cofold", kinds::COFOLD).unwrap();
    ed.add_node_with_id("gate", kinds::POSE_GATE).unwrap();

    let receptor = common::fixture("egfr/receptor.cif");
    ed.set_param(&id("receptor"), "path", receptor.to_str().unwrap())
        .unwrap();
    ed.set_param(&id("ligands"), "path", ligands_file(root).to_str().unwrap())
        .unwrap();
    ed.set_param(&id("cofold"), "samples", SAMPLES).unwrap();
    ed.set_param(&id("cofold"), "seed", SEED).unwrap();

    ed.connect(
        OutputRef::new("ligands", "ligands"),
        &id("cofold"),
        "ligands",
    )
    .unwrap();
    ed.connect(
        OutputRef::new("receptor", "structure"),
        &id("cofold"),
        "receptor",
    )
    .unwrap();
    ed.connect(OutputRef::new("cofold", "poses"), &id("gate"), "poses")
        .unwrap();

    let mut graph = ed.into_graph();
    // Everything on the subprocess executor: the worker node as a command job, the
    // Rust-native nodes cooked in place (ADR-0009 §2).
    graph.executor = Some(mold_jobs::LOCAL.to_string());
    // A broken worker must fail the test, never hang it.
    graph.nodes.get_mut(&id("cofold")).unwrap().timeout = Some(120.0);
    graph
}

/// Registry, executors and engine over `root`, with `cache`.
fn engine(
    reg: &Arc<Registry>,
    root: &Path,
    cache: Arc<dyn Cache>,
    g: &mold_graph::Graph,
) -> Engine {
    let executors = Executors::new(Profiles::builtin(), common::workers(), root.join("runs"));
    executors
        .attach(Engine::new(reg.clone(), cache), g)
        .expect("the built-in `local` executor")
}

fn registry() -> Arc<Registry> {
    Arc::new(mold_nodes::registry_with_workers(common::workers()))
}

/// The gate's rows for every item, in item order.
fn gate_rows(items: &mold_graph::ItemGraph) -> Vec<Vec<PoseRow>> {
    items
        .items(&id("gate"))
        .iter()
        .map(|it| {
            let out = it
                .result
                .as_ref()
                .unwrap_or_else(|| panic!("{} is {:?}: {:?}", it.id, it.state, it.error));
            table::from_frame(out.get("table").unwrap().downcast_ref().unwrap()).unwrap()
        })
        .collect()
}

// ---- The headline test --------------------------------------------------------------

#[test]
fn the_fixture_graph_cooks_end_to_end_on_the_mock_worker() {
    let tmp = common::TempDir::new("e2e");
    let reg = registry();
    let g = fixture_graph(&reg, tmp.path());
    let cache = Arc::new(MemoryCache::new());
    let items = engine(&reg, tmp.path(), cache.clone(), &g)
        .cook(&g, &[])
        .unwrap();

    // One receptor, one item per ligand record, and one co-fold run per ligand.
    assert_eq!(items.count(&id("receptor"), ItemState::Cooked), 1);
    assert_eq!(items.count(&id("ligands"), ItemState::Cooked), 2);
    assert_eq!(
        items.count(&id("cofold"), ItemState::Cooked),
        2,
        "{items:#?}"
    );
    assert_eq!(items.count(&id("gate"), ItemState::Cooked), 2);
    assert_eq!(common::worker_runs(&tmp.path().join("runs")), 2);

    // The receptor: 4WKQ chain A, with the modified Cys797 as an `X`.
    let structure = items.items(&id("receptor"))[0].result.as_ref().unwrap();
    let arena: &mold_mol::Arena = structure.get("structure").unwrap().downcast_ref().unwrap();
    assert_eq!(arena.pose_count(), 1);
    assert_eq!(
        arena.atom_count(),
        2317,
        "receptor.cif's 2338 atom sites, one conformer per altloc residue"
    );
    let sequence: &String = structure.get("sequence").unwrap().downcast_ref().unwrap();
    // Every residue group the file models, one chain, no gap filling. (297, not the
    // 296 `fixtures/egfr/README.md` quotes: the file has 297 residue groups.)
    assert_eq!(sequence.len(), arena.residues().len());
    assert_eq!(sequence.len(), 297);
    assert_eq!(sequence.matches('X').count(), 1, "CSX 797");

    // The ligands: one per record, each with its id and its bonds.
    let ids: BTreeSet<String> = items
        .items(&id("ligands"))
        .iter()
        .map(|it| {
            let set: &mold_nodes::LigandSet = it
                .result
                .as_ref()
                .unwrap()
                .get("ligands")
                .unwrap()
                .downcast_ref()
                .unwrap();
            let l = set.single().expect("one ligand per item");
            assert_eq!(set.arena.atom_count(), 31, "gefitinib");
            assert_eq!(set.arena.pose_bonds(l.pose).unwrap().len(), 34);
            assert_eq!(l.property("comp_id"), Some("IRE"));
            l.id.clone()
        })
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from(["4WKQ_IRE".to_string(), "IRE-COPY".to_string()])
    );

    // The poses: `samples` per ligand, the receptor carried along, scores in the
    // table, and the worker's own files recorded as artifacts.
    for it in items.items(&id("cofold")) {
        let out = it.result.as_ref().unwrap();
        let set: &PoseSet = out.get("poses").unwrap().downcast_ref().unwrap();
        assert_eq!(set.len(), SAMPLES as usize);
        assert_eq!(set.receptor.atom_count(), 2317, "the shared receptor");
        assert_eq!(out.files.len(), SAMPLES as usize);
        assert!(out.files.iter().all(|f| f.is_file()), "{:?}", out.files);
        let rows = table::from_frame(&set.table).unwrap();
        assert_eq!(rows.len(), SAMPLES as usize);
        for (rank, row) in rows.iter().enumerate() {
            assert_eq!(row.pose_id, rank as u32);
            assert_eq!(row.model_rank, rank as u32);
            assert!(!row.ligand_id.is_empty());
            // The mock's seeded metrics, under the contract column names.
            assert!(row.plddt.is_some_and(|x| (0.60..0.95).contains(&x)));
            assert!(row.iptm.is_some_and(|x| (0.40..0.90).contains(&x)));
            assert!(row.affinity_pred.is_some_and(|x| (5.0..9.0).contains(&x)));
            assert!(row.binder_prob.is_some_and(|x| (0.0..1.0).contains(&x)));
            // Not measured yet: the gate's and the profiler's columns.
            assert_eq!(row.clash_count, None);
            assert_eq!(row.gate_pass, None);
            assert_eq!(row.n_hbonds, None);
        }
        // Each pose keeps the ligand's bonds, which is what the gate needs.
        for pose in set.poses.pose_ids() {
            assert_eq!(set.poses.pose_bonds(pose).unwrap().len(), 34);
        }
    }

    // The gate: every pose judged, the co-fold scores still beside the verdict.
    //
    // Every mock pose fails on **ligand** geometry, and that is the right answer: the
    // mock jitters each coordinate independently by up to 0.3 Å, which stretches bonds
    // well past the ±25% the gate allows. The protein checks are happy (`clash_count`
    // 0: the pose is still in the pocket), which is what this graph is here to show.
    // `the_gate_parameters_reach_geom` below loosens the ligand thresholds and the
    // same poses pass.
    let rows = gate_rows(&items);
    assert_eq!(rows.len(), 2);
    for item in &rows {
        assert_eq!(item.len(), SAMPLES as usize);
        for row in item {
            assert!(row.plddt.is_some(), "the co-fold scores survive the gate");
            assert_eq!(row.clash_count, Some(0), "no protein clash: {row:?}");
            assert_eq!(row.gate_pass, Some(false), "jittered bonds: {row:?}");
        }
    }

    // The gate's `PoseSet` output is the same poses with the judged table.
    let gated = items.items(&id("gate"))[0].result.as_ref().unwrap();
    let set: &PoseSet = gated.get("poses").unwrap().downcast_ref().unwrap();
    assert_eq!(set.len(), SAMPLES as usize, "no pose is dropped");
    assert_eq!(
        gated.attrs.get("gate_pass"),
        Some(&mold_graph::AttrValue::Int(0))
    );
    assert_eq!(gated.get("table").unwrap().ty(), PortType::Table);
}

/// Every `GateConfig` field is a parameter, and the parameters reach `geom`: with the
/// ligand thresholds opened up far enough for the mock's jitter, the same poses pass.
#[test]
fn the_gate_parameters_reach_geom() {
    let tmp = common::TempDir::new("params");
    let reg = registry();
    let mut g = fixture_graph(&reg, tmp.path());
    let gate = g.nodes.get_mut(&id("gate")).unwrap();
    for (name, value) in [
        ("bond_length_tolerance", 1.0),
        ("bond_angle_tolerance", 1.0),
        ("aromatic_flatness_threshold", 1.5),
        ("internal_clash_ratio", 0.1),
    ] {
        gate.params.set(name, value);
    }
    let items = engine(&reg, tmp.path(), Arc::new(MemoryCache::new()), &g)
        .cook(&g, &[])
        .unwrap();
    assert_eq!(items.count(&id("gate"), ItemState::Cooked), 2);
    for item in gate_rows(&items) {
        assert!(item.iter().all(|r| r.gate_pass == Some(true)), "{item:#?}");
    }
}

// ---- Dirtying -----------------------------------------------------------------------

#[test]
fn changing_a_gate_threshold_re_runs_the_gate_only() {
    let tmp = common::TempDir::new("dirty");
    let reg = registry();
    let g = fixture_graph(&reg, tmp.path());
    let runs = tmp.path().join("runs");
    let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());

    let first = engine(&reg, tmp.path(), cache.clone(), &g)
        .cook(&g, &[])
        .unwrap();
    assert_eq!(first.count(&id("gate"), ItemState::Cooked), 2);
    assert_eq!(common::worker_runs(&runs), 2, "one worker run per ligand");
    for item in gate_rows(&first) {
        assert!(item.iter().all(|r| r.clash_count == Some(0)));
    }

    // A clash cutoff that calls every close contact a clash: only the gate's items
    // change key.
    let mut tightened = g.clone();
    tightened
        .nodes
        .get_mut(&id("gate"))
        .unwrap()
        .params
        .set("protein_clash_cutoff", 1.5);
    let again = engine(&reg, tmp.path(), cache.clone(), &tightened)
        .cook(&tightened, &[])
        .unwrap();

    for node in ["receptor", "ligands", "cofold"] {
        assert_eq!(
            again.count(&id(node), ItemState::Cached),
            first.count(&id(node), ItemState::Cooked),
            "{node} was not re-cooked"
        );
    }
    assert_eq!(again.count(&id("gate"), ItemState::Cooked), 2, "re-gated");
    assert_eq!(
        common::worker_runs(&runs),
        2,
        "the worker did not run again: a gate threshold is not in co-fold's key"
    );
    // The new threshold really was used.
    for item in gate_rows(&again) {
        assert!(
            item.iter().all(|r| r.gate_pass == Some(false)),
            "everything clashes at 1.5 x the vdW sum"
        );
    }

    // Back to the original value: the first results are still cached.
    let back = engine(&reg, tmp.path(), cache, &g).cook(&g, &[]).unwrap();
    assert_eq!(back.count(&id("gate"), ItemState::Cached), 2);
    assert_eq!(common::worker_runs(&runs), 2);
}

// ---- Restart ------------------------------------------------------------------------

/// Everything a cold process needs over one root directory.
fn cold(root: &Path) -> (Arc<Registry>, mold_graph::Graph, Arc<RunDirCache>, Engine) {
    let reg = registry();
    let g = fixture_graph(&reg, root);
    let codecs: Arc<CodecRegistry> = Arc::new(mold_nodes::codecs());
    let cache = Arc::new(RunDirCache::open(root.join("cache"), codecs).unwrap());
    let engine = engine(&reg, root, cache.clone(), &g);
    (reg, g, cache, engine)
}

#[test]
fn a_second_process_finds_the_whole_graph_in_the_run_directory() {
    // Child mode: be the first process, cook everything into the run directory.
    if let Some(root) = std::env::var_os(RESTART_CHILD) {
        let root = PathBuf::from(root);
        let (_, g, cache, engine) = cold(&root);
        let items = engine.cook(&g, &[]).unwrap();
        assert!(
            items.all().all(|i| i.state == ItemState::Cooked),
            "{items:#?}"
        );
        assert_eq!(
            cache.take_events(),
            [],
            "every payload has a codec, so every item persisted"
        );
        return;
    }

    let tmp = common::TempDir::new("restart");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "a_second_process_finds_the_whole_graph_in_the_run_directory",
            "--exact",
            "--test-threads=1",
        ])
        .env(RESTART_CHILD, tmp.path())
        .status()
        .unwrap();
    assert!(status.success(), "the first process failed: {status}");
    let runs = tmp.path().join("runs");
    assert_eq!(
        common::worker_runs(&runs),
        2,
        "the first process ran the worker"
    );

    // This process starts cold: new registry, engine and cache over the directory.
    let (_, g, cache, engine) = cold(tmp.path());
    let generated = engine.generate(&g, &[]).unwrap();
    for it in generated
        .all()
        .filter(|i| i.role == mold_graph::ItemRole::Work)
    {
        assert_eq!(it.state, ItemState::Cached, "{} is a cache hit", it.id);
    }
    let items = engine.cook(&g, &[]).unwrap();
    for node in ["receptor", "ligands", "cofold", "gate"] {
        assert_eq!(
            items.count(&id(node), ItemState::Cooked),
            0,
            "{node} cooked again after a restart"
        );
    }
    assert_eq!(items.count(&id("gate"), ItemState::Cached), 2);
    assert_eq!(
        common::worker_runs(&runs),
        2,
        "no worker ran in the second process"
    );
    assert_eq!(cache.take_events(), []);

    // The decoded results are the cooked ones: poses, receptor and judged table.
    let rows = gate_rows(&items);
    assert_eq!(rows.len(), 2);
    for item in &rows {
        assert_eq!(item.len(), SAMPLES as usize);
        // Decoded, not re-cooked: the verdict, the clash count and the worker's
        // scores all came back out of the entry on disk.
        assert!(item.iter().all(|r| r.gate_pass == Some(false)));
        assert!(item.iter().all(|r| r.clash_count == Some(0)));
        assert!(item.iter().all(|r| r.plddt.is_some()));
    }
    let set: &PoseSet = items.items(&id("gate"))[0]
        .result
        .as_ref()
        .unwrap()
        .get("poses")
        .unwrap()
        .downcast_ref()
        .unwrap();
    assert_eq!(set.receptor.atom_count(), 2317, "the receptor came back");
    assert_eq!(set.poses.pose_bonds(mold_mol::PoseId(0)).unwrap().len(), 34);
}

// ---- Recipe round-trip --------------------------------------------------------------

#[test]
fn the_fixture_graph_round_trips_through_a_recipe() {
    let tmp = common::TempDir::new("recipe");
    let reg = registry();
    let g = fixture_graph(&reg, tmp.path());

    let text = mold_recipe::Recipe::new(g.clone()).to_toml();
    let back = mold_recipe::from_toml_with(&text, &reg).expect("a recipe this registry validates");
    assert_eq!(back.graph, g, "the graph survives the round trip");
    assert_eq!(mold_recipe::Recipe::new(back.graph).to_toml(), text);

    // The text is what a reviewer reads: kinds, params and wiring, nothing machine-specific
    // beyond the paths the user typed.
    for want in [
        "kind = \"load_structure\"",
        "kind = \"load_ligands\"",
        "kind = \"cofold\"",
        "kind = \"pose_gate\"",
        "executor = \"local\"",
        "samples = 3",
        "worker = \"mock-cofold\"",
        "protein = \"polymer\"",
        "inputs = { ligands = \"ligands.ligands\", receptor = \"receptor.structure\" }",
    ] {
        assert!(
            text.contains(want),
            "the recipe should contain {want:?}:\n{text}"
        );
    }
}
