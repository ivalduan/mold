//! Acceptance tests from `crates/recipe/CLAUDE.md` (the TOML ⇄ graph half).
//! Error positions are in `errors.rs`.

mod common;

use std::collections::BTreeMap;

use mold_graph::{Graph, NodeData, NodeId, OnFailure, OutputRef, ParamValue};
use mold_recipe::{Layout, Position, Provenance, Recipe, from_toml, from_toml_with, to_toml};
use proptest::prelude::*;

use common::{FIXTURE, edit_once, registry};

fn load(text: &str) -> Recipe {
    from_toml_with(text, &registry()).unwrap_or_else(|e| panic!("{e}\n---\n{text}"))
}

/// Lines that differ between `a` and `b`, which must have the same number of lines.
fn changed_lines<'a>(a: &'a str, b: &'a str) -> Vec<(&'a str, &'a str)> {
    let (la, lb): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    assert_eq!(la.len(), lb.len(), "line count changed:\n{a}\n---\n{b}");
    la.into_iter().zip(lb).filter(|(x, y)| x != y).collect()
}

// ---------------------------------------------------------------------------------
// Identity: graph → TOML → graph, over generated recipes.

/// Any string, including dots, quotes, newlines, control and non-ASCII characters,
/// mixed with plain identifiers (the common case).
fn name() -> impl Strategy<Value = String> {
    prop_oneof![3 => "[a-z][a-z0-9_]{0,8}", 1 => "(?s).{0,6}"]
}

fn param_value() -> impl Strategy<Value = ParamValue> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(ParamValue::Bool),
        any::<i64>().prop_map(ParamValue::Int),
        // NaN is excluded only because NaN != NaN; it is written as `nan` and read back.
        any::<f64>()
            .prop_filter("not NaN", |f| !f.is_nan())
            .prop_map(ParamValue::Float),
        name().prop_map(ParamValue::Str),
    ];
    leaf.prop_recursive(2, 12, 4, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(ParamValue::List)
    })
}

fn position() -> impl Strategy<Value = Position> {
    let c = || any::<f32>().prop_filter("finite", |f| f.is_finite());
    (c(), c()).prop_map(|(x, y)| Position { x, y })
}

/// Kinds and labels from a small pool most of the time, so several nodes of one kind
/// and repeated labels are common.
fn kind() -> impl Strategy<Value = String> {
    prop_oneof![3 => "(cofold|pose_gate|load)", 1 => name()]
}

fn label() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop_oneof![3 => "(Pose gate|Boltz-2|)", 1 => name()])
}

/// Failure policy: retries, timeout (valid: finite, positive seconds), on_failure.
fn policy() -> impl Strategy<Value = (u32, Option<f64>, OnFailure)> {
    (
        prop_oneof![2 => Just(0u32), 1 => any::<u32>()],
        prop::option::of(prop_oneof![Just(3600.0), 1e-3f64..1e9]),
        prop_oneof![
            Just(OnFailure::Error),
            Just(OnFailure::Warn),
            Just(OnFailure::Ignore)
        ],
    )
}

/// Per node: kind, label, executor, policy, params, inputs as (upstream index, port),
/// and optional layout and provenance. Inputs only point at earlier nodes, so the
/// graph is a DAG.
type NodeParts = (
    String,
    Option<String>,
    Option<String>,
    (u32, Option<f64>, OnFailure),
    BTreeMap<String, ParamValue>,
    BTreeMap<String, (usize, String)>,
    Option<Position>,
    BTreeMap<String, ParamValue>,
);

fn node_parts() -> impl Strategy<Value = NodeParts> {
    (
        kind(),
        label(),
        prop::option::of(name()),
        policy(),
        prop::collection::btree_map(name(), param_value(), 0..5),
        prop::collection::btree_map(name(), (any::<usize>(), name()), 0..4),
        prop::option::of(position()),
        prop::collection::btree_map(name(), param_value(), 0..3),
    )
}

