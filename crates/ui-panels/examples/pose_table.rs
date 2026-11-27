//! The pose table on a synthetic pose table, with a stand-in second panel, for
//! trying both directions of the selection bus by hand.
//!
//! ```text
//! cargo run -p mold-ui-panels --example pose_table --release
//! cargo run -p mold-ui-panels --example pose_table --release -- --rows 10000
//! ```
//!
//! Left: the pose table (GPUI Kit's virtualized table). Click a row to select a
//! pose, Ctrl-click to add or remove one, Shift-click for a range; click a header
//! to sort (ascending, descending, off). The toolbar sets filters.
//! Right: a stand-in for the plot / viewport, connected to the same bus. It shows
//! the current selection, who published it and the revision, logs every
//! delivery it receives (its own echoes marked), and has buttons that publish.

#[cfg(not(windows))]
fn main() {
    eprintln!("The panel views are built on Windows only for now (like ui-viewport, ADR-0003).");
}

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(windows)]
mod app {
    use std::collections::BTreeSet;

    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::prelude::FluentBuilder as _;
    use gpui_kit::*;
    use mold_mol::{PoseId, Selection, pose_columns as col};
    use mold_ui_panels::synthetic;
    use mold_ui_panels::table::{Filter, PoseTableModel, Predicate};
    use mold_ui_panels::view::{Connection, PoseTableView, SelectionHub};
    use mold_ui_panels::{Publication, bus::PanelId};

    fn rows_from_args() -> usize {
        let args: Vec<String> = std::env::args().collect();
        args.iter()
            .position(|a| a == "--rows")
            .and_then(|i| args.get(i + 1))
            .and_then(|n| n.parse().ok())
            .unwrap_or(300)
    }

    /// The stand-in second panel.
    struct Inspector {
        hub: Entity<SelectionHub>,
        connection: Connection,
        current: Option<Publication>,
        log: Vec<String>,
        total: u32,
        /// Poses whose gate fails: selecting them while the table shows only
        /// passing poses exercises "selected but hidden by the filter".
        gate_failures: Vec<PoseId>,
        rng: u64,
    }

    impl Inspector {
        fn new(
            hub: &Entity<SelectionHub>,
            total: u32,
            gate_failures: Vec<PoseId>,
            cx: &mut Context<Self>,
        ) -> Self {
            let connection = SelectionHub::connect(hub, "inspector", cx, |this, p, cx| {
                let me = this.connection.panel;
                let origin = this.name(p.origin, cx);
                this.log.insert(
                    0,
                    format!(
                        "rev {} from {}{}: {} poses",
                        p.revision,
                        origin,
                        if p.is_from(me) { " (own echo)" } else { "" },
                        p.selection.poses.len()
                    ),
                );
                this.log.truncate(14);
                this.current = Some(p);
                cx.notify();
            });
            Self {
                hub: hub.clone(),
                connection,
                current: None,
                log: Vec::new(),
                total,
                gate_failures,
                rng: 0x2545_f491_4f6c_dd1d,
            }
        }

        fn name(&self, panel: PanelId, cx: &App) -> String {
            self.hub
                .read(cx)
                .bus()
                .name(panel)
                .unwrap_or("?")
                .to_owned()
        }

        fn publish(&mut self, poses: impl IntoIterator<Item = u32>, cx: &mut Context<Self>) {
            let selection = Selection {
                poses: poses.into_iter().map(PoseId).collect(),
                atom_mask: None,
            };
            let me = self.connection.panel;
            let r = self.hub.update(cx, |h, cx| h.publish(me, selection, cx));
            if r.is_none() {
                self.log
                    .insert(0, "publish: unchanged, nothing sent".into());
                cx.notify();
            }
        }

