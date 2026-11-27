//! Row filters over pose-table columns. A missing value (null, NaN) fails every
//! predicate.

use std::fmt;

use mold_mol::pose_table::ColumnType;

/// One condition on one column.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// A boolean column equals `value` (e.g. `gate_pass == true`).
    Is { column: String, value: bool },
    /// A numeric column (UInt32 or Float64) lies in `[min, max]`; either bound
    /// may be open.
    Range {
        column: String,
        min: Option<f64>,
        max: Option<f64>,
    },
    /// A string column contains `text`, ignoring ASCII case.
    Contains { column: String, text: String },
}

impl Predicate {
    pub fn is(column: &str, value: bool) -> Self {
        Predicate::Is {
            column: column.into(),
            value,
        }
    }

    pub fn at_least(column: &str, min: f64) -> Self {
        Self::between(column, Some(min), None)
    }

    pub fn at_most(column: &str, max: f64) -> Self {
        Self::between(column, None, Some(max))
    }

    pub fn between(column: &str, min: Option<f64>, max: Option<f64>) -> Self {
        Predicate::Range {
            column: column.into(),
            min,
            max,
        }
    }

    pub fn contains(column: &str, text: &str) -> Self {
        Predicate::Contains {
            column: column.into(),
            text: text.into(),
        }
    }

    pub fn column(&self) -> &str {
        match self {
            Predicate::Is { column, .. }
            | Predicate::Range { column, .. }
            | Predicate::Contains { column, .. } => column,
        }
    }

    /// Whether the predicate can apply to a column of type `ty`.
    pub fn accepts(&self, ty: ColumnType) -> bool {
        match self {
            Predicate::Is { .. } => ty == ColumnType::Boolean,
            Predicate::Range { .. } => matches!(ty, ColumnType::UInt32 | ColumnType::Float64),
            Predicate::Contains { .. } => ty == ColumnType::String,
        }
    }
}

impl fmt::Display for Predicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Predicate::Is { column, value } => write!(f, "{column} = {value}"),
            Predicate::Range {
                column,
                min: Some(lo),
                max: Some(hi),
            } => write!(f, "{lo} ≤ {column} ≤ {hi}"),
            Predicate::Range {
                column,
                min: Some(lo),
                max: None,
            } => write!(f, "{column} ≥ {lo}"),
            Predicate::Range {
                column,
                min: None,
                max: Some(hi),
            } => write!(f, "{column} ≤ {hi}"),
            Predicate::Range {
                column,
                min: None,
                max: None,
            } => write!(f, "{column} present"),
            Predicate::Contains { column, text } => write!(f, "{column} contains \"{text}\""),
        }
    }
}

/// All predicates must hold (AND). The empty filter keeps every row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub predicates: Vec<Predicate>,
}

impl Filter {
    /// The filter that keeps every row.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn and(mut self, p: Predicate) -> Self {
        self.predicates.push(p);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.predicates.is_empty()
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.predicates.is_empty() {
            return f.write_str("all rows");
        }
        for (i, p) in self.predicates.iter().enumerate() {
            if i > 0 {
                f.write_str(" and ")?;
            }
            write!(f, "{p}")?;
        }
        Ok(())
    }
}

/// Why a filter was refused. The model keeps its previous filter then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
    UnknownColumn(String),
    WrongType { column: String, ty: ColumnType },
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterError::UnknownColumn(c) => write!(f, "no column `{c}` in the pose table"),
            FilterError::WrongType { column, ty } => {
                write!(f, "predicate does not apply to `{column}` ({ty:?})")
            }
        }
    }
}

impl std::error::Error for FilterError {}
