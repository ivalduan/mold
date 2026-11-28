//! Every `RecipeError` case points at the offending key or value, with a message a
//! person can act on. Each case is a one-place edit of the fixture recipe.

mod common;

use mold_graph::GraphError;
use mold_recipe::{ErrorKind, RecipeError, from_toml, from_toml_with};

use common::{FIXTURE, edit_once, registry};

/// 1-based (line, column) of the first `needle` in `text`, plus `skip` bytes.
fn pos(text: &str, needle: &str, skip: usize) -> (usize, usize) {
    let at = text.find(needle).unwrap_or_else(|| panic!("no `{needle}`")) + skip;
    let line = text[..at].matches('\n').count() + 1;
    let col = text[..at].rsplit('\n').next().unwrap().chars().count() + 1;
    (line, col)
}

struct Case {
    text: String,
    with_registry: bool,
    /// The error is at the first `needle`, plus `skip` bytes.
    needle: &'static str,
    skip: usize,
    kind: fn(&ErrorKind) -> bool,
    /// Substrings the message must contain.
    says: &'static [&'static str],
}

fn check(name: &str, c: Case) {
    let err: RecipeError = if c.with_registry {
        from_toml_with(&c.text, &registry())
    } else {
        from_toml(&c.text)
    }
    .expect_err(name);
    let (line, column) = pos(&c.text, c.needle, c.skip);
    assert_eq!(
        (err.line, err.column),
        (line, column),
        "{name}: wrong position for `{err}`"
    );
    assert!((c.kind)(&err.kind), "{name}: wrong kind {:?}", err.kind);
    for s in c.says {
        assert!(
            err.message.contains(s),
            "{name}: `{}` lacks `{s}`",
            err.message
        );
    }
    // Display carries the position.
    assert!(
        err.to_string()
            .starts_with(&format!("line {line}, column {column}: "))
    );
}

fn case(
    text: String,
    needle: &'static str,
    skip: usize,
    kind: fn(&ErrorKind) -> bool,
    says: &'static [&'static str],
) -> Case {
    Case {
        text,
        with_registry: false,
        needle,
        skip,
        kind,
        says,
    }
}

fn with_registry(mut c: Case) -> Case {
    c.with_registry = true;
    c
}

fn fixture(from: &str, to: &str) -> String {
    edit_once(FIXTURE, from, to)
}

fn graph(k: &ErrorKind, is: fn(&GraphError) -> bool) -> bool {
    matches!(k, ErrorKind::Graph(g) if is(g))
}

#[test]
fn not_toml() {
    check(
        "syntax",
        case(
            fixture("samples = 5", "samples = "),
            "samples = ",
            0,
            |k| *k == ErrorKind::Syntax,
            &[],
        )
        .line_only(),
    );
}

#[test]
fn unknown_top_level_table() {
    check(
        "top-level",
        case(
            format!("{FIXTURE}\n[nodes.extra]\nkind = \"x\"\n"),
            "[nodes.extra]",
            1,
            |k| *k == ErrorKind::UnknownKey,
            &["`nodes`", "`node`"],
        ),
    );
}

#[test]
fn missing_header_and_format() {
    let no_header = fixture("[recipe]\nformat = 1\n", "");
    let no_header = no_header.replacen("executor = \"local\"", "", 1);
    // The first `executor` left is a node's; with no `[recipe]` the error is at 1:1.
    let err = from_toml(&no_header).unwrap_err();
    assert_eq!(
        (err.line, err.column, &err.kind),
        (1, 1, &ErrorKind::MissingKey)
    );
    assert!(err.message.contains("[recipe]"), "{err}");

    check(
        "no format",
        case(
            fixture("format = 1\n", ""),
            "[recipe]",
            1,
            |k| *k == ErrorKind::MissingKey,
            &["format"],
        ),
    );
}

#[test]
fn unsupported_format() {
    check(
        "format 2",
        case(
            fixture("format = 1", "format = 2"),
            "format = 2",
            9,
            |k| *k == ErrorKind::UnsupportedFormat,
            &["format 2", "format 1"],
        ),
    );
}

