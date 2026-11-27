//! The pose table's view model, GPUI-free: a typed snapshot of the pose-table
//! columns, a stable sort and a filter over it, row ⇄ [`PoseId`] mapping, the
//! selection ⇄ rows mapping and cell formatting.
//!
//! Sort and filter read the pose table only, never coordinates or the arena. The
//! model never reorders the `DataFrame`: it keeps a permutation of row indices,
//! so a sort or filter change costs one pass over one or two columns.

mod column;
mod filter;

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::ops::Range;

use mold_mol::pose_table::{ColumnType, SCHEMA};
use mold_mol::{PoseId, Selection, pose_columns};
use polars::prelude::{DataFrame, PolarsError};

pub use column::{COLUMNS, Cell, CellFormat, CellStyle, ColumnSpec, NULL_TEXT};
pub use filter::{Filter, FilterError, Predicate};

/// One column's values, by type. `None` = null.
#[derive(Debug, Clone)]
enum Data {
    UInt32(Vec<Option<u32>>),
    Float64(Vec<Option<f64>>),
    Boolean(Vec<Option<bool>>),
    String(Vec<Option<String>>),
}

/// Why a `DataFrame` could not be read as a pose table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableError {
    /// `pose_id` is the join key; a table without it cannot be linked.
    MissingPoseId,
    NullPoseId {
        row: usize,
    },
    DuplicatePoseId(PoseId),
    WrongType {
        column: &'static str,
        expected: ColumnType,
        found: String,
    },
    Polars(String),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::MissingPoseId => write!(f, "the table has no `pose_id` column"),
            TableError::NullPoseId { row } => write!(f, "row {row} has no pose_id"),
            TableError::DuplicatePoseId(p) => write!(f, "pose_id {} appears twice", p.0),
            TableError::WrongType {
                column,
                expected,
                found,
            } => write!(f, "column `{column}` is {found}, expected {expected:?}"),
            TableError::Polars(e) => write!(f, "polars: {e}"),
        }
    }
}

impl std::error::Error for TableError {}

impl From<PolarsError> for TableError {
    fn from(e: PolarsError) -> Self {
        TableError::Polars(e.to_string())
    }
}

/// A typed snapshot of the pose-table columns the panels show.
///
/// Holds the `SCHEMA` columns present in the frame, in `SCHEMA` order. A missing
/// contract column (e.g. no gate has run yet) is simply not shown; columns
/// outside the contract are ignored. `pose_id` is required and unique.
#[derive(Debug, Clone)]
pub struct PoseTable {
    pose_ids: Vec<PoseId>,
    specs: Vec<ColumnSpec>,
    data: Vec<Data>,
    index: HashMap<PoseId, u32>,
}

impl PoseTable {
    /// Read the pose table. One pass per column; no coordinates involved.
    pub fn from_frame(df: &DataFrame) -> Result<Self, TableError> {
        let mut specs = Vec::new();
        let mut data = Vec::new();
        let mut pose_ids = None;
        for (spec, (name, ty)) in COLUMNS.iter().zip(SCHEMA) {
            debug_assert_eq!(spec.name, name);
            let Ok(column) = df.column(name) else {
                continue;
            };
            if column.dtype() != &ty.to_polars() {
                return Err(TableError::WrongType {
                    column: name,
                    expected: ty,
                    found: column.dtype().to_string(),
                });
            }
            let values = match ty {
                ColumnType::UInt32 => Data::UInt32(column.u32()?.iter().collect()),
                ColumnType::Float64 => Data::Float64(column.f64()?.iter().collect()),
                ColumnType::Boolean => Data::Boolean(column.bool()?.iter().collect()),
                ColumnType::String => {
                    Data::String(column.str()?.iter().map(|s| s.map(str::to_owned)).collect())
                }
            };
            if name == pose_columns::POSE_ID {
                let Data::UInt32(ids) = &values else {
                    unreachable!("pose_id is UInt32 in SCHEMA")
                };
                let ids: Vec<PoseId> = ids
                    .iter()
                    .enumerate()
                    .map(|(row, id)| id.map(PoseId).ok_or(TableError::NullPoseId { row }))
                    .collect::<Result<_, _>>()?;
                pose_ids = Some(ids);
            }
            specs.push(*spec);
            data.push(values);
        }
        let pose_ids = pose_ids.ok_or(TableError::MissingPoseId)?;
        let mut index = HashMap::with_capacity(pose_ids.len());
        for (row, &id) in pose_ids.iter().enumerate() {
            if index.insert(id, row as u32).is_some() {
                return Err(TableError::DuplicatePoseId(id));
            }
        }
        Ok(Self {
            pose_ids,
            specs,
            data,
            index,
        })
    }

