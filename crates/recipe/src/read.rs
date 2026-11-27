//! TOML → graph, with a position on every error.
//!
//! The text is parsed with `toml`'s span-carrying `DeTable`; this module walks it
//! table by table, so each refusal points at the key or value at fault. Fields of
//! `Graph` and `NodeData` other than the ones the format shapes itself (`nodes`,
//! `kind`, `params`, `inputs`) go through their serde derive, so a new defaulted
//! field needs no code here (see `probe`).

use std::collections::BTreeMap;
use std::ops::Range;

use mold_graph::{Graph, GraphError, NodeData, NodeId, OutputRef, ParamValue, Params, Registry};
use serde::de::DeserializeOwned;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::error::{ErrorKind, RecipeError};
use crate::probe::struct_fields;
use crate::{FORMAT_VERSION, Layout, Position, Provenance, Recipe};

type Span = Range<usize>;
type Entry<'a, 'i> = (
    &'a Spanned<std::borrow::Cow<'i, str>>,
    &'a Spanned<DeValue<'i>>,
);

const TOP_LEVEL: &[&str] = &["recipe", "node", "layout", "provenance"];

/// Where each part of the graph was written, to place registry errors.
#[derive(Default)]
struct Spans {
    node: BTreeMap<NodeId, Span>,
    kind: BTreeMap<NodeId, Span>,
    param_key: BTreeMap<(NodeId, String), Span>,
    param_value: BTreeMap<(NodeId, String), Span>,
    input_key: BTreeMap<(NodeId, String), Span>,
    input_value: BTreeMap<(NodeId, String), Span>,
    /// Values of serde-mapped node fields (`executor`, `timeout`, ...).
    field_value: BTreeMap<(NodeId, String), Span>,
}

struct Reader<'t> {
    text: &'t str,
    spans: Spans,
}

pub(crate) fn read(text: &str, registry: Option<&Registry>) -> Result<Recipe, RecipeError> {
    let doc = DeTable::parse(text).map_err(|e| {
        let span = e.span().unwrap_or(0..0);
        RecipeError::at(text, span, ErrorKind::Syntax, e.message().to_owned())
    })?;
    let mut r = Reader {
        text,
        spans: Spans::default(),
    };

    let mut header = None;
    let mut nodes = None;
    let mut layout = None;
    let mut provenance = None;
    for entry @ (k, _) in doc.get_ref().iter() {
        let slot = match k.get_ref().as_ref() {
            "recipe" => &mut header,
            "node" => &mut nodes,
            "layout" => &mut layout,
            "provenance" => &mut provenance,
            other => {
                return Err(r.unknown_key(k.span(), &format!("table `{other}`"), TOP_LEVEL));
            }
        };
        *slot = Some(entry);
    }

    let Some(header) = header else {
        return Err(r.err(
            0..0,
            ErrorKind::MissingKey,
            format!("missing the `[recipe]` table (it holds `format = {FORMAT_VERSION}`)"),
        ));
    };
    let mut graph = r.header(header)?;
    if let Some(nodes) = nodes {
        for entry in r.table(nodes.1, "`node`")?.iter() {
            let (id, node) = r.node(entry)?;
            graph.nodes.insert(id, node);
        }
    }
    r.check_structure(&graph)?;
    if let Some(reg) = registry {
        graph.validate(reg).map_err(|e| r.graph_error(&graph, e))?;
    }

    let layout = match layout {
        Some(l) => r.layout(l, &graph)?,
        None => Layout::default(),
    };
    let provenance = match provenance {
        Some(p) => r.provenance(p, &graph)?,
        None => Provenance::default(),
    };
    Ok(Recipe {
        graph,
        layout,
        provenance,
    })
}

