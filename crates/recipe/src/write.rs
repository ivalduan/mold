//! Graph → TOML. A hand-rolled emitter: the layout of the text (which tables are
//! headers, which are inline, the order of everything) is the format, so it is not
//! left to a generic serializer. See `FORMAT.md`.

use std::fmt::Write as _;

use mold_graph::{Graph, NodeData, OutputRef, ParamValue, Params};

use crate::probe::{default_table, struct_fields};
use crate::{FORMAT_VERSION, Layout, Provenance};

/// The canonical text of a recipe. Deterministic: the same inputs give the same
/// bytes. Layout positions and provenance records of nodes that are not in `graph`
/// are left out (the canvas keeps positions of removed nodes for undo).
pub fn to_toml(graph: &Graph, layout: &Layout, provenance: &Provenance) -> String {
    let mut out = String::new();

    out.push_str("[recipe]\n");
    let _ = writeln!(out, "format = {FORMAT_VERSION}");
    let header = serialize_table(graph);
    let defaults = default_table::<Graph>(toml::Table::new());
    write_fields(
        &mut out,
        &header,
        struct_fields::<Graph>(),
        &["nodes"],
        &[],
        defaults.as_ref(),
    );

    let node_fields = struct_fields::<NodeData>();
    let mut seed = toml::Table::new();
    seed.insert("kind".into(), toml::Value::String(String::new()));
    let node_defaults = default_table::<NodeData>(seed);
    for (id, node) in &graph.nodes {
        let path = format!("node.{}", key(id.as_str()));
        let _ = writeln!(out, "\n[{path}]");
        write_fields(
            &mut out,
            &serialize_table(node),
            node_fields,
            &["params", "inputs"],
            &["kind"],
            node_defaults.as_ref(),
        );
        if !node.inputs.is_empty() {
            out.push_str("inputs = { ");
            for (i, (port, src)) in node.inputs.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{} = {}", key(port), output_ref(src));
            }
            out.push_str(" }\n");
        }
        if !node.params.0.is_empty() {
            let _ = writeln!(out, "\n[{path}.params]");
            write_params(&mut out, &node.params);
        }
    }

    let positions: Vec<_> = layout
        .positions
        .iter()
        .filter(|(id, _)| graph.contains(id))
        .collect();
    if !positions.is_empty() {
        out.push_str("\n[layout.positions]\n");
        for (id, p) in positions {
            let _ = writeln!(
                out,
                "{} = {{ x = {}, y = {} }}",
                key(id.as_str()),
                f32_text(p.x),
                f32_text(p.y)
            );
        }
    }

    for (id, record) in &provenance.nodes {
        if record.is_empty() || !graph.contains(id) {
            continue;
        }
        let _ = writeln!(out, "\n[provenance.{}]", key(id.as_str()));
        for (k, v) in record {
            let _ = writeln!(out, "{} = {}", key(k), param_text(v));
        }
    }
    out
}

/// `value` serialized as a TOML table. Panics only if a graph type stops being
/// representable in TOML, which a test would catch first.
fn serialize_table<T: serde::Serialize>(value: &T) -> toml::Table {
    match toml::Value::try_from(value) {
        Ok(toml::Value::Table(t)) => t,
        Ok(other) => panic!("graph type serialized to {other:?}, not a table"),
        Err(e) => panic!("graph type is not representable in TOML: {e}"),
    }
}

/// `key = value` for each serialized field in declaration order, skipping `skip`
/// (written elsewhere) and fields equal to their default, except `always`.
fn write_fields(
    out: &mut String,
    table: &toml::Table,
    order: &[&str],
    skip: &[&str],
    always: &[&str],
    defaults: Option<&toml::Table>,
) {
    let is_default = |name: &str, v: &toml::Value| {
        !always.contains(&name) && defaults.and_then(|d| d.get(name)) == Some(v)
    };
    for name in order {
        if skip.contains(name) {
            continue;
        }
        if let Some(v) = table.get(*name)
            && !is_default(name, v)
        {
            let _ = writeln!(out, "{} = {}", key(name), value_text(v));
        }
    }
    // Keys the field probe did not report (it found no struct): sorted, not lost.
    for (name, v) in table {
        if !order.contains(&name.as_str()) && !skip.contains(&name.as_str()) {
            let _ = writeln!(out, "{} = {}", key(name), value_text(v));
        }
    }
}

fn write_params(out: &mut String, params: &Params) {
    for (name, v) in params.iter() {
        let _ = writeln!(out, "{} = {}", key(name), param_text(v));
    }
}

/// `"node.port"`, or `{ node = "...", port = "..." }` when the short form would be
/// ambiguous (empty node or port, or a dot in the port). The reader splits the short
/// form at the **last** dot, so node ids may contain dots.
fn output_ref(r: &OutputRef) -> String {
    let short = !r.node.as_str().is_empty() && !r.port.is_empty() && !r.port.contains('.');
    if short {
        string(&format!("{}.{}", r.node, r.port))
    } else {
        format!(
            "{{ node = {}, port = {} }}",
            string(r.node.as_str()),
            string(&r.port)
        )
    }
}

/// A key: bare when TOML allows it, else a quoted basic string.
pub(crate) fn key(k: &str) -> String {
    let bare = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare { k.to_owned() } else { string(k) }
}

/// A TOML basic string (`"..."`), escaped. Never a multi-line or literal string, so
/// the same text is valid as a key or a value.
fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Shortest text that reads back as exactly `f` (Rust's `Debug`, which keeps a `.0`
/// or an exponent so TOML reads a float), with TOML's spelling of the specials.
fn f64_text(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{f:?}")
    }
}

/// A canvas coordinate: an integer when it is one (`x = 320`), else the shortest
/// `f32` text. Read back as `f64` and narrowed (exact; a property test covers it).
fn f32_text(f: f32) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else if f.is_finite() {
        format!("{f:?}")
    } else {
        f64_text(f as f64)
    }
}

pub(crate) fn param_text(v: &ParamValue) -> String {
    match v {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Int(i) => i.to_string(),
        ParamValue::Float(f) => f64_text(*f),
        ParamValue::Str(s) => string(s),
        ParamValue::List(items) => {
            let parts: Vec<_> = items.iter().map(param_text).collect();
            format!("[{}]", parts.join(", "))
        }
    }
}

fn value_text(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => string(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f64_text(*f),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(items) => {
            let parts: Vec<_> = items.iter().map(value_text).collect();
            format!("[{}]", parts.join(", "))
        }
        toml::Value::Table(t) if t.is_empty() => "{}".into(),
        toml::Value::Table(t) => {
            let parts: Vec<_> = t
                .iter()
                .map(|(k, v)| format!("{} = {}", key(k), value_text(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_bare_when_allowed() {
        assert_eq!(key("max_clashes"), "max_clashes");
        assert_eq!(key("load-receptor"), "load-receptor");
        assert_eq!(key("a.b"), "\"a.b\"");
        assert_eq!(key(""), "\"\"");
        assert_eq!(key("é"), "\"é\"");
    }

    #[test]
    fn strings_escape_what_toml_requires() {
        assert_eq!(string("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }

    #[test]
    fn floats_stay_floats() {
        assert_eq!(f64_text(1.0), "1.0");
        assert_eq!(f64_text(0.1), "0.1");
        assert_eq!(f64_text(-0.0), "-0.0");
        assert_eq!(f64_text(1e300), "1e300");
        assert_eq!(f64_text(f64::NEG_INFINITY), "-inf");
        assert_eq!(f32_text(320.0), "320");
        assert_eq!(f32_text(0.1), "0.1");
    }
}
