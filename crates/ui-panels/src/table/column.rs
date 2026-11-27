//! How each pose-table column is shown: title, width, alignment, number format.

use mold_mol::pose_columns as col;
use mold_mol::pose_table::ColumnType;

/// How a cell's value turns into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellFormat {
    /// Unsigned integer as is.
    Integer,
    /// Float with this many decimals.
    Fixed(u8),
    /// String as is.
    Text,
    /// Boolean as "pass" / "fail" (`gate_pass`).
    PassFail,
    /// Boolean as "yes" / "no".
    YesNo,
}

/// Display spec of one pose-table column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnSpec {
    /// Column name in the pose table (`mold_mol::pose_columns`).
    pub name: &'static str,
    /// Header text.
    pub title: &'static str,
    pub ty: ColumnType,
    pub format: CellFormat,
    /// Suggested width in px.
    pub width: f32,
}

impl ColumnSpec {
    /// Numbers are right-aligned, text and pass/fail left-aligned.
    pub fn right_aligned(&self) -> bool {
        matches!(self.format, CellFormat::Integer | CellFormat::Fixed(_))
    }

    /// The spec of the contract column `name`.
    pub fn for_name(name: &str) -> Option<ColumnSpec> {
        COLUMNS.iter().find(|c| c.name == name).copied()
    }
}

/// The pose-table columns in `mold_mol::pose_table::SCHEMA` order, with how
/// each is shown. A unit test keeps names and types in step with `SCHEMA`.
pub const COLUMNS: [ColumnSpec; 13] = [
    spec(
        col::POSE_ID,
        "pose",
        ColumnType::UInt32,
        CellFormat::Integer,
        70.0,
    ),
    spec(
        col::LIGAND_ID,
        "ligand",
        ColumnType::String,
        CellFormat::Text,
        110.0,
    ),
    spec(
        col::MODEL_RANK,
        "rank",
        ColumnType::UInt32,
        CellFormat::Integer,
        60.0,
    ),
    spec(
        col::PLDDT,
        "pLDDT",
        ColumnType::Float64,
        CellFormat::Fixed(1),
        70.0,
    ),
    spec(
        col::IPTM,
        "ipTM",
        ColumnType::Float64,
        CellFormat::Fixed(3),
        70.0,
    ),
    spec(
        col::AFFINITY_PRED,
        "affinity",
        ColumnType::Float64,
        CellFormat::Fixed(2),
        80.0,
    ),
    spec(
        col::BINDER_PROB,
        "P(binder)",
        ColumnType::Float64,
        CellFormat::Fixed(3),
        85.0,
    ),
    spec(
        col::CLASH_COUNT,
        "clashes",
        ColumnType::UInt32,
        CellFormat::Integer,
        70.0,
    ),
    spec(
        col::STRAIN_KCAL,
        "strain kcal/mol",
        ColumnType::Float64,
        CellFormat::Fixed(1),
        115.0,
    ),
    spec(
        col::N_HBONDS,
        "H-bonds",
        ColumnType::UInt32,
        CellFormat::Integer,
        75.0,
    ),
    spec(
        col::N_SALTBRIDGE,
        "salt br.",
        ColumnType::UInt32,
        CellFormat::Integer,
        70.0,
    ),
    spec(
        col::POCKET_RMSD,
        "pocket RMSD Å",
        ColumnType::Float64,
        CellFormat::Fixed(2),
        110.0,
    ),
    spec(
        col::GATE_PASS,
        "gate",
        ColumnType::Boolean,
        CellFormat::PassFail,
        60.0,
    ),
];

const fn spec(
    name: &'static str,
    title: &'static str,
    ty: ColumnType,
    format: CellFormat,
    width: f32,
) -> ColumnSpec {
    ColumnSpec {
        name,
        title,
        ty,
        format,
        width,
    }
}

/// What a cell shows, and how the view should style it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub style: CellStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStyle {
    Plain,
    /// Missing value (null, or NaN).
    Null,
    Pass,
    Fail,
}

