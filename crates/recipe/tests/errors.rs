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
            fixture("kind = \"cofold\"", "kidn = \"cofold\""),
            "kidn",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["`kidn`", "node `cofold`", "`kind`"],
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
}

#[test]
fn a_node_without_kind() {
    check(
        "no kind",
        case(
            fixture("kind = \"load_ligands\"\n", ""),
            "[node.ligands]",
            6,
            |k| *k == ErrorKind::MissingKey,
            &["node `ligands`", "`kind`"],
        ),
    );
}

#[test]
fn unknown_node_kind() {
    check(
        "kind",
        with_registry(case(
            fixture("kind = \"pose_gate\"", "kind = \"pose_gat\""),
            "\"pose_gat\"",
            0,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::UnknownKind(_))),
            &["unknown node kind `pose_gat`", "`gate`"],
        )),
    );
}

#[test]
fn dangling_connection_needs_no_registry() {
    check(
        "dangling",
        case(
            fixture("msa = \"msa.msa\"", "msa = \"msaa.msa\""),
            "\"msaa.msa\"",
            0,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::UnknownNode(_))),
            &["`cofold.msa`", "no node `msaa`"],
        ),
    );
}

#[test]
fn type_mismatched_connection() {
    check(
        "type",
        with_registry(case(
            fixture("poses = \"cofold.poses\"", "poses = \"msa.msa\""),
            "poses = \"msa.msa\"",
            8,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::TypeMismatch { .. })),
            &["`msa.msa` (Msa)", "`gate.poses` (PoseSet)"],
        )),
    );
}

#[test]
fn unknown_ports() {
    check(
        "input",
        with_registry(case(
            fixture("msa = \"msa.msa\"", "mas = \"msa.msa\""),
            "mas = ",
            0,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::UnknownInput { .. })),
            &["no input `mas`"],
        )),
    );
    check(
        "output",
        with_registry(case(
            fixture("msa = \"msa.msa\"", "msa = \"msa.alignment\""),
            "\"msa.alignment\"",
            0,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::UnknownOutput { .. })),
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
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::UnknownParam { .. })),
            &["`cofold` has no parameter `sed`"],
        )),
    );
    check(
        "param type",
        with_registry(case(
            fixture("samples = 5", "samples = \"five\""),
            "\"five\"",
            0,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::ParamKind { .. })),
            &["`cofold.samples` is Int, got Str"],
        )),
    );
    check(
        "param shape",
        case(
            fixture("ph = 7.4", "ph = { value = 7.4 }"),
            "{ value",
            0,
            |k| *k == ErrorKind::BadValue,
            &["`prep.ph`", "not a table"],
        ),
    );
}

#[test]
fn malformed_connection() {
    check(
        "no dot",
        case(
            fixture("ligands = \"prep.ligands\"", "ligands = \"prep\""),
            "\"prep\"",
            0,
            |k| *k == ErrorKind::BadValue,
            &["`cofold.ligands`", "\"node.port\""],
        ),
    );
}

#[test]
fn cycle_needs_no_registry() {
    // prep -> cofold -> gate -> prep. Reported at the first node of the cycle by id.
    check(
        "cycle",
        case(
            fixture(
                "inputs = { ligands = \"ligands.ligands\" }",
                "inputs = { ligands = \"gate.poses\" }",
            ),
            "[node.cofold]",
            6,
            |k| matches!(k, ErrorKind::Graph(g) if matches!(**g, GraphError::Cycle { .. })),
            &["cycle", "`cofold`"],
        ),
    );
}

#[test]
fn a_wrong_type_in_a_serde_mapped_field() {
    // `executor` goes through NodeData's serde derive (as will ADR-0009's `retries`,
    // `timeout`, `on_failure`); the error still lands on the value.
    check(
        "executor",
        case(
            fixture("executor = \"gpu-box\"", "executor = 3"),
            "executor = 3",
            11,
            |k| *k == ErrorKind::BadValue,
            &["`executor` of node `cofold`", "invalid type"],
        ),
    );
}

#[test]
fn layout_and_provenance_errors() {
    check(
        "layout node",
        case(
            fixture("prep = { x = 280", "prepp = { x = 280"),
            "prepp",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["layout position", "no node `prepp`"],
        ),
    );
    check(
        "layout key",
        case(
            fixture("y = 60.5", "z = 60.5"),
            "z = 60.5",
            0,
            |k| *k == ErrorKind::UnknownKey,
            &["`z`", "`gate`"],
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
            fixture("[provenance.msa]", "[provenance.mas]"),
            "[provenance.mas]",
            12,
            |k| *k == ErrorKind::UnknownKey,
            &["provenance record", "no node `mas`"],
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