    /// Number of poses (rows).
    pub fn len(&self) -> usize {
        self.pose_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pose_ids.is_empty()
    }

    /// The columns shown, in `SCHEMA` order.
    pub fn columns(&self) -> &[ColumnSpec] {
        &self.specs
    }

    /// Index into [`Self::columns`] of the column called `name`.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.specs.iter().position(|s| s.name == name)
    }

    /// The pose in table row `row` (the frame's own order).
    pub fn pose_id(&self, row: usize) -> PoseId {
        self.pose_ids[row]
    }

    /// The table row holding `pose`, if any.
    pub fn row_of(&self, pose: PoseId) -> Option<usize> {
        self.index.get(&pose).map(|&r| r as usize)
    }

    /// The formatted value at table row `row`, column `col`.
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        let format = self.specs[col].format;
        match &self.data[col] {
            Data::UInt32(v) => Cell::uint(v[row]),
            Data::Float64(v) => Cell::float(v[row], format),
            Data::Boolean(v) => Cell::boolean(v[row], format),
            Data::String(v) => Cell::text(v[row].as_deref()),
        }
    }

    /// Table rows in sorted order. Stable: ties keep table order. Missing
    /// values (null, NaN) sort last in both directions.
    fn sorted(&self, sort: Sort) -> Vec<u32> {
        let mut order: Vec<u32> = (0..self.len() as u32).collect();
        let desc = sort.descending;
        match &self.data[sort.column] {
            Data::UInt32(v) => order.sort_by(|&a, &b| {
                cmp_missing_last(v[a as usize], v[b as usize], desc, |x, y| x.cmp(y))
            }),
            Data::Float64(v) => {
                let key = |i: u32| v[i as usize].filter(|x| !x.is_nan());
                order.sort_by(|&a, &b| cmp_missing_last(key(a), key(b), desc, f64::total_cmp))
            }
            Data::Boolean(v) => order.sort_by(|&a, &b| {
                cmp_missing_last(v[a as usize], v[b as usize], desc, |x, y| x.cmp(y))
            }),
            Data::String(v) => order.sort_by(|&a, &b| {
                cmp_missing_last(
                    v[a as usize].as_deref(),
                    v[b as usize].as_deref(),
                    desc,
                    |x, y| x.cmp(y),
                )
            }),
        }
        order
    }

    /// One row predicate per filter predicate, all of which must hold.
    fn compile(&self, filter: &Filter) -> Result<Vec<RowTest<'_>>, FilterError> {
        filter
            .predicates
            .iter()
            .map(|p| {
                let col = self
                    .column_index(p.column())
                    .ok_or_else(|| FilterError::UnknownColumn(p.column().to_owned()))?;
                let ty = self.specs[col].ty;
                if !p.accepts(ty) {
                    return Err(FilterError::WrongType {
                        column: p.column().to_owned(),
                        ty,
                    });
                }
                Ok(match (p, &self.data[col]) {
                    (Predicate::Is { value, .. }, Data::Boolean(v)) => RowTest::Is(v, *value),
                    (Predicate::Range { min, max, .. }, Data::Float64(v)) => RowTest::RangeF(
                        v,
                        min.unwrap_or(f64::NEG_INFINITY),
                        max.unwrap_or(f64::INFINITY),
                    ),
                    (Predicate::Range { min, max, .. }, Data::UInt32(v)) => RowTest::RangeU(
                        v,
                        min.unwrap_or(f64::NEG_INFINITY),
                        max.unwrap_or(f64::INFINITY),
                    ),
                    (Predicate::Contains { text, .. }, Data::String(v)) => {
                        RowTest::Contains(v, text.to_ascii_lowercase())
                    }
                    _ => unreachable!("accepts() checked the type"),
                })
            })
            .collect()
    }
}

fn cmp_missing_last<T>(
    a: Option<T>,
    b: Option<T>,
    descending: bool,
    cmp: impl Fn(&T, &T) -> Ordering,
) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) if descending => cmp(&y, &x),
        (Some(x), Some(y)) => cmp(&x, &y),
    }
}

