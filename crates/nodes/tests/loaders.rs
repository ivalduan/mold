//! Load Structure and Load Ligands: the parameters that change what is read, and the
//! failures a user can cause, each with a message that says what to do.

mod common;

use std::path::Path;
use std::sync::Arc;

use mold_graph::{Editor, Engine, ItemState, MemoryCache, NodeId, ParamValue, Registry};
use mold_nodes::{LigandSet, kinds};

/// Cook one source node of `kind` with `params`, and return its items.
fn cook(kind: &str, params: &[(&str, ParamValue)]) -> mold_graph::ItemGraph {
    let reg: Arc<Registry> = Arc::new(mold_nodes::registry());
    let mut ed = Editor::new(reg.clone());
    ed.add_node_with_id("n", kind).unwrap();
    for (name, value) in params {
        ed.set_param(&NodeId::new("n"), name, value.clone())
            .unwrap();
    }
    Engine::new(reg, Arc::new(MemoryCache::new()))
        .cook(ed.graph(), &[])
        .unwrap()
}

/// The one error of a graph whose only node failed — at generation or at cook.
fn error(items: &mold_graph::ItemGraph) -> String {
    if let Some((_, e)) = items.generation_errors.first() {
        return e.to_string();
    }
    let it = items.items(&NodeId::new("n"));
    let it = it.first().expect("an item or a generation error");
    assert_eq!(it.state, ItemState::Failed, "{:?}", it.result);
    it.error.as_ref().expect("an error").to_string()
}

fn path(p: &Path) -> ParamValue {
    ParamValue::from(p.to_str().unwrap())
}

// ---- Load Structure -----------------------------------------------------------------

#[test]
fn load_structure_reads_the_receptor_and_keeps_one_conformer_per_altloc_residue() {
    let receptor = common::fixture("egfr/receptor.cif");
    for (altloc, atoms) in [("highest_occupancy", 2317), ("first", 2317)] {
        let items = cook(
            kinds::LOAD_STRUCTURE,
            &[("path", path(&receptor)), ("altloc", altloc.into())],
        );
        let out = items.items(&NodeId::new("n"))[0]
            .result
            .as_ref()
            .expect("cooked");
        let arena: &mold_mol::Arena = out.get("structure").unwrap().downcast_ref().unwrap();
        assert_eq!(arena.atom_count(), atoms, "{altloc}");
        assert_eq!(
            out.attrs.get("models"),
            Some(&mold_graph::AttrValue::Int(1))
        );
    }
}

#[test]
fn load_structure_says_what_is_wrong() {
    assert!(error(&cook(kinds::LOAD_STRUCTURE, &[])).contains("`path` is empty"));

    let missing = common::fixture("egfr/not-a-file.cif");
    let e = error(&cook(kinds::LOAD_STRUCTURE, &[("path", path(&missing))]));
    assert!(
        e.contains("cannot read") && e.contains("not-a-file.cif"),
        "{e}"
    );

    // A real file in a format `mol` does not read.
    let smi = common::fixture("egfr/ligands.smi");
    let e = error(&cook(kinds::LOAD_STRUCTURE, &[("path", path(&smi))]));
    assert!(e.contains("unknown structure format"), "{e}");

    let e = error(&cook(
        kinds::LOAD_STRUCTURE,
        &[
            ("path", path(&common::fixture("egfr/receptor.cif"))),
            ("altloc", "best".into()),
        ],
    ));
    assert!(e.contains("`altloc` is \"best\""), "{e}");
}

// ---- Load Ligands -------------------------------------------------------------------

#[test]
fn load_ligands_takes_the_id_from_a_data_item_when_asked() {
    let sdf = common::fixture("egfr/reference_ligand.sdf");
    for (property, want) in [("", "4WKQ_IRE"), ("comp_id", "IRE")] {
        let items = cook(
            kinds::LOAD_LIGANDS,
            &[("path", path(&sdf)), ("id_property", property.into())],
        );
        let out = items.items(&NodeId::new("n"))[0]
            .result
            .as_ref()
            .expect("cooked");
        let set: &LigandSet = out.get("ligands").unwrap().downcast_ref().unwrap();
        assert_eq!(set.single().unwrap().id, want, "id_property = {property:?}");
        assert_eq!(
            out.attrs.get("atoms"),
            Some(&mold_graph::AttrValue::Int(31))
        );
    }
}

#[test]
fn load_ligands_either_skips_a_bad_record_or_says_which_one() {
    let tmp = common::TempDir::new("bad-sdf");
    let good = std::fs::read_to_string(common::fixture("egfr/reference_ligand.sdf")).unwrap();
    // A record whose counts line promises atoms it does not have.
    let bad = "broken\n  mold          3D\n\n  9  0  0  0  0  0  0  0  0  0999 V2000\n$$$$\n";
    let file = tmp.path().join("mixed.sdf");
    std::fs::write(&file, format!("{good}{bad}{good}")).unwrap();

    let skipped = cook(
        kinds::LOAD_LIGANDS,
        &[("path", path(&file)), ("skip_bad_records", true.into())],
    );
    assert_eq!(
        skipped.items(&NodeId::new("n")).len(),
        2,
        "two good records, the bad one skipped"
    );

    let e = error(&cook(
        kinds::LOAD_LIGANDS,
        &[("path", path(&file)), ("skip_bad_records", false.into())],
    ));
    assert!(
        e.contains("record 1") && e.contains("skip_bad_records"),
        "{e}"
    );
}

#[test]
fn load_ligands_says_what_is_wrong() {
    assert!(error(&cook(kinds::LOAD_LIGANDS, &[])).contains("`path` is empty"));

    let tmp = common::TempDir::new("empty-sdf");
    let empty = tmp.path().join("empty.sdf");
    std::fs::write(&empty, "").unwrap();
    let e = error(&cook(kinds::LOAD_LIGANDS, &[("path", path(&empty))]));
    assert!(e.contains("no readable record"), "{e}");
}