/// Text shown for a missing value.
pub const NULL_TEXT: &str = "–";

impl Cell {
    pub(crate) fn null() -> Self {
        Cell {
            text: NULL_TEXT.to_owned(),
            style: CellStyle::Null,
        }
    }

    pub(crate) fn plain(text: String) -> Self {
        Cell {
            text,
            style: CellStyle::Plain,
        }
    }

    pub(crate) fn uint(v: Option<u32>) -> Self {
        v.map_or_else(Cell::null, |v| Cell::plain(v.to_string()))
    }

    pub(crate) fn float(v: Option<f64>, format: CellFormat) -> Self {
        match v {
            Some(v) if !v.is_nan() => {
                let decimals = match format {
                    CellFormat::Fixed(d) => d as usize,
                    _ => 3,
                };
                Cell::plain(format!("{v:.decimals$}"))
            }
            _ => Cell::null(),
        }
    }

    pub(crate) fn boolean(v: Option<bool>, format: CellFormat) -> Self {
        match (v, format) {
            (None, _) => Cell::null(),
            (Some(true), CellFormat::PassFail) => Cell {
                text: "pass".into(),
                style: CellStyle::Pass,
            },
            (Some(false), CellFormat::PassFail) => Cell {
                text: "fail".into(),
                style: CellStyle::Fail,
            },
            (Some(b), _) => Cell::plain(if b { "yes" } else { "no" }.into()),
        }
    }

    pub(crate) fn text(v: Option<&str>) -> Self {
        v.map_or_else(Cell::null, |s| Cell::plain(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mold_mol::pose_table::SCHEMA;

    #[test]
    fn display_specs_follow_the_schema() {
        assert_eq!(COLUMNS.len(), SCHEMA.len());
        for (spec, (name, ty)) in COLUMNS.iter().zip(SCHEMA) {
            assert_eq!((spec.name, spec.ty), (name, ty));
            let ok = matches!(
                (spec.ty, spec.format),
                (ColumnType::UInt32, CellFormat::Integer)
                    | (ColumnType::Float64, CellFormat::Fixed(_))
                    | (
                        ColumnType::Boolean,
                        CellFormat::PassFail | CellFormat::YesNo
                    )
                    | (ColumnType::String, CellFormat::Text)
            );
            assert!(ok, "{name}: format {:?} for {:?}", spec.format, spec.ty);
        }
    }

    #[test]
    fn cells_format_per_column_type() {
        let plddt = ColumnSpec::for_name(col::PLDDT).unwrap();
        assert_eq!(Cell::float(Some(87.456), plddt.format).text, "87.5");
        let iptm = ColumnSpec::for_name(col::IPTM).unwrap();
        assert_eq!(Cell::float(Some(0.81234), iptm.format).text, "0.812");
        assert_eq!(Cell::float(None, iptm.format), Cell::null());
        assert_eq!(
            Cell::float(Some(f64::NAN), iptm.format).style,
            CellStyle::Null
        );
        assert_eq!(Cell::uint(Some(3)).text, "3");
        assert_eq!(Cell::uint(None).text, NULL_TEXT);

        let gate = ColumnSpec::for_name(col::GATE_PASS).unwrap();
        assert_eq!(gate.format, CellFormat::PassFail);
        let pass = Cell::boolean(Some(true), gate.format);
        assert_eq!((pass.text.as_str(), pass.style), ("pass", CellStyle::Pass));
        let fail = Cell::boolean(Some(false), gate.format);
        assert_eq!((fail.text.as_str(), fail.style), ("fail", CellStyle::Fail));
        assert_eq!(Cell::boolean(None, gate.format).style, CellStyle::Null);
        assert_eq!(Cell::text(Some("LIG-0007")).text, "LIG-0007");

        assert!(plddt.right_aligned());
        assert!(!gate.right_aligned());
        assert_eq!(ColumnSpec::for_name("vina_score"), None);
    }
}