enum RowTest<'a> {
    Is(&'a [Option<bool>], bool),
    RangeF(&'a [Option<f64>], f64, f64),
    RangeU(&'a [Option<u32>], f64, f64),
    Contains(&'a [Option<String>], String),
}

impl RowTest<'_> {
    fn keeps(&self, row: usize) -> bool {
        match self {
            RowTest::Is(v, want) => v[row] == Some(*want),
            // NaN fails both comparisons, so a NaN never passes.
            RowTest::RangeF(v, lo, hi) => v[row].is_some_and(|x| *lo <= x && x <= *hi),
            RowTest::RangeU(v, lo, hi) => {
                v[row].is_some_and(|x| *lo <= x as f64 && x as f64 <= *hi)
            }
            RowTest::Contains(v, needle) => v[row]
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(needle.as_str())),
        }
    }
}

/// Sort key: a column (index into [`PoseTableModel::columns`]) and direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

/// Where a pose is in the table view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowLookup {
    /// At this display row.
    Visible(usize),
    /// In the pose table, but filtered out.
    Hidden,
    /// Not in this pose table at all.
    Absent,
}

/// A `Selection` resolved against the table view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionRows {
    /// Display rows of the selected poses that pass the filter, ascending.
    pub visible: Vec<usize>,
    /// Selected poses in the table that the filter hides.
    pub hidden: Vec<PoseId>,
    /// Selected poses this table does not have.
    pub absent: Vec<PoseId>,
}

/// What a row click does with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKind {
    /// Plain click: select just this pose.
    Replace,
    /// Ctrl-click: add or remove this pose, keep the rest (hidden ones too).
    Toggle,
    /// Shift-click: select the display rows from the anchor to this one.
    Extend,
}

/// The table view model: a [`PoseTable`] seen through a sort and a filter.
///
/// "Display row" = position in the sorted, filtered view (what the table widget
/// calls a row index). "Table row" = position in the `DataFrame`.
#[derive(Debug, Clone)]
pub struct PoseTableModel {
    table: PoseTable,
    sort: Option<Sort>,
    filter: Filter,
    /// All table rows in sort order.
    order: Vec<u32>,
    /// Display row → table row: `order` restricted to rows passing the filter.
    rows: Vec<u32>,
    /// Table row → display row; `HIDDEN` if filtered out.
    position: Vec<u32>,
}

const HIDDEN: u32 = u32::MAX;

impl PoseTableModel {
    /// Unsorted, unfiltered view of `table`.
    pub fn new(table: PoseTable) -> Self {
        let order: Vec<u32> = (0..table.len() as u32).collect();
        let (rows, position) = Self::apply(&order, table.len(), &[]);
        Self {
            table,
            sort: None,
            filter: Filter::none(),
            order,
            rows,
            position,
        }
    }

    pub fn from_frame(df: &DataFrame) -> Result<Self, TableError> {
        PoseTable::from_frame(df).map(Self::new)
    }

    pub fn table(&self) -> &PoseTable {
        &self.table
    }

    pub fn columns(&self) -> &[ColumnSpec] {
        self.table.columns()
    }

    /// Number of display rows (after the filter).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Number of poses in the table, filtered or not.
    pub fn total_rows(&self) -> usize {
        self.table.len()
    }

    pub fn sort(&self) -> Option<Sort> {
        self.sort
    }

    /// Sort by a column, or restore table order with `None`. Out-of-range
    /// columns are ignored (table order).
    pub fn set_sort(&mut self, sort: Option<Sort>) {
        let sort = sort.filter(|s| s.column < self.table.columns().len());
        self.sort = sort;
        self.order = match sort {
            Some(s) => self.table.sorted(s),
            None => (0..self.table.len() as u32).collect(),
        };
        let tests = self.table.compile(&self.filter).expect("validated on set");
        let (rows, position) = Self::apply(&self.order, self.table.len(), &tests);
        self.rows = rows;
        self.position = position;
    }