impl Reader<'_> {
    fn err(&self, span: Span, kind: ErrorKind, message: impl Into<String>) -> RecipeError {
        RecipeError::at(self.text, span, kind, message)
    }

    fn unknown_key(&self, span: Span, what: &str, expected: &[&str]) -> RecipeError {
        let list: Vec<_> = expected.iter().map(|k| format!("`{k}`")).collect();
        self.err(
            span,
            ErrorKind::UnknownKey,
            format!("unknown {what}; expected one of {}", list.join(", ")),
        )
    }

    fn table<'a, 'i>(
        &self,
        v: &'a Spanned<DeValue<'i>>,
        what: &str,
    ) -> Result<&'a DeTable<'i>, RecipeError> {
        match v.get_ref() {
            DeValue::Table(t) => Ok(t),
            other => Err(self.err(
                v.span(),
                ErrorKind::BadValue,
                format!("{what} must be a table, not {}", type_name(other)),
            )),
        }
    }

    fn string<'a>(&self, v: &'a Spanned<DeValue<'_>>, what: &str) -> Result<&'a str, RecipeError> {
        v.get_ref().as_str().ok_or_else(|| {
            self.err(
                v.span(),
                ErrorKind::BadValue,
                format!("{what} must be a string, not {}", type_name(v.get_ref())),
            )
        })
    }

    /// `[recipe]`: `format`, then `Graph`'s own fields (`executor`, ...).
    fn header(&self, (key, value): Entry<'_, '_>) -> Result<Graph, RecipeError> {
        let table = self.table(value, "`recipe`")?;
        let allowed: Vec<&str> = std::iter::once("format")
            .chain(struct_fields::<Graph>().iter().copied())
            .filter(|f| *f != "nodes")
            .collect();
        let mut format = None;
        let mut fields = Vec::new();
        for (k, v) in table.iter() {
            match k.get_ref().as_ref() {
                "format" => format = Some(v),
                name if allowed.contains(&name) => {
                    fields.push((name.to_owned(), self.value(v)?, v.span()));
                }
                name => {
                    return Err(self.unknown_key(
                        k.span(),
                        &format!("key `{name}` in `[recipe]`"),
                        &allowed,
                    ));
                }
            }
        }
        let Some(format) = format else {
            return Err(self.err(
                key.span(),
                ErrorKind::MissingKey,
                format!(
                    "`[recipe]` has no `format` (this build reads `format = {FORMAT_VERSION}`)"
                ),
            ));
        };
        let version = self.int(format, "`format`")?;
        if version != i64::from(FORMAT_VERSION) {
            return Err(self.err(
                format.span(),
                ErrorKind::UnsupportedFormat,
                format!("recipe format {version} is not supported (this build reads format {FORMAT_VERSION})"),
            ));
        }
        self.deserialize(toml::Table::new(), fields, value.span(), "`[recipe]`")
    }

    /// One `[node.<id>]` table.
    fn node(&mut self, (key, value): Entry<'_, '_>) -> Result<(NodeId, NodeData), RecipeError> {
        let id = NodeId::new(key.get_ref().as_ref());
        let what = format!("node `{id}`");
        let table = self.table(value, &what)?;
        let fields = struct_fields::<NodeData>();
        let mut kind = None;
        let mut params = Params::new();
        let mut inputs = BTreeMap::new();
        let mut extra = Vec::new();
        for (k, v) in table.iter() {
            let name = k.get_ref().as_ref();
            match name {
                "kind" => {
                    kind = Some(self.string(v, &format!("the kind of {what}"))?.to_owned());
                    self.spans.kind.insert(id.clone(), v.span());
                }
                "params" => {
                    for (pk, pv) in self.table(v, &format!("`params` of {what}"))?.iter() {
                        let pname = pk.get_ref().to_string();
                        let pval = self.param(pv, &format!("parameter `{id}.{pname}`"))?;
                        let at = (id.clone(), pname.clone());
                        self.spans.param_key.insert(at.clone(), pk.span());
                        self.spans.param_value.insert(at, pv.span());
                        params.set(pname, pval);
                    }
                }
                "inputs" => {
                    for (ik, iv) in self.table(v, &format!("`inputs` of {what}"))?.iter() {
                        let port = ik.get_ref().to_string();
                        let src = self.output_ref(iv, &format!("input `{id}.{port}`"))?;
                        let at = (id.clone(), port.clone());
                        self.spans.input_key.insert(at.clone(), ik.span());
                        self.spans.input_value.insert(at, iv.span());
                        inputs.insert(port, src);
                    }
                }
                _ if fields.contains(&name) => {
                    self.spans
                        .field_value
                        .insert((id.clone(), name.to_owned()), v.span());
                    extra.push((name.to_owned(), self.value(v)?, v.span()));
                }
                _ => {
                    return Err(self.unknown_key(
                        k.span(),
                        &format!("field `{name}` in {what}"),
                        fields,
                    ));
                }
            }
        }
        let Some(kind) = kind else {
            return Err(self.err(
                key.span(),
                ErrorKind::MissingKey,
                format!("{what} has no `kind`"),
            ));
        };
        self.spans.node.insert(id.clone(), key.span());
        let mut seed = toml::Table::new();
        seed.insert("kind".into(), toml::Value::String(kind));
        let mut node: NodeData = self.deserialize(seed, extra, value.span(), &what)?;
        node.params = params;
        node.inputs = inputs;
        Ok((id, node))
    }

    /// Deserialize `T` from `seed` plus `fields`. On failure, retry each field alone
    /// on top of `seed` to point at the one at fault.
    fn deserialize<T: DeserializeOwned>(
        &self,
        seed: toml::Table,
        fields: Vec<(String, toml::Value, Span)>,
        table_span: Span,
        what: &str,
    ) -> Result<T, RecipeError> {
        let mut all = seed.clone();
        for (name, v, _) in &fields {
            all.insert(name.clone(), v.clone());
        }
        let first_err = match toml::Value::Table(all).try_into::<T>() {
            Ok(t) => return Ok(t),
            Err(e) => e,
        };
        for (name, v, span) in fields {
            let mut one = seed.clone();
            one.insert(name.clone(), v);
            if let Err(e) = toml::Value::Table(one).try_into::<T>() {
                return Err(self.err(
                    span,
                    ErrorKind::BadValue,
                    format!("`{name}` of {what}: {}", e.message()),
                ));
            }
        }
        Err(self.err(
            table_span,
            ErrorKind::BadValue,
            format!("{what}: {}", first_err.message()),
        ))
    }

    fn int(&self, v: &Spanned<DeValue<'_>>, what: &str) -> Result<i64, RecipeError> {
        match v.get_ref() {
            DeValue::Integer(i) => i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| {
                self.err(
                    v.span(),
                    ErrorKind::BadValue,
                    format!("{what} does not fit a 64-bit integer"),
                )
            }),
            other => Err(self.err(
                v.span(),
                ErrorKind::BadValue,
                format!("{what} must be an integer, not {}", type_name(other)),
            )),
        }
    }

    fn float(&self, v: &Spanned<DeValue<'_>>, what: &str) -> Result<f64, RecipeError> {
        match v.get_ref() {
            DeValue::Float(f) => f.as_str().parse().map_err(|_| {
                self.err(
                    v.span(),
                    ErrorKind::BadValue,
                    format!("{what}: unreadable float"),
                )
            }),
            DeValue::Integer(_) => Ok(self.int(v, what)? as f64),
            other => Err(self.err(
                v.span(),
                ErrorKind::BadValue,
                format!("{what} must be a number, not {}", type_name(other)),
            )),
        }
    }

    /// A parameter (or provenance) value: bool, integer, float, string or a list of
    /// those. Tables and datetimes are refused.
    fn param(&self, v: &Spanned<DeValue<'_>>, what: &str) -> Result<ParamValue, RecipeError> {
        Ok(match v.get_ref() {
            DeValue::Boolean(b) => ParamValue::Bool(*b),
            DeValue::Integer(_) => ParamValue::Int(self.int(v, what)?),
            DeValue::Float(_) => ParamValue::Float(self.float(v, what)?),
            DeValue::String(s) => ParamValue::Str(s.to_string()),
            DeValue::Array(items) => ParamValue::List(
                items
                    .iter()
                    .map(|x| self.param(x, what))
                    .collect::<Result<_, _>>()?,
            ),
            other => {
                return Err(self.err(
                    v.span(),
                    ErrorKind::BadValue,
                    format!(
                        "{what} must be a boolean, number, string or list, not {}",
                        type_name(other)
                    ),
                ));
            }
        })
    }

    /// `"node.port"` (split at the last dot) or `{ node = "...", port = "..." }`.
    fn output_ref(&self, v: &Spanned<DeValue<'_>>, what: &str) -> Result<OutputRef, RecipeError> {
        let bad = || {
            self.err(
                v.span(),
                ErrorKind::BadValue,
                format!(
                    "{what} must name an upstream output as \"node.port\" or {{ node = \"...\", port = \"...\" }}"
                ),
            )
        };
        match v.get_ref() {
            DeValue::String(s) => match s.rsplit_once('.') {
                Some((node, port)) if !node.is_empty() && !port.is_empty() => {
                    Ok(OutputRef::new(node, port))
                }
                _ => Err(bad()),
            },
            DeValue::Table(t) => {
                let mut node = None;
                let mut port = None;
                for (k, x) in t.iter() {
                    match k.get_ref().as_ref() {
                        "node" => node = Some(self.string(x, &format!("`node` of {what}"))?),
                        "port" => port = Some(self.string(x, &format!("`port` of {what}"))?),
                        other => {
                            return Err(self.unknown_key(
                                k.span(),
                                &format!("key `{other}` in {what}"),
                                &["node", "port"],
                            ));
                        }
                    }
                }
                match (node, port) {
                    (Some(n), Some(p)) => Ok(OutputRef::new(n, p)),
                    _ => Err(bad()),
                }
            }
            _ => Err(bad()),
        }
    }

    /// Any TOML value, for serde-mapped fields.
    fn value(&self, v: &Spanned<DeValue<'_>>) -> Result<toml::Value, RecipeError> {
        Ok(match v.get_ref() {
            DeValue::String(s) => toml::Value::String(s.to_string()),
            DeValue::Integer(_) => toml::Value::Integer(self.int(v, "value")?),
            DeValue::Float(_) => toml::Value::Float(self.float(v, "value")?),
            DeValue::Boolean(b) => toml::Value::Boolean(*b),
            DeValue::Datetime(d) => toml::Value::Datetime(*d),
            DeValue::Array(items) => toml::Value::Array(
                items
                    .iter()
                    .map(|x| self.value(x))
                    .collect::<Result<_, _>>()?,
            ),
            DeValue::Table(t) => {
                let mut out = toml::Table::new();
                for (k, x) in t.iter() {
                    out.insert(k.get_ref().to_string(), self.value(x)?);
                }
                toml::Value::Table(out)
            }
        })
    }

    /// Checks that need no registry: every connection names a node of this recipe,
    /// and connections form no cycle.
    fn check_structure(&self, graph: &Graph) -> Result<(), RecipeError> {
        for (id, node) in &graph.nodes {
            for (port, src) in &node.inputs {
                if !graph.contains(&src.node) {
                    let span = self.spans.input_value[&(id.clone(), port.clone())].clone();
                    return Err(self.err(
                        span,
                        ErrorKind::Graph(Box::new(GraphError::UnknownNode(src.node.clone()))),
                        format!(
                            "input `{id}.{port}` is connected to `{}.{}`, but there is no node `{}`",
                            src.node, src.port, src.node
                        ),
                    ));
                }
            }
            // Graph's rule, through its public API: a timeout the failure policy drops
            // is not a finite, positive number of seconds.
            if let Some(seconds) = node.timeout
                && node.failure_policy().timeout.is_none()
            {
                let e = GraphError::InvalidTimeout {
                    node: id.clone(),
                    seconds,
                };
                return Err(self.graph_error(graph, e));
            }
        }
        graph
            .topo_order()
            .map(|_| ())
            .map_err(|e| self.graph_error(graph, e))
    }

    /// Place a `GraphError` (from `Graph::validate` or `topo_order`) in the text.
    fn graph_error(&self, graph: &Graph, e: GraphError) -> RecipeError {
        let s = &self.spans;
        let pair = |n: &NodeId, k: &str| (n.clone(), k.to_owned());
        // The first input fed by `node.port` (`port = None`: by any port of `node`).
        let input_from = |node: &NodeId, port: Option<&str>| {
            graph.nodes.iter().find_map(|(id, d)| {
                d.inputs.iter().find_map(|(input, src)| {
                    (src.node == *node && port.is_none_or(|p| p == src.port))
                        .then(|| s.input_value.get(&pair(id, input)).cloned())
                        .flatten()
                })
            })
        };
        let span = match &e {
            GraphError::UnknownKind(k) => graph
                .nodes
                .iter()
                .find(|(_, d)| d.kind == *k)
                .and_then(|(id, _)| s.kind.get(id).cloned()),
            GraphError::UnknownParam { node, name } => s.param_key.get(&pair(node, name)).cloned(),
            GraphError::ParamKind { node, name, .. } => {
                s.param_value.get(&pair(node, name)).cloned()
            }
            GraphError::UnknownInput { node, port } => s.input_key.get(&pair(node, port)).cloned(),
            GraphError::UnknownOutput { node, port } => input_from(node, Some(port)),
            GraphError::UnknownNode(node) => input_from(node, None),
            GraphError::TypeMismatch { to, input, .. } => {
                s.input_value.get(&pair(to, input)).cloned()
            }
            GraphError::Cycle { at } | GraphError::DuplicateNode(at) => s.node.get(at).cloned(),
            GraphError::InvalidTimeout { node, .. } => {
                s.field_value.get(&pair(node, "timeout")).cloned()
            }
            GraphError::NotConnected { node, input } => {
                s.input_key.get(&pair(node, input)).cloned()
            }
        };
        let message = match &e {
            GraphError::UnknownKind(k) => {
                let ids: Vec<_> = graph
                    .nodes
                    .iter()
                    .filter(|(_, d)| d.kind == *k)
                    .map(|(id, _)| format!("`{id}`"))
                    .collect();
                format!("unknown node kind `{k}` (node {})", ids.join(", "))
            }
            GraphError::Cycle { at } => format!("connections form a cycle through node `{at}`"),
            other => other.to_string(),
        };
        self.err(span.unwrap_or(0..0), ErrorKind::Graph(Box::new(e)), message)
    }

    /// `[layout]`: `positions = { <id> = { x, y } }`.
    fn layout(&self, (_, value): Entry<'_, '_>, graph: &Graph) -> Result<Layout, RecipeError> {
        let mut layout = Layout::default();
        for (k, v) in self.table(value, "`layout`")?.iter() {
            if k.get_ref() != "positions" {
                return Err(self.unknown_key(
                    k.span(),
                    &format!("key `{}` in `[layout]`", k.get_ref()),
                    &["positions"],
                ));
            }
            for (ik, iv) in self.table(v, "`layout.positions`")?.iter() {
                let id = NodeId::new(ik.get_ref().as_ref());
                self.known_node(graph, &id, ik.span(), "a layout position")?;
                let what = format!("the position of `{id}`");
                let (mut x, mut y) = (None, None);
                for (ck, cv) in self.table(iv, &what)?.iter() {
                    let slot = match ck.get_ref().as_ref() {
                        "x" => &mut x,
                        "y" => &mut y,
                        other => {
                            return Err(self.unknown_key(
                                ck.span(),
                                &format!("key `{other}` in {what}"),
                                &["x", "y"],
                            ));
                        }
                    };
                    *slot = Some(
                        self.float(cv, &format!("`{other}` of {what}", other = ck.get_ref()))?
                            as f32,
                    );
                }
                let (Some(x), Some(y)) = (x, y) else {
                    return Err(self.err(
                        iv.span(),
                        ErrorKind::MissingKey,
                        format!("{what} needs both `x` and `y`"),
                    ));
                };
                layout.positions.insert(id, Position { x, y });
            }
        }
        Ok(layout)
    }

    /// `[provenance.<id>]`: free-form records, values as parameter values.
    fn provenance(
        &self,
        (_, value): Entry<'_, '_>,
        graph: &Graph,
    ) -> Result<Provenance, RecipeError> {
        let mut prov = Provenance::default();
        for (k, v) in self.table(value, "`provenance`")?.iter() {
            let id = NodeId::new(k.get_ref().as_ref());
            self.known_node(graph, &id, k.span(), "a provenance record")?;
            let what = format!("provenance of `{id}`");
            let mut record = BTreeMap::new();
            for (rk, rv) in self.table(v, &what)?.iter() {
                let name = rk.get_ref().to_string();
                let val = self.param(rv, &format!("`{name}` in the {what}"))?;
                record.insert(name, val);
            }
            prov.nodes.insert(id, record);
        }
        Ok(prov)
    }

    fn known_node(
        &self,
        graph: &Graph,
        id: &NodeId,
        span: Span,
        what: &str,
    ) -> Result<(), RecipeError> {
        if graph.contains(id) {
            Ok(())
        } else {
            Err(self.err(
                span,
                ErrorKind::UnknownKey,
                format!("{what} for `{id}`, but there is no node `{id}`"),
            ))
        }
    }
}

fn type_name(v: &DeValue<'_>) -> &'static str {
    match v {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a datetime",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}
