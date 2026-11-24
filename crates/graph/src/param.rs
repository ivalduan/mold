//! Node parameters: values, declarations, and the per-node parameter map.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hash::KeyHasher;
use crate::node::NodeError;

/// One parameter value. Untagged, so a recipe reads `threshold = 0.5`, `chains = ["A"]`.
///
/// An `Int` is accepted where a `Float` is declared (a hand-edited `threshold = 1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<ParamValue>),
}

impl ParamValue {
    /// The declared kind this value satisfies most narrowly.
    pub fn kind(&self) -> ParamKind {
        match self {
            Self::Bool(_) => ParamKind::Bool,
            Self::Int(_) => ParamKind::Int,
            Self::Float(_) => ParamKind::Float,
            Self::Str(_) => ParamKind::Str,
            Self::List(_) => ParamKind::List,
        }
    }

    /// Whether this value may be stored in a parameter declared as `kind`.
    pub fn fits(&self, kind: ParamKind) -> bool {
        self.kind() == kind || (kind == ParamKind::Float && self.kind() == ParamKind::Int)
    }

    pub(crate) fn hash_into(&self, h: &mut KeyHasher) {
        match self {
            Self::Bool(b) => h.tag(0).tag(*b as u8),
            Self::Int(i) => h.tag(1).bytes(&i.to_le_bytes()),
            // Floats hash by bit pattern; normalise -0.0 so it equals 0.0.
            Self::Float(f) => h.tag(2).bytes(
                &(if *f == 0.0 { 0.0f64 } else { *f })
                    .to_bits()
                    .to_le_bytes(),
            ),
            Self::Str(s) => h.tag(3).str(s),
            Self::List(v) => {
                h.tag(4).bytes(&(v.len() as u64).to_le_bytes());
                for x in v {
                    x.hash_into(h);
                }
                h
            }
        };
    }
}

impl From<bool> for ParamValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<i64> for ParamValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}
impl From<f64> for ParamValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}
impl From<&str> for ParamValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_owned())
    }
}
impl From<String> for ParamValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}

/// A work-item attribute value. Same shape as a parameter value: the two meet when
/// parameters start reading attributes (`@attr` expressions, a follow-up).
pub type AttrValue = ParamValue;

/// A work item's attributes, ordered by name.
pub type Attributes = BTreeMap<String, AttrValue>;

pub(crate) fn hash_attrs(attrs: &Attributes, h: &mut KeyHasher) {
    h.bytes(&(attrs.len() as u64).to_le_bytes());
    for (k, v) in attrs {
        h.str(k);
        v.hash_into(h);
    }
}

/// The declared type of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParamKind {
    Bool,
    Int,
    Float,
    Str,
    List,
}

/// A parameter declaration: name, kind, default, one-line help for the inspector.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub kind: ParamKind,
    pub default: ParamValue,
    pub doc: String,
}

impl ParamSpec {
    pub fn new(name: impl Into<String>, default: impl Into<ParamValue>) -> Self {
        let default = default.into();
        Self {
            name: name.into(),
            kind: default.kind(),
            default,
            doc: String::new(),
        }
    }

    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = doc.into();
        self
    }
}

/// A node's parameter map, ordered by name (stable recipe diffs, stable hashing).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Params(pub BTreeMap<String, ParamValue>);

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.0.get(name)
    }

    pub fn set(
        &mut self,
        name: impl Into<String>,
        value: impl Into<ParamValue>,
    ) -> Option<ParamValue> {
        self.0.insert(name.into(), value.into())
    }

    pub fn remove(&mut self, name: &str) -> Option<ParamValue> {
        self.0.remove(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &ParamValue)> {
        self.0.iter()
    }

    fn require(&self, name: &str) -> Result<&ParamValue, NodeError> {
        self.get(name)
            .ok_or_else(|| NodeError::msg(format!("missing parameter `{name}`")))
    }

    fn wrong(name: &str, want: &str) -> NodeError {
        NodeError::msg(format!("parameter `{name}` is not {want}"))
    }

    pub fn bool(&self, name: &str) -> Result<bool, NodeError> {
        match self.require(name)? {
            ParamValue::Bool(b) => Ok(*b),
            _ => Err(Self::wrong(name, "a bool")),
        }
    }

    pub fn int(&self, name: &str) -> Result<i64, NodeError> {
        match self.require(name)? {
            ParamValue::Int(i) => Ok(*i),
            _ => Err(Self::wrong(name, "an integer")),
        }
    }

    /// Accepts `Int` too.
    pub fn float(&self, name: &str) -> Result<f64, NodeError> {
        match self.require(name)? {
            ParamValue::Float(f) => Ok(*f),
            ParamValue::Int(i) => Ok(*i as f64),
            _ => Err(Self::wrong(name, "a number")),
        }
    }

    pub fn str(&self, name: &str) -> Result<&str, NodeError> {
        match self.require(name)? {
            ParamValue::Str(s) => Ok(s),
            _ => Err(Self::wrong(name, "a string")),
        }
    }

    pub fn list(&self, name: &str) -> Result<&[ParamValue], NodeError> {
        match self.require(name)? {
            ParamValue::List(v) => Ok(v),
            _ => Err(Self::wrong(name, "a list")),
        }
    }

    /// Defaults from `specs`, overlaid with the explicit values. Unknown names are kept
    /// out: what is hashed and handed to `evaluate` is exactly the declared set.
    pub(crate) fn resolved(&self, specs: &[ParamSpec]) -> Params {
        let mut out = Params::new();
        for s in specs {
            let v = self
                .get(&s.name)
                .cloned()
                .unwrap_or_else(|| s.default.clone());
            out.0.insert(s.name.clone(), v);
        }
        out
    }

    pub(crate) fn hash_into(&self, h: &mut KeyHasher) {
        hash_attrs(&self.0, h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_fits_float_and_float_getter_coerces() {
        assert!(ParamValue::Int(1).fits(ParamKind::Float));
        assert!(!ParamValue::Float(1.0).fits(ParamKind::Int));
        let mut p = Params::new();
        p.set("t", 1i64);
        assert_eq!(p.float("t").unwrap(), 1.0);
    }

    #[test]
    fn resolved_applies_defaults_and_drops_unknown() {
        let specs = [ParamSpec::new("a", 1i64), ParamSpec::new("b", "x")];
        let mut p = Params::new();
        p.set("b", "y");
        p.set("zzz", true);
        let r = p.resolved(&specs);
        assert_eq!(r.int("a").unwrap(), 1);
        assert_eq!(r.str("b").unwrap(), "y");
        assert!(r.get("zzz").is_none());
    }
}