fn recipe() -> impl Strategy<Value = Recipe> {
    (
        prop::collection::btree_set(name(), 0..7),
        prop::option::of(name()),
    )
        .prop_flat_map(|(ids, executor)| {
            let n = ids.len();
            (
                Just(ids.into_iter().collect::<Vec<_>>()),
                Just(executor),
                prop::collection::vec(node_parts(), n),
            )
        })
        .prop_map(|(ids, executor, parts)| {
            let mut r = Recipe::default();
            r.graph.executor = executor;
            for (i, (id, (kind, label, exec, policy, params, inputs, pos, prov))) in
                ids.iter().zip(parts).enumerate()
            {
                let id = NodeId::new(id.as_str());
                let mut node = NodeData::new(kind);
                node.label = label;
                node.executor = exec;
                (node.retries, node.timeout, node.on_failure) = policy;
                node.params.0 = params;
                if i > 0 {
                    for (port, (up, out)) in inputs {
                        node.inputs
                            .insert(port, OutputRef::new(ids[up % i].as_str(), out));
                    }
                }
                if let Some(p) = pos {
                    r.layout.positions.insert(id.clone(), p);
                }
                if !prov.is_empty() {
                    r.provenance.nodes.insert(id.clone(), prov);
                }
                r.graph.nodes.insert(id, node);
            }
            r
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn graph_to_toml_to_graph_is_the_identity(r in recipe()) {
        let text = r.to_toml();
        let back = from_toml(&text).map_err(|e| TestCaseError::fail(format!("{e}\n---\n{text}")))?;
        prop_assert_eq!(&back, &r, "\n{}", text);
        prop_assert_eq!(back.to_toml(), text);
    }

    /// Canvas coordinates are `f32` but TOML reads `f64`: the shortest `f32` text,
    /// read as `f64` and narrowed, is the same `f32`.
    #[test]
    fn every_finite_f32_position_survives(p in position()) {
        let mut r = Recipe::default();
        r.graph.nodes.insert(NodeId::new("n"), NodeData::new("k"));
        r.layout.positions.insert(NodeId::new("n"), p);
        let back = from_toml(&r.to_toml()).unwrap();
        prop_assert_eq!(back.layout.positions[&NodeId::new("n")], p);
    }
}

fn id(s: &str) -> NodeId {
    NodeId::new(s)
}

#[test]
fn the_fixture_loads_against_the_registry() {
    let r = load(FIXTURE);
    let n = |s: &str| &r.graph.nodes[&id(s)];
    assert_eq!(r.graph.nodes.len(), 8);
    assert_eq!(r.graph.executor.as_deref(), Some("local"));

    let c1 = n("cofold-1");
    assert_eq!(c1.label.as_deref(), Some("Boltz-2 with MSA"));
    assert_eq!(c1.executor, None);
    assert_eq!(c1.inputs["msa"], OutputRef::new("msa_query-1", "msa"));
    assert_eq!(c1.params.int("samples").unwrap(), 5);

    let c2 = n("cofold-2");
    assert_eq!(c2.kind, c1.kind);
    assert_eq!(c2.executor.as_deref(), Some("gpu-box"));
    assert_eq!(
        (c2.retries, c2.timeout, c2.on_failure),
        (0, Some(3600.0), OnFailure::Warn)
    );
    assert!(!c2.inputs.contains_key("msa"));
    assert_eq!(c2.params.int("samples").unwrap(), 10);

    assert_eq!(n("msa_query-1").retries, 2);
    assert_eq!(n("prepare_ligands-1").label, None);
    assert_eq!(n("pose_gate-1").label, n("pose_gate-2").label);
    assert_eq!(
        n("pose_gate-2").params.float("pocket_radius").unwrap(),
        10.0
    );
    assert_eq!(
        n("load_structure-1").params.get("chains"),
        Some(&ParamValue::List(vec!["A".into()]))
    );
    assert_eq!(
        r.layout.positions[&id("pose_gate-2")],
        Position { x: 840.0, y: 240.5 }
    );
    assert_eq!(
        r.provenance.nodes[&id("cofold-1")]["worker"],
        ParamValue::from("mock-cofold")
    );
}

#[test]
fn nodes_of_one_kind_and_repeated_labels_keep_their_own_configuration() {
    // Three gates: one kind, one label, different params, policy and wiring. Ids are
    // the identity; kind and label say nothing about which node is which.
    let mut g = Graph::new();
    for s in ["cofold-1", "cofold-2"] {
        g.nodes.insert(id(s), NodeData::new("cofold"));
    }
    for (i, (src, clashes, on_failure)) in [
        ("cofold-1", 0, OnFailure::Error),
        ("cofold-2", 2, OnFailure::Warn),
        ("cofold-2", 5, OnFailure::Ignore),
    ]
    .into_iter()
    .enumerate()
    {
        let mut gate = NodeData::new("pose_gate");
        gate.label = Some("Pose gate".into());
        gate.on_failure = on_failure;
        gate.retries = i as u32;
        gate.params.set("max_clashes", clashes as i64);
        gate.inputs
            .insert("poses".into(), OutputRef::new(src, "poses"));
        g.nodes.insert(id(&format!("pose_gate-{}", i + 1)), gate);
    }
    let text = Recipe::new(g.clone()).to_toml();
    assert_eq!(text.matches("label = \"Pose gate\"").count(), 3, "{text}");
    let back = from_toml(&text).unwrap().graph;
    assert_eq!(back, g);
    let gate = |s: &str| &back.nodes[&id(s)];
    assert_eq!(gate("pose_gate-3").inputs["poses"].node, id("cofold-2"));
    assert_eq!(gate("pose_gate-3").params.int("max_clashes").unwrap(), 5);
    assert_eq!(gate("pose_gate-1").on_failure, OnFailure::Error);
}

#[test]
fn fields_are_written_in_canonical_order() {
    // kind, label, executor, policy, inputs; then the params sub-table.
    let text = load(FIXTURE).to_toml();
    let block = &text[text.find("[node.cofold-2]").unwrap()..];
    let block = &block[..block.find("[node.load_ligands-1]").unwrap()];
    let keys: Vec<_> = block
        .lines()
        .filter_map(|l| l.split_once(" = ").map(|(k, _)| k))
        .collect();
    assert_eq!(
        keys,
        [
            "kind",
            "label",
            "executor",
            "timeout",
            "on_failure",
            "inputs",
            "model",
            "samples",
            "seed"
        ],
        "{block}"
    );
    assert!(block.contains("\n[node.cofold-2.params]\n"));
}

#[test]
fn a_label_change_touches_one_line_and_nothing_else() {
    let base = load(FIXTURE);
    let mut relabeled = base.clone();
    relabeled
        .graph
        .nodes
        .get_mut(&id("cofold-2"))
        .unwrap()
        .label = Some("Boltz-1 (no MSA)".into());
    assert_eq!(
        changed_lines(&base.to_toml(), &relabeled.to_toml()),
        [(
            "label = \"Boltz-1, single sequence\"",
            "label = \"Boltz-1 (no MSA)\""
        )]
    );
}

// ---------------------------------------------------------------------------------
// Hand edit.

#[test]
fn a_param_changed_in_a_text_editor_loads_back_with_only_that_param_changed() {
    let before = load(FIXTURE);
    let after = load(&edit_once(FIXTURE, "samples = 5", "samples = 8"));

    let mut expected = before.clone();
    expected
        .graph
        .nodes
        .get_mut(&id("cofold-1"))
        .unwrap()
        .params
        .set("samples", 8i64);
    assert_eq!(after, expected);
    assert_ne!(after, before);
}

#[test]
fn an_int_typed_into_a_float_param_is_accepted() {
    // `pocket_radius` is declared Float; a hand-edited `10` is an Int, which fits.
    let r = load(&edit_once(
        FIXTURE,
        "pocket_radius = 8.0",
        "pocket_radius = 10",
    ));
    let gate = &r.graph.nodes[&id("pose_gate-1")];
    assert_eq!(gate.params.get("pocket_radius"), Some(&ParamValue::Int(10)));
    assert_eq!(gate.params.float("pocket_radius").unwrap(), 10.0);
}

// ---------------------------------------------------------------------------------
// Byte stability and one-line diffs.

#[test]
fn serialize_parse_serialize_is_byte_identical() {
    let canonical = load(FIXTURE).to_toml();
    assert_eq!(load(&canonical).to_toml(), canonical);
    assert_eq!(load(&canonical), load(FIXTURE));
}

#[test]
fn one_change_is_a_one_line_diff() {
    let base = load(FIXTURE);
    let text = base.to_toml();

    let mut param = base.clone();
    let cofold = param.graph.nodes.get_mut(&id("cofold-1")).unwrap();
    cofold.params.set("samples", 8i64);
    assert_eq!(
        changed_lines(&text, &param.to_toml()),
        [("samples = 5", "samples = 8")]
    );

    let mut new_param = base.clone();
    let gate = new_param.graph.nodes.get_mut(&id("pose_gate-2")).unwrap();
    gate.params.remove("min_contacts");
    let removed = new_param.to_toml();
    assert_eq!(text.lines().count() - removed.lines().count(), 1);
    assert_eq!(removed.matches("min_contacts").count(), 1); // pose_gate-1 keeps its own

    let mut executor = base.clone();
    let cofold = executor.graph.nodes.get_mut(&id("cofold-2")).unwrap();
    cofold.executor = Some("ssh-cluster".into());
    assert_eq!(
        changed_lines(&text, &executor.to_toml()),
        [("executor = \"gpu-box\"", "executor = \"ssh-cluster\"")]
    );

    let mut rewire = base.clone();
    let gate = rewire.graph.nodes.get_mut(&id("pose_gate-2")).unwrap();
    gate.inputs
        .insert("poses".into(), OutputRef::new("cofold-1", "poses"));
    assert_eq!(
        changed_lines(&text, &rewire.to_toml()),
        [(
            r#"inputs = { poses = "cofold-2.poses", receptor = "load_structure-1.structure" }"#,
            r#"inputs = { poses = "cofold-1.poses", receptor = "load_structure-1.structure" }"#
        )]
    );

    let mut policy = base.clone();
    policy.graph.nodes.get_mut(&id("cofold-2")).unwrap().timeout = Some(7200.0);
    assert_eq!(
        changed_lines(&text, &policy.to_toml()),
        [("timeout = 3600.0", "timeout = 7200.0")]
    );

    let mut moved = base.clone();
    moved
        .layout
        .positions
        .insert(id("pose_gate-2"), Position { x: 900.0, y: 240.5 });
    assert_eq!(
        changed_lines(&text, &moved.to_toml()),
        [(
            "pose_gate-2 = { x = 840, y = 240.5 }",
            "pose_gate-2 = { x = 900, y = 240.5 }"
        )]
    );
}

#[test]
fn format_md_shows_the_canonical_form_of_the_fixture() {
    let canonical = load(FIXTURE).to_toml();
    let doc = include_str!("../FORMAT.md");
    assert!(
        doc.contains(&format!("```toml\n{canonical}```")),
        "FORMAT.md is out of date; its canonical example should be:\n{canonical}"
    );
}

// ---------------------------------------------------------------------------------
// Layout.

fn without_layout(text: &str) -> String {
    let start = text.find("# Canvas positions").unwrap();
    let end = text.find("# What answered").unwrap();
    format!("{}{}", &text[..start], &text[end..])
}

#[test]
fn a_recipe_without_layout_loads() {
    let full = load(FIXTURE);
    let bare = load(&without_layout(FIXTURE));
    assert_eq!(bare.layout, Layout::default());
    assert_eq!(bare.graph, full.graph);
    assert_eq!(bare.provenance, full.provenance);
    assert!(!bare.to_toml().contains("[layout"));
}

#[test]
fn layout_changes_do_not_touch_the_graph() {
    // Cache keys are a function of `Graph` alone (`mold-graph` DESIGN.md), so an
    // equal `Graph` means equal keys: moving nodes never dirties a result.
    let a = load(FIXTURE);
    let b = load(&edit_once(
        FIXTURE,
        "x = 560, y = 240",
        "x = -3.25, y = 1e6",
    ));
    assert_ne!(a.layout, b.layout);
    assert_eq!(a.graph, b.graph);
}

#[test]
fn positions_and_provenance_of_removed_nodes_are_not_written() {
    // The canvas keeps positions of removed nodes (for undo); the file does not.
    let mut r = load(FIXTURE);
    r.graph.nodes.remove(&id("pose_gate-2"));
    let text = r.to_toml();
    assert!(!text.contains("pose_gate-2"), "{text}");
    let mut prov = Provenance::default();
    prov.nodes.insert(NodeId::new("ghost"), BTreeMap::new());
    assert_eq!(
        to_toml(&r.graph, &r.layout, &prov),
        to_toml(&r.graph, &r.layout, &Provenance::default())
    );
}

#[test]
fn an_empty_graph_is_just_the_header() {
    assert_eq!(
        Recipe::new(Graph::new()).to_toml(),
        "[recipe]\nformat = 1\n"
    );
    assert_eq!(
        from_toml("[recipe]\nformat = 1\n").unwrap(),
        Recipe::default()
    );
}

#[test]
fn connections_with_dots_round_trip() {
    let mut g = Graph::new();
    g.nodes.insert(NodeId::new("a.b"), NodeData::new("k"));
    let mut n = NodeData::new("k");
    n.inputs.insert("in".into(), OutputRef::new("a.b", "out"));
    n.inputs.insert("odd".into(), OutputRef::new("a.b", "x.y"));
    g.nodes.insert(NodeId::new("c"), n);
    let text = Recipe::new(g.clone()).to_toml();
    assert!(
        text.contains(r#"inputs = { in = "a.b.out", odd = { node = "a.b", port = "x.y" } }"#),
        "{text}"
    );
    assert_eq!(from_toml(&text).unwrap().graph, g);
}
