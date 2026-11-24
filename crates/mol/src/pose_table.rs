//! The pose table: one Polars `DataFrame`, one row per pose, joined to the arena
//! by `pose_id`. It never holds coordinates.
//!
//! [`SCHEMA`] is the typed form of the [`pose_columns`](crate::pose_columns)
//! contract: the column names in their canonical order, each with its dtype.

use polars::prelude::{DataFrame, DataType, Field, Schema};

use crate::pose_columns as col;

/// Column dtype, independent of Polars so non-Polars code can read the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnType {
    UInt32,
    Float64,
    Boolean,
    String,
}

impl ColumnType {
    pub fn to_polars(self) -> DataType {
        match self {
            ColumnType::UInt32 => DataType::UInt32,
            ColumnType::Float64 => DataType::Float64,
            ColumnType::Boolean => DataType::Boolean,
            ColumnType::String => DataType::String,
        }
    }
}

/// Every pose-table column, in canonical order, with its dtype.
pub const SCHEMA: [(&str, ColumnType); 13] = [
    (col::POSE_ID, ColumnType::UInt32),
    (col::LIGAND_ID, ColumnType::String),
    (col::MODEL_RANK, ColumnType::UInt32),
    (col::PLDDT, ColumnType::Float64),
    (col::IPTM, ColumnType::Float64),
    (col::AFFINITY_PRED, ColumnType::Float64),
    (col::BINDER_PROB, ColumnType::Float64),
    (col::CLASH_COUNT, ColumnType::UInt32),
    (col::STRAIN_KCAL, ColumnType::Float64),
    (col::N_HBONDS, ColumnType::UInt32),
    (col::N_SALTBRIDGE, ColumnType::UInt32),
    (col::POCKET_RMSD, ColumnType::Float64),
    (col::GATE_PASS, ColumnType::Boolean),
];

/// The Polars schema of the pose table.
pub fn polars_schema() -> Schema {
    SCHEMA
        .iter()
        .map(|(name, ty)| Field::new((*name).into(), ty.to_polars()))
        .collect()
}

/// An empty pose table (zero rows) with every column of [`SCHEMA`].
pub fn empty() -> DataFrame {
    DataFrame::empty_with_schema(&polars_schema())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_table_has_contract_columns_and_dtypes() {
        let df = empty();
        assert_eq!(df.height(), 0);
        let names: Vec<&str> = df.get_column_names().iter().map(|s| s.as_str()).collect();
        let expected: Vec<&str> = SCHEMA.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, expected);
        for (name, ty) in SCHEMA {
            assert_eq!(df.column(name).unwrap().dtype(), &ty.to_polars(), "{name}");
        }
    }

    #[test]
    fn schema_matches_architecture_doc_order() {
        // docs/architecture.md, "Core data model".
        let csv = "pose_id,ligand_id,model_rank,plddt,iptm,affinity_pred,binder_prob,\
                   clash_count,strain_kcal,n_hbonds,n_saltbridge,pocket_rmsd,gate_pass";
        let names: Vec<&str> = SCHEMA.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.join(","), csv);
    }
}