#[test]
fn a_typo_in_a_node_field_is_caught() {
    check(
        "kidn",
        case(
            fixture("kind = \"msa_query\"", "kidn = \"msa_query\""),
            "kidn",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["`kidn`", "node `msa_query-1`", "`kind`", "`label`"],
        ),
    );
    check(
        "header typo",
        case(
            fixture("executor = \"local\"", "exector = \"local\""),
            "exector",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["`exector`", "`executor`"],
        ),
    );
    check(
        "policy typo",
        case(
            fixture("retries = 2", "retry = 2"),
            "retry = 2",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["`retry`", "`retries`"],
        ),
    );
}

#[test]
fn a_node_without_kind() {
    check(
        "no kind",
        case(
            fixture("kind = \"load_ligands\"\n", ""),
            "[node.load_ligands-1]",
            6,
            |k| *k == ErrorKind::MissingKey,
            &["node `load_ligands-1`", "`kind`"],
        ),
    );
}

#[test]
fn unknown_node_kind() {
    check(
        "kind",
        with_registry(case(
            fixture("kind = \"prepare_ligands\"", "kind = \"prepare_ligand\""),
            "\"prepare_ligand\"",
            0,
            |k| graph(k, |g| matches!(g, GraphError::UnknownKind(_))),
            &["unknown node kind `prepare_ligand`", "`prepare_ligands-1`"],
        )),
    );
}

#[test]
fn dangling_connection_needs_no_registry() {
    check(
        "dangling",
        case(
            fixture("msa = \"msa_query-1.msa\"", "msa = \"msaa.msa\""),
            "\"msaa.msa\"",
            0,
            |k| graph(k, |g| matches!(g, GraphError::UnknownNode(_))),
            &["`cofold-1.msa`", "no node `msaa`"],
        ),
    );
}

#[test]
fn type_mismatched_connection() {
    check(
        "type",
        with_registry(case(
            fixture("poses = \"cofold-1.poses\"", "poses = \"msa_query-1.msa\""),
            "poses = \"msa_query-1.msa\"",
            8,
            |k| graph(k, |g| matches!(g, GraphError::TypeMismatch { .. })),
            &["`msa_query-1.msa` (Msa)", "`pose_gate-1.poses` (PoseSet)"],
        )),
    );
}

#[test]
fn unknown_ports() {
    check(
        "input",
        with_registry(case(
            fixture("msa = \"msa_query-1.msa\"", "mas = \"msa_query-1.msa\""),
            "mas = ",
            0,
            |k| graph(k, |g| matches!(g, GraphError::UnknownInput { .. })),
            &["no input `mas`"],
        )),
    );
    check(
        "output",
        with_registry(case(
            fixture(
                "msa = \"msa_query-1.msa\"",
                "msa = \"msa_query-1.alignment\"",
            ),
            "\"msa_query-1.alignment\"",
            0,
            |k| graph(k, |g| matches!(g, GraphError::UnknownOutput { .. })),
            &["no output `alignment`"],
        )),
    );
}

#[test]
fn param_errors() {
    check(
        "unknown param",
        with_registry(case(
            fixture("seed = 42", "sed = 42"),
            "sed = 42",
            0,
            |k| graph(k, |g| matches!(g, GraphError::UnknownParam { .. })),
            &["`cofold-1` has no parameter `sed`"],
        )),
    );
    check(
        "param type",
        with_registry(case(
            fixture("samples = 5", "samples = \"five\""),
            "\"five\"",
            0,
            |k| graph(k, |g| matches!(g, GraphError::ParamKind { .. })),
            &["`cofold-1.samples` is Int, got Str"],
        )),
    );
    check(
        "param shape",
        case(
            fixture("ph = 7.4", "ph = { value = 7.4 }"),
            "{ value",
            0,
            |k| *k == ErrorKind::BadValue,
            &["`prepare_ligands-1.ph`", "not a table"],
        ),
    );
}

#[test]
fn malformed_connection() {
    check(
        "no dot",
        case(
            fixture(
                "structure = \"load_structure-1.structure\"",
                "structure = \"load_structure-1\"",
            ),
            "\"load_structure-1\"",
            0,
            |k| *k == ErrorKind::BadValue,
            &["`msa_query-1.structure`", "\"node.port\""],
        ),
    );
}

