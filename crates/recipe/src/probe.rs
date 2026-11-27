//! Reading a struct's shape from its serde derive, so the recipe follows
//! `mold_graph::NodeData` / `Graph` without naming every field.
//!
//! - [`struct_fields`]: the field names, in declaration order. A recipe key not in
//!   this list is an error (typos are caught); a field added to `NodeData` is
//!   accepted and written with no change here.
//! - [`default_table`]: the serialized value of every defaulted field, so the writer
//!   can leave defaults out even when a field has no `skip_serializing_if`.

use serde::Serialize;
use serde::de::{self, DeserializeOwned, Deserializer, Visitor};

/// The field names `T`'s derived `Deserialize` declares, in declaration order (after
/// `rename`). Empty if `T` does not deserialize as a plain struct (e.g. it uses
/// `#[serde(flatten)]`); a unit test pins the types this crate relies on.
pub(crate) fn struct_fields<T: DeserializeOwned>() -> &'static [&'static str] {
    let mut fields: &'static [&'static str] = &[];
    let _ = T::deserialize(Probe(&mut fields));
    fields
}

/// `T` deserialized from `seed` (its required fields), serialized back: every field
/// that is not skipped, at its default value. `None` if `T` cannot be built from
/// `seed` alone (a required field is missing from it).
pub(crate) fn default_table<T: DeserializeOwned + Serialize>(
    seed: toml::Table,
) -> Option<toml::Table> {
    let value: T = toml::Value::Table(seed).try_into().ok()?;
    match toml::Value::try_from(&value).ok()? {
        toml::Value::Table(t) => Some(t),
        _ => None,
    }
}

/// A deserializer that records the `fields` passed to `deserialize_struct`, then
/// refuses everything.
struct Probe<'a>(&'a mut &'static [&'static str]);

impl<'de> Deserializer<'de> for Probe<'_> {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom("probe"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Self::Error> {
        *self.0 = fields;
        Err(de::Error::custom("probe"))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
        byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
        identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mold_graph::{Graph, NodeData};
    use serde::Deserialize;

    #[test]
    fn graph_types_deserialize_as_plain_structs() {
        // If this fails, a `flatten` (or similar) reached NodeData/Graph and the
        // recipe's unknown-key check would refuse every field.
        // Declaration order is the canonical write order (FORMAT.md).
        assert_eq!(
            struct_fields::<NodeData>(),
            &[
                "kind",
                "label",
                "executor",
                "retries",
                "timeout",
                "on_failure",
                "params",
                "inputs"
            ]
        );
        assert!(struct_fields::<Graph>().contains(&"nodes"));
        assert!(struct_fields::<Graph>().contains(&"executor"));
    }

    /// Stand-in for `NodeData` after ADR-0009 §7 lands: new defaulted fields, one of
    /// them without `skip_serializing_if`.
    #[derive(Serialize, Deserialize)]
    struct Future {
        kind: String,
        #[serde(default)]
        retries: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(default = "error")]
        on_failure: String,
    }

    fn error() -> String {
        "error".into()
    }

    #[test]
    fn new_defaulted_fields_are_found_with_their_defaults() {
        assert_eq!(
            struct_fields::<Future>(),
            &["kind", "retries", "timeout", "on_failure"]
        );
        let mut seed = toml::Table::new();
        seed.insert("kind".into(), "x".into());
        let d = default_table::<Future>(seed).unwrap();
        assert_eq!(d.get("retries"), Some(&toml::Value::Integer(0)));
        assert_eq!(d.get("on_failure"), Some(&toml::Value::from("error")));
        assert!(d.get("timeout").is_none());
        assert!(default_table::<Future>(toml::Table::new()).is_none());
    }
}