    /// Sort by the column called `name`.
    pub fn sort_by(&mut self, name: &str, descending: bool) {
        let sort = self
            .table
            .column_index(name)
            .map(|column| Sort { column, descending });
        self.set_sort(sort);
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    /// Show only the rows `filter` keeps. A filter naming an unknown column or
    /// the wrong type is refused and the current one stays.
    pub fn set_filter(&mut self, filter: Filter) -> Result<(), FilterError> {
        let tests = self.table.compile(&filter)?;
        let (rows, position) = Self::apply(&self.order, self.table.len(), &tests);
        self.filter = filter;
        self.rows = rows;
        self.position = position;
        Ok(())
    }

    fn apply(order: &[u32], len: usize, tests: &[RowTest<'_>]) -> (Vec<u32>, Vec<u32>) {
        let mut position = vec![HIDDEN; len];
        let mut rows = Vec::with_capacity(len);
        for &r in order {
            if tests.iter().all(|t| t.keeps(r as usize)) {
                position[r as usize] = rows.len() as u32;
                rows.push(r);
            }
        }
        (rows, position)
    }

    /// The pose shown at display row `row`.
    pub fn pose_at(&self, row: usize) -> Option<PoseId> {
        self.rows.get(row).map(|&r| self.table.pose_id(r as usize))
    }

    /// Where `pose` is shown.
    pub fn row_of(&self, pose: PoseId) -> RowLookup {
        match self.table.row_of(pose) {
            None => RowLookup::Absent,
            Some(r) => match self.position[r] {
                HIDDEN => RowLookup::Hidden,
                p => RowLookup::Visible(p as usize),
            },
        }
    }

    /// Resolve a selection against the current view. The atom mask is ignored:
    /// the table works on whole poses.
    pub fn map_selection(&self, selection: &Selection) -> SelectionRows {
        let mut out = SelectionRows::default();
        for &pose in &selection.poses {
            match self.row_of(pose) {
                RowLookup::Visible(r) => out.visible.push(r),
                RowLookup::Hidden => out.hidden.push(pose),
                RowLookup::Absent => out.absent.push(pose),
            }
        }
        out.visible.sort_unstable();
        out
    }

    /// The selection of the poses at these display rows (out-of-range rows are
    /// skipped).
    pub fn selection_of_rows(&self, rows: impl IntoIterator<Item = usize>) -> Selection {
        Selection {
            poses: rows.into_iter().filter_map(|r| self.pose_at(r)).collect(),
            atom_mask: None,
        }
    }

    /// The selection after clicking display row `row`, and the new anchor for
    /// shift-clicks. `None` if `row` is out of range.
    pub fn click(
        &self,
        current: &Selection,
        anchor: Option<PoseId>,
        row: usize,
        kind: ClickKind,
    ) -> Option<(Selection, PoseId)> {
        let pose = self.pose_at(row)?;
        let anchor_row = anchor.and_then(|a| match self.row_of(a) {
            RowLookup::Visible(r) => Some((a, r)),
            _ => None,
        });
        Some(match (kind, anchor_row) {
            (ClickKind::Toggle, _) => {
                let mut poses: BTreeSet<PoseId> = current.poses.clone();
                if !poses.remove(&pose) {
                    poses.insert(pose);
                }
                (
                    Selection {
                        poses,
                        atom_mask: None,
                    },
                    pose,
                )
            }
            (ClickKind::Extend, Some((a, ar))) => {
                (self.selection_of_rows(ar.min(row)..=ar.max(row)), a)
            }
            (ClickKind::Replace | ClickKind::Extend, _) => (Selection::single(pose), pose),
        })
    }

    /// The formatted cell at display row `row`, column `col`.
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        self.table.cell(self.rows[row] as usize, col)
    }

    /// Formatted cells of the display rows in `rows` (clamped to the view): what
    /// a virtualized widget paints for its visible window.
    pub fn window(&self, rows: Range<usize>) -> Vec<Vec<Cell>> {
        let end = rows.end.min(self.row_count());
        let start = rows.start.min(end);
        let cols = self.columns().len();
        (start..end)
            .map(|r| (0..cols).map(|c| self.cell(r, c)).collect())
            .collect()
    }
}

/// The display rows a virtualized list must build for a viewport `viewport_height`
/// px tall scrolled to `scroll_top` px, with uniform `row_height`, plus `overscan`
/// rows on each side.
pub fn visible_rows(
    scroll_top: f32,
    viewport_height: f32,
    row_height: f32,
    row_count: usize,
    overscan: usize,
) -> Range<usize> {
    if row_count == 0 || row_height <= 0.0 || viewport_height <= 0.0 {
        return 0..0;
    }
    let first = (scroll_top.max(0.0) / row_height).floor() as usize;
    let last = ((scroll_top.max(0.0) + viewport_height) / row_height).ceil() as usize;
    let start = first.saturating_sub(overscan).min(row_count);
    let end = (last + overscan).min(row_count);
    start..end
}

/// Where to scroll for an incoming selection: `None` if a selected row is
/// already in `shown` (or nothing selected is visible), else the first selected
/// display row.
pub fn scroll_target(selected_rows: &[usize], shown: Range<usize>) -> Option<usize> {
    if selected_rows.iter().any(|r| shown.contains(r)) {
        None
    } else {
        selected_rows.iter().copied().min()
    }
}