#[test]
fn cycle_needs_no_registry() {
    // prepare_ligands-1 -> cofold-1 -> pose_gate-1 -> prepare_ligands-1. Reported at
    // the first node (by id) that cannot be ordered.
    check(
        "cycle",
        case(
            fixture(
                "inputs = { ligands = \"load_ligands-1.ligands\" }",
                "inputs = { ligands = \"pose_gate-1.poses\" }",
            ),
            "[node.cofold-1]",
            6,
            |k| graph(k, |g| matches!(g, GraphError::Cycle { .. })),
            &["cycle", "`cofold-1`"],
        ),
    );
}

#[test]
fn wrong_types_in_serde_mapped_fields() {
    // `label`, `executor` and the failure policy go through NodeData's serde derive;
    // the error still lands on the value.
    check(
        "label",
        case(
            fixture("label = \"Quinazoline series\"", "label = 3"),
            "label = 3",
            8,
            |k| *k == ErrorKind::BadValue,
            &["`label` of node `load_ligands-1`", "invalid type"],
        ),
    );
    check(
        "executor",
        case(
            fixture("executor = \"gpu-box\"", "executor = 3"),
            "executor = 3",
            11,
            |k| *k == ErrorKind::BadValue,
            &["`executor` of node `cofold-2`", "invalid type"],
        ),
    );
    check(
        "retries",
        case(
            fixture("retries = 2", "retries = -1"),
            "retries = -1",
            10,
            |k| *k == ErrorKind::BadValue,
            &["`retries` of node `msa_query-1`"],
        ),
    );
    check(
        "on_failure",
        case(
            fixture("on_failure = \"warn\"", "on_failure = \"warm\""),
            "\"warm\"",
            0,
            |k| *k == ErrorKind::BadValue,
            &["`on_failure` of node `cofold-2`", "warm"],
        ),
    );
}

#[test]
fn invalid_timeout_needs_no_registry() {
    for bad in ["-1.0", "0", "inf", "nan"] {
        let to = format!("timeout = {bad}");
        let text = fixture("timeout = 3600.0", &to);
        let err = from_toml(&text).expect_err(bad);
        assert_eq!((err.line, err.column), pos(&text, &to, 10), "{bad}: {err}");
        assert!(
            graph(&err.kind, |g| matches!(
                g,
                GraphError::InvalidTimeout { .. }
            )),
            "{bad}: {:?}",
            err.kind
        );
        assert!(err.message.contains("`cofold-2`"), "{err}");
        assert!(err.message.contains("positive number of seconds"), "{err}");
    }
}

#[test]
fn layout_and_provenance_errors() {
    check(
        "layout node",
        case(
            fixture("prepare_ligands-1 = { x", "prepare_ligand-1 = { x"),
            "prepare_ligand-1",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["layout position", "no node `prepare_ligand-1`"],
        ),
    );
    check(
        "layout key",
        case(
            fixture("y = 240.5", "z = 240.5"),
            "z = 240.5",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["`z`", "`pose_gate-2`"],
        ),
    );
    check(
        "layout section",
        case(
            fixture("[layout.positions]", "[layout.position]"),
            "[layout.position]",
            8,
            |k| *k == ErrorKind::UnknownKey,
            &["`position`", "`positions`"],
        ),
    );
    check(
        "provenance node",
        case(
            fixture("[provenance.msa_query-1]", "[provenance.msa_query-2]"),
            "[provenance.msa_query-2]",
            12,
            |k| *k == ErrorKind::UnknownKey,
            &["provenance record", "no node `msa_query-2`"],
        ),
    );
}

impl Case {
    /// Syntax errors: the parser picks the column; only the line is pinned.
    fn line_only(self) -> Case {
        let err = from_toml(&self.text).unwrap_err();
        let (line, _) = pos(&self.text, self.needle, self.skip);
        assert_eq!(err.line, line, "{err}");
        // Re-anchor the needle at the reported column so `check` passes the rest.
        let line_text = self.text.lines().nth(line - 1).unwrap();
        let col_byte: usize = line_text
            .chars()
            .take(err.column - 1)
            .map(char::len_utf8)
            .sum();
        let skip = col_byte - line_text.find(self.needle).unwrap();
        Case {
            skip: self.skip + skip,
            ..self
        }
    }
}
