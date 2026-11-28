//! The Pose Gate's `protein` parameter (product-owner decision, 2026-11-28): both
//! settings are honoured, and they give different clash counts on 4WKQ.
//!
//! The fixture is the one `geom` cross-validates against Python PoseBusters
//! (`fixtures/posebusters/`): 4WKQ's receptor, whose Cys797 is deposited as the
//! modified residue `CSX` on HETATM records, and the `clash` pose of gefitinib (the
//! whole ligand translated 4 Å towards the protein centroid).
//!
//! `crates/geom/tests/posebusters.rs` records the two numbers from the other side:
//! "With CSX included the 4WKQ clash pose has 65 clashes and 0.367 overlap instead of
//! 57 and 0.315."

mod common;

use std::sync::Arc;

use mold_graph::{Cache, Engine, ItemState, MemoryCache, NodeId, ParamValue, PortType, Registry};
use mold_mol::Arena;
use mold_nodes::table::{self, PoseRow};
use mold_nodes::{PoseSet, kinds};

/// A one-pose `PoseSet`: 4WKQ's receptor and one pose of gefitinib.
fn clash_pose_set(pose_file: &str) -> PoseSet {
    let receptor = common::receptor("posebusters/inputs/4WKQ_protein.pdb");
    let ligand = common::ligand(pose_file);
    let rows = vec![PoseRow {
        pose_id: 0,
        ligand_id: "IRE".into(),
        model_rank: 0,
        ..PoseRow::default()
    }];
    PoseSet {
        receptor: Arc::new(receptor),
        poses: Arc::new(ligand),
        table: Arc::new(table::to_frame(&rows)),
    }
}

/// Cook `test_poses -> pose_gate` with `params` set and return the gate's rows.
fn gate(set: PoseSet, params: &[(&str, ParamValue)]) -> Vec<PoseRow> {
    let reg: Arc<Registry> = common::registry_with_poses(set);
    let mut ed = mold_graph::Editor::new(reg.clone());
    ed.add_node_with_id(common::POSES_KIND, common::POSES_KIND)
        .unwrap();
    ed.add_node_with_id(kinds::POSE_GATE, kinds::POSE_GATE)
        .unwrap();
    ed.connect(common::poses_out(), &NodeId::new(kinds::POSE_GATE), "poses")
        .unwrap();
    for (name, value) in params {
        ed.set_param(&NodeId::new(kinds::POSE_GATE), name, value.clone())
            .unwrap();
    }
    let graph = ed.graph().clone();
    let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
    let items = Engine::new(reg, cache).cook(&graph, &[]).unwrap();
    let gate = NodeId::new(kinds::POSE_GATE);
    let it = items.items(&gate)[0];
    assert_eq!(it.state, ItemState::Cooked, "{:?}", it.error);
    let value = it.result.as_ref().unwrap().get("table").unwrap();
    assert_eq!(value.ty(), PortType::Table);
    table::from_frame(value.downcast_ref().unwrap()).unwrap()
}

#[test]
fn both_protein_rules_are_honoured_and_count_4wkq_differently() {
    let set = clash_pose_set("posebusters/inputs/4WKQ_IRE_clash.sdf");

    // `detect_covalent` off, so `clash_count` is the raw count PoseBusters reports
    // (gefitinib is not covalent; on a pose pushed 4 A into the protein the detector
    // does find a plausible C-S pair and excludes its neighbourhood, which is the
    // gate's own behaviour, not PoseBusters').
    let with = |rule: &str| {
        vec![
            ("protein", ParamValue::from(rule)),
            ("detect_covalent", ParamValue::Bool(false)),
        ]
    };

    // The default: every residue of a polymer chain, the modified Cys797 (CSX)
    // included, because physically it is protein.
    let polymer = gate(set.clone(), &with("polymer"));
    assert_eq!(polymer[0].clash_count, Some(65));
    assert_eq!(polymer[0].gate_pass, Some(false));

    // PoseBusters' split: ATOM records only, so CSX's seven atoms are a cofactor the
    // protein checks never see.
    let posebusters = gate(set, &with("posebusters"));
    assert_eq!(posebusters[0].clash_count, Some(57));
    assert_eq!(posebusters[0].gate_pass, Some(false));
}