        fn random(&mut self) -> u32 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng % self.total.max(1) as u64) as u32
        }
    }

    impl Render for Inspector {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.theme();
            let (muted, border) = (theme.muted_foreground, theme.border);
            let summary = match &self.current {
                None => "nothing selected yet".to_owned(),
                Some(p) => {
                    let ids: Vec<String> = p
                        .selection
                        .poses
                        .iter()
                        .take(24)
                        .map(|id| id.0.to_string())
                        .collect();
                    let more = p.selection.poses.len().saturating_sub(24);
                    format!(
                        "rev {} from {}: [{}]{}",
                        p.revision,
                        self.name(p.origin, cx),
                        ids.join(", "),
                        if more > 0 {
                            format!(" and {more} more")
                        } else {
                            String::new()
                        }
                    )
                }
            };
            let fail = self.gate_failures.first().copied();
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(10.0))
                .border_l_1()
                .border_color(border)
                .child(div().text_size(px(14.0)).child("Second panel (stand-in)"))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(muted)
                        .child("Connected to the same selection bus as the table."),
                )
                .child(div().text_size(px(13.0)).child(summary))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .child(
                            Button::new("random")
                                .small()
                                .label("3 random poses")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let ids: BTreeSet<u32> =
                                        (0..3).map(|_| this.random()).collect();
                                    this.publish(ids, cx);
                                })),
                        )
                        .child(
                            Button::new("first10")
                                .small()
                                .label("poses 0–9")
                                .on_click(cx.listener(|this, _, _, cx| this.publish(0..10, cx))),
                        )
                        .child(Button::new("last").small().label("last pose").on_click(
                            cx.listener(|this, _, _, cx| {
                                let last = this.total.saturating_sub(1);
                                this.publish([last], cx)
                            }),
                        ))
                        .when_some(fail, |d, fail| {
                            d.child(
                                Button::new("hidden")
                                    .small()
                                    .label(format!("pose {} (gate fails)", fail.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.publish([fail.0], cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("absent")
                                .small()
                                .label("pose not in table")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let id = this.total + 1000;
                                    this.publish([id], cx)
                                })),
                        )
                        .child(
                            Button::new("clear")
                                .small()
                                .label("clear")
                                .on_click(cx.listener(|this, _, _, cx| this.publish([], cx))),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(muted)
                        .child("Deliveries received (newest first):"),
                )
                .children(
                    self.log
                        .iter()
                        .map(|l| div().text_size(px(12.0)).child(l.clone())),
                )
        }
    }

    struct Demo {
        table: Entity<PoseTableView>,
        inspector: Entity<Inspector>,
        status: SharedString,
    }

    impl Demo {
        fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
            let rows = rows_from_args();
            let df = synthetic::pose_table(rows, 42);
            let model = PoseTableModel::from_frame(&df).expect("synthetic table is valid");
            let gate = model.table().column_index(col::GATE_PASS).unwrap();
            let gate_failures: Vec<PoseId> = (0..model.row_count())
                .filter(|&r| model.cell(r, gate).text == "fail")
                .filter_map(|r| model.pose_at(r))
                .collect();
            // The app owns the one hub; panels get the entity.
            let hub = cx.new(|_| SelectionHub::new());
            let table = cx.new(|cx| PoseTableView::new(&hub, model, window, cx));
            let inspector = cx.new(|cx| Inspector::new(&hub, rows as u32, gate_failures, cx));
            cx.new(|_| Demo {
                table,
                inspector,
                status: format!("{rows} synthetic poses (seed 42)").into(),
            })
        }

        fn filter(&mut self, label: &str, filter: Filter, cx: &mut Context<Self>) {
            let r = self.table.update(cx, |t, cx| t.set_filter(filter, cx));
            self.status = match r {
                Ok(()) => format!("filter: {label}").into(),
                Err(e) => format!("filter refused: {e}").into(),
            };
            cx.notify();
        }
    }

    impl Render for Demo {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.theme();
            let (bg, fg, muted, border) = (
                theme.background,
                theme.foreground,
                theme.muted_foreground,
                theme.border,
            );
            let toolbar =
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .p(px(8.0))
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_size(px(12.0)).text_color(muted).child("filter:"))
                    .child(Button::new("f-all").small().label("all rows").on_click(
                        cx.listener(|this, _, _, cx| this.filter("all rows", Filter::none(), cx)),
                    ))
                    .child(
                        Button::new("f-gate")
                            .small()
                            .label("gate pass")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let f = Filter::none().and(Predicate::is(col::GATE_PASS, true));
                                this.filter("gate pass", f, cx)
                            })),
                    )
                    .child(Button::new("f-plddt").small().label("pLDDT ≥ 70").on_click(
                        cx.listener(|this, _, _, cx| {
                            let f = Filter::none().and(Predicate::at_least(col::PLDDT, 70.0));
                            this.filter("pLDDT ≥ 70", f, cx)
                        }),
                    ))
                    .child(
                        Button::new("f-tight")
                            .small()
                            .label("gate pass, ≤ 1 clash, affinity ≤ 0")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let f = Filter::none()
                                    .and(Predicate::is(col::GATE_PASS, true))
                                    .and(Predicate::at_most(col::CLASH_COUNT, 1.0))
                                    .and(Predicate::at_most(col::AFFINITY_PRED, 0.0));
                                this.filter("tight", f, cx)
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(muted)
                            .child(self.status.clone()),
                    );
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(bg)
                .text_color(fg)
                .child(toolbar)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(div().flex_1().min_w_0().h_full().child(self.table.clone()))
                        .child(div().w(px(360.0)).h_full().child(self.inspector.clone())),
                )
        }
    }

    pub fn run() {
        application().run(|cx: &mut App| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(1400.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Mold · pose table + selection bus (WS-10)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, Demo::new) {
                eprintln!("failed to open the window: {error:#}");
                std::process::exit(2);
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    }
}
