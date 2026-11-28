//! The pose table as rows, and back.
//!
//! The table itself is a Polars `DataFrame` with exactly the columns of
//! [`mold_mol::pose_table::SCHEMA`] — the contract nodes write and panels read. Nodes
//! are easier to read row by row, so this module converts both ways. Everything a node
//! has not measured yet is `None` (a null column), never a sentinel.

use mold_mol::pose_columns as col;
use mold_mol::pose_table::{ColumnType, SCHEMA};
use polars::prelude::{Column, DataFrame, NamedFrom, Series};

/// One row of the pose table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PoseRow {
    /// Index of the pose in the [`PoseSet`](crate::PoseSet)'s arena.
    pub pose_id: u32,
    pub ligand_id: String,
    /// Which sample of that ligand this is (the worker's pose index).
    pub model_rank: u32,
    pub plddt: Option<f64>,
    pub iptm: Option<f64>,
    pub affinity_pred: Option<f64>,
    pub binder_prob: Option<f64>,
    pub clash_count: Option<u32>,
    pub strain_kcal: Option<f64>,
    pub n_hbonds: Option<u32>,
    pub n_saltbridge: Option<u32>,
    pub pocket_rmsd: Option<f64>,
    pub gate_pass: Option<bool>,
}

/// The pose table of `rows`, with every contract column in canonical order.
pub fn to_frame(rows: &[PoseRow]) -> DataFrame {
    let f64s =
        |f: fn(&PoseRow) -> Option<f64>| -> Vec<Option<f64>> { rows.iter().map(f).collect() };
    let u32s =
        |f: fn(&PoseRow) -> Option<u32>| -> Vec<Option<u32>> { rows.iter().map(f).collect() };
    let columns: Vec<Column> = vec![
        Series::new(
            col::POSE_ID.into(),
            rows.iter().map(|r| r.pose_id).collect::<Vec<u32>>(),
        )
        .into(),
        Series::new(
            col::LIGAND_ID.into(),
            rows.iter()
                .map(|r| r.ligand_id.as_str())
                .collect::<Vec<&str>>(),
        )
        .into(),
        Series::new(
            col::MODEL_RANK.into(),
            rows.iter().map(|r| r.model_rank).collect::<Vec<u32>>(),
        )
        .into(),
        Series::new(col::PLDDT.into(), f64s(|r| r.plddt)).into(),
        Series::new(col::IPTM.into(), f64s(|r| r.iptm)).into(),
        Series::new(col::AFFINITY_PRED.into(), f64s(|r| r.affinity_pred)).into(),
        Series::new(col::BINDER_PROB.into(), f64s(|r| r.binder_prob)).into(),
        Series::new(col::CLASH_COUNT.into(), u32s(|r| r.clash_count)).into(),
        Series::new(col::STRAIN_KCAL.into(), f64s(|r| r.strain_kcal)).into(),
        Series::new(col::N_HBONDS.into(), u32s(|r| r.n_hbonds)).into(),
        Series::new(col::N_SALTBRIDGE.into(), u32s(|r| r.n_saltbridge)).into(),
        Series::new(col::POCKET_RMSD.into(), f64s(|r| r.pocket_rmsd)).into(),
        Series::new(
            col::GATE_PASS.into(),
            rows.iter()
                .map(|r| r.gate_pass)
                .collect::<Vec<Option<bool>>>(),
        )
        .into(),
    ];
    DataFrame::new(rows.len(), columns).expect("every pose column has the same length")
}