#[test]
fn the_crystal_pose_passes_under_both_rules() {
    let set = clash_pose_set("posebusters/inputs/4WKQ_IRE_crystal.sdf");
    for rule in ["polymer", "posebusters"] {
        let rows = gate(set.clone(), &[("protein", ParamValue::from(rule))]);
        assert_eq!(rows[0].clash_count, Some(0), "{rule}");
        assert_eq!(rows[0].gate_pass, Some(true), "{rule}");
    }
}

#[test]
fn the_rule_is_part_of_the_cache_key_so_changing_it_re_gates() {
    let set = clash_pose_set("posebusters/inputs/4WKQ_IRE_clash.sdf");
    let reg = common::registry_with_poses(set);
    let mut ed = mold_graph::Editor::new(reg.clone());
    ed.add_node_with_id(common::POSES_KIND, common::POSES_KIND)
        .unwrap();
    ed.add_node_with_id("gate", kinds::POSE_GATE).unwrap();
    ed.connect(common::poses_out(), &NodeId::new("gate"), "poses")
        .unwrap();
    let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
    let engine = Engine::new(reg, cache);
    let gate = NodeId::new("gate");

    let first = engine.cook(ed.graph(), &[]).unwrap();
    let before = first.items(&gate)[0].key;
    assert_eq!(first.count(&gate, ItemState::Cooked), 1);

    // The same graph again: a cache hit, nothing re-gated.
    let again = engine.cook(ed.graph(), &[]).unwrap();
    assert_eq!(again.count(&gate, ItemState::Cached), 1);

    ed.set_param(&gate, "protein", "posebusters").unwrap();
    let switched = engine.cook(ed.graph(), &[]).unwrap();
    assert_ne!(switched.items(&gate)[0].key, before, "a new cache key");
    assert_eq!(switched.count(&gate, ItemState::Cooked), 1);
}

#[test]
fn a_pose_set_without_a_receptor_is_refused_with_a_reason() {
    let mut set = clash_pose_set("posebusters/inputs/4WKQ_IRE_crystal.sdf");
    set.receptor = Arc::new(Arena::new());
    let reg = common::registry_with_poses(set);
    let mut ed = mold_graph::Editor::new(reg.clone());
    ed.add_node_with_id(common::POSES_KIND, common::POSES_KIND)
        .unwrap();
    ed.add_node_with_id("gate", kinds::POSE_GATE).unwrap();
    ed.connect(common::poses_out(), &NodeId::new("gate"), "poses")
        .unwrap();
    let items = Engine::new(reg, Arc::new(MemoryCache::new()))
        .cook(ed.graph(), &[])
        .unwrap();
    let it = items.items(&NodeId::new("gate"))[0];
    assert_eq!(it.state, ItemState::Failed);
    let e = it.error.as_ref().unwrap().to_string();
    assert!(e.contains("no receptor"), "{e}");
}

#[test]
fn the_profiler_finds_the_hinge_hydrogen_bond_of_the_crystal_pose() {
    // The literature contact `geom` pins on this fixture: quinazoline N1 accepts from
    // Met793's backbone NH. Under either rule it is a polymer backbone donor.
    let set = clash_pose_set("posebusters/inputs/4WKQ_IRE_crystal.sdf");
    let reg = common::registry_with_poses(set);
    let mut ed = mold_graph::Editor::new(reg.clone());
    ed.add_node_with_id(common::POSES_KIND, common::POSES_KIND)
        .unwrap();
    ed.add_node_with_id("profile", kinds::INTERACTION_PROFILER)
        .unwrap();
    ed.connect(common::poses_out(), &NodeId::new("profile"), "poses")
        .unwrap();
    let items = Engine::new(reg, Arc::new(MemoryCache::new()))
        .cook(ed.graph(), &[])
        .unwrap();
    let it = items.items(&NodeId::new("profile"))[0];
    assert_eq!(it.state, ItemState::Cooked, "{:?}", it.error);
    let out = it.result.as_ref().unwrap();
    let rows = table::from_frame(out.get("table").unwrap().downcast_ref().unwrap()).unwrap();
    assert_eq!(rows[0].n_hbonds, Some(1), "the hinge H-bond");
    assert_eq!(
        rows[0].n_saltbridge,
        Some(1),
        "gefitinib's morpholine–Asp800"
    );
    assert_eq!(
        out.attrs.get("n_hbonds"),
        Some(&mold_graph::AttrValue::Int(1))
    );
    // `pose_id` stays the join key; the profiler only adds columns.
    assert_eq!(rows[0].pose_id, 0);
    assert_eq!(rows[0].ligand_id, "IRE");
}
