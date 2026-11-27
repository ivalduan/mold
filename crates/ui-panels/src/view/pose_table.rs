use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mold_mol::{PoseId, Selection};

use super::hub::{Connection, SelectionHub};
use crate::bus::{PanelId, Publication};
use crate::table::{
    CellStyle, ClickKind, Filter, FilterError, PoseTableModel, Sort, scroll_target,
};

/// The [`TableDelegate`] behind [`PoseTableView`]: GPUI Kit's virtualized table
/// asks it for columns and the cells of the visible rows only.
///
/// Row selection is ours, not the Kit's (its table selects one row; a
/// `Selection` is a set): the Kit's row selection is off, clicks publish on the
/// bus, and the highlight follows the bus.
pub struct PoseTableDelegate {
    model: PoseTableModel,
    hub: Entity<SelectionHub>,
    panel: PanelId,
    /// The bus's current selection, as last delivered.
    selection: Arc<Selection>,
    /// Shift-click anchor.
    anchor: Option<PoseId>,
}

impl std::fmt::Debug for PoseTableDelegate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoseTableDelegate")
            .field("panel", &self.panel)
            .field("rows", &self.model.row_count())
            .field("selected", &self.selection.poses.len())
            .finish_non_exhaustive()
    }
}

impl PoseTableDelegate {
    pub fn model(&self) -> &PoseTableModel {
        &self.model
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    fn is_selected(&self, row: usize) -> bool {
        self.model
            .pose_at(row)
            .is_some_and(|p| self.selection.poses.contains(&p))
    }

    fn on_row_click(&mut self, row: usize, kind: ClickKind, cx: &mut App) {
        let Some((selection, anchor)) = self.model.click(&self.selection, self.anchor, row, kind)
        else {
            return;
        };
        self.anchor = Some(anchor);
        let panel = self.panel;
        // The highlight changes when the echo comes back (one path for both
        // directions); a click on the only selected row publishes nothing.
        self.hub.update(cx, |h, cx| h.publish(panel, selection, cx));
    }
}

fn click_kind(m: &Modifiers) -> ClickKind {
    if m.shift {
        ClickKind::Extend
    } else if m.control || m.platform {
        ClickKind::Toggle
    } else {
        ClickKind::Replace
    }
}

impl TableDelegate for PoseTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.model.columns().len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.model.row_count()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let spec = &self.model.columns()[col_ix];
        let column = Column::new(spec.name, spec.title)
            .width(px(spec.width))
            .sortable();
        let column = match self.model.sort() {
            Some(s) if s.column == col_ix && s.descending => column.descending(),
            Some(s) if s.column == col_ix => column.ascending(),
            _ => column,
        };
        if spec.right_aligned() {
            column.text_right()
        } else {
            column
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.model.set_sort(match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some(Sort {
                column: col_ix,
                descending: false,
            }),
            ColumnSort::Descending => Some(Sort {
                column: col_ix,
                descending: true,
            }),
        });
        cx.notify();
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let selected = self.is_selected(row_ix);
        div()
            .id(("pose-row", row_ix))
            .when(selected, |d| d.bg(cx.theme().table_active))
            .on_click(cx.listener(move |table, e: &ClickEvent, _, cx| {
                let kind = click_kind(&e.modifiers());
                table.delegate_mut().on_row_click(row_ix, kind, cx);
            }))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let cell = self.model.cell(row_ix, col_ix);
        let theme = cx.theme();
        let color = match cell.style {
            CellStyle::Plain => theme.foreground,
            CellStyle::Null => theme.muted_foreground,
            CellStyle::Pass => theme.success,
            CellStyle::Fail => theme.danger,
        };
        div().text_color(color).child(cell.text)
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        Some(self.model.cell(row_ix, col_ix).text.into())
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.model.cell(row_ix, col_ix).text
    }
}

/// The pose table panel: the Kit's virtualized table over a [`PoseTableModel`],
/// connected to the [`SelectionHub`], plus a status line (rows shown, filter,
/// selected poses including those the filter hides).
pub struct PoseTableView {
    table: Entity<TableState<PoseTableDelegate>>,
    connection: Connection,
    /// The last delivery, for the status line.
    last: Option<Publication>,
}

impl std::fmt::Debug for PoseTableView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoseTableView")
            .field("connection", &self.connection)
            .finish_non_exhaustive()
    }
}

impl PoseTableView {
    /// A pose table panel on `hub`, registered as "pose table".
    pub fn new(
        hub: &Entity<SelectionHub>,
        model: PoseTableModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let connection = SelectionHub::connect(hub, "pose table", cx, Self::on_selection);
        let delegate = PoseTableDelegate {
            model,
            hub: hub.clone(),
            panel: connection.panel,
            selection: Arc::new(hub.read(cx).current().clone()),
            anchor: None,
        };
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .sortable(true)
                .col_movable(false)
        });
        Self {
            table,
            connection,
            last: None,
        }
    }

    /// This panel's id on the bus.
    pub fn panel(&self) -> PanelId {
        self.connection.panel
    }

    pub fn table(&self) -> &Entity<TableState<PoseTableDelegate>> {
        &self.table
    }

    /// Show only the rows `filter` keeps. Selected poses the filter hides stay
    /// selected (and are counted in the status line).
    pub fn set_filter(
        &mut self,
        filter: Filter,
        cx: &mut Context<Self>,
    ) -> Result<(), FilterError> {
        let r = self.table.update(cx, |t, cx| {
            let r = t.delegate_mut().model.set_filter(filter);
            cx.notify();
            r
        });
        cx.notify();
        r
    }

    /// Replace the data (a new pose table from the graph). Sort and filter are
    /// carried over where the new table allows; otherwise they are dropped.
    pub fn set_model(&mut self, mut model: PoseTableModel, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            let old = &t.delegate().model;
            if let Some(s) = old.sort() {
                let name = old.columns()[s.column].name;
                model.sort_by(name, s.descending);
            }
            let _ = model.set_filter(old.filter().clone());
            t.delegate_mut().model = model;
            t.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn on_selection(&mut self, p: Publication, cx: &mut Context<Self>) {
        let own = p.is_from(self.panel());
        self.table.update(cx, |t, cx| {
            t.delegate_mut().selection = p.selection.clone();
            if !own {
                let rows = t.delegate().model.map_selection(&p.selection);
                let shown = t.visible_range().rows().clone();
                if let Some(row) = scroll_target(&rows.visible, shown) {
                    t.scroll_to_row(row, cx);
                }
            }
            cx.notify();
        });
        self.last = Some(p);
        cx.notify();
    }

    fn status(&self, cx: &App) -> String {
        let d = self.table.read(cx).delegate();
        let m = &d.model;
        let mut s = format!(
            "{} of {} poses · {}",
            m.row_count(),
            m.total_rows(),
            m.filter()
        );
        let rows = m.map_selection(&d.selection);
        let n = d.selection.poses.len();
        if n > 0 {
            s += &format!(" · selected {n}");
            if !rows.hidden.is_empty() {
                s += &format!(", {} hidden by the filter", rows.hidden.len());
            }
            if !rows.absent.is_empty() {
                s += &format!(", {} not in this table", rows.absent.len());
            }
        }
        s
    }
}

impl Render for PoseTableView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status(cx);
        let muted = cx.theme().muted_foreground;
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(&self.table).stripe(true)),
            )
            .child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .text_color(muted)
                    .child(status),
            )
    }
}