/// The rows of a pose table. Errs if a contract column is missing or has another
/// dtype (a hand-built frame, or one from an older build).
pub fn from_frame(df: &DataFrame) -> Result<Vec<PoseRow>, String> {
    for (name, ty) in SCHEMA {
        let c = df
            .column(name)
            .map_err(|_| format!("the pose table has no `{name}` column"))?;
        if c.dtype() != &ty.to_polars() {
            return Err(format!(
                "the pose table's `{name}` is {:?}, the schema says {:?}",
                c.dtype(),
                ty.to_polars()
            ));
        }
    }
    let f64c = |name: &str| -> Result<Vec<Option<f64>>, String> {
        Ok(df
            .column(name)
            .and_then(|c| c.f64())
            .map_err(|e| e.to_string())?
            .iter()
            .collect())
    };
    let u32c = |name: &str| -> Result<Vec<Option<u32>>, String> {
        Ok(df
            .column(name)
            .and_then(|c| c.u32())
            .map_err(|e| e.to_string())?
            .iter()
            .collect())
    };
    let pose_id = u32c(col::POSE_ID)?;
    let model_rank = u32c(col::MODEL_RANK)?;
    let ligand_id: Vec<Option<&str>> = df
        .column(col::LIGAND_ID)
        .and_then(|c| c.str())
        .map_err(|e| e.to_string())?
        .iter()
        .collect();
    let plddt = f64c(col::PLDDT)?;
    let iptm = f64c(col::IPTM)?;
    let affinity_pred = f64c(col::AFFINITY_PRED)?;
    let binder_prob = f64c(col::BINDER_PROB)?;
    let clash_count = u32c(col::CLASH_COUNT)?;
    let strain_kcal = f64c(col::STRAIN_KCAL)?;
    let n_hbonds = u32c(col::N_HBONDS)?;
    let n_saltbridge = u32c(col::N_SALTBRIDGE)?;
    let pocket_rmsd = f64c(col::POCKET_RMSD)?;
    let gate_pass: Vec<Option<bool>> = df
        .column(col::GATE_PASS)
        .and_then(|c| c.bool())
        .map_err(|e| e.to_string())?
        .iter()
        .collect();
    (0..df.height())
        .map(|i| {
            Ok(PoseRow {
                pose_id: pose_id[i].ok_or("the pose table has a null `pose_id`")?,
                ligand_id: ligand_id[i].unwrap_or_default().to_string(),
                model_rank: model_rank[i].unwrap_or(0),
                plddt: plddt[i],
                iptm: iptm[i],
                affinity_pred: affinity_pred[i],
                binder_prob: binder_prob[i],
                clash_count: clash_count[i],
                strain_kcal: strain_kcal[i],
                n_hbonds: n_hbonds[i],
                n_saltbridge: n_saltbridge[i],
                pocket_rmsd: pocket_rmsd[i],
                gate_pass: gate_pass[i],
            })
        })
        .collect()
}

/// The dtype of `column`, as one of the four pose-table column types. `None` for a
/// dtype the schema (and therefore the codec) does not know.
pub fn column_type(dtype: &polars::prelude::DataType) -> Option<ColumnType> {
    [
        ColumnType::UInt32,
        ColumnType::Float64,
        ColumnType::Boolean,
        ColumnType::String,
    ]
    .into_iter()
    .find(|t| &t.to_polars() == dtype)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: u32) -> PoseRow {
        PoseRow {
            pose_id: i,
            ligand_id: format!("L{i}"),
            model_rank: i,
            plddt: Some(0.5 + f64::from(i)),
            gate_pass: Some(i.is_multiple_of(2)),
            ..PoseRow::default()
        }
    }

    #[test]
    fn rows_round_trip_through_the_contract_schema() {
        let rows: Vec<PoseRow> = (0..3).map(row).collect();
        let df = to_frame(&rows);
        let names: Vec<&str> = df.get_column_names().iter().map(|s| s.as_str()).collect();
        assert_eq!(names, SCHEMA.iter().map(|(n, _)| *n).collect::<Vec<_>>());
        for (name, ty) in SCHEMA {
            assert_eq!(df.column(name).unwrap().dtype(), &ty.to_polars(), "{name}");
        }
        assert_eq!(from_frame(&df).unwrap(), rows);
        assert_eq!(from_frame(&to_frame(&[])).unwrap(), vec![]);
    }

    #[test]
    fn a_table_without_the_contract_columns_is_refused() {
        let df = DataFrame::new(1, vec![Series::new("x".into(), [1u32]).into()]).unwrap();
        assert!(from_frame(&df).unwrap_err().contains("pose_id"));
    }
}
