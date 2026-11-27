//! The molecular viewport (ADR-0003) with `mold-render` behind it: a real
//! structure, spheres / ball & stick / sticks, GPU picking, re-aim on click.
//! Not the app shell.
//!
//! ```text
//! cargo run -p mold-ui-viewport --example viewport --release
//! cargo run -p mold-ui-viewport --example viewport --release -- path/to/a.cif other.pdb
//! cargo run -p mold-ui-viewport --example viewport --release -- --self-test
//! ```
//!
//! With no paths it loads `fixtures/egfr/reference.cif` (EGFR + gefitinib)
//! twice, the second copy shifted 70 Å along x, so there are two poses.
//! Left-click an atom to pick it (pose + atom printed) and re-aim; left-drag
//! orbits, middle-drag pans, the wheel zooms; right-click opens a menu.
//!
//! `--self-test` drives the same paths without a mouse and exits non-zero if
//! a check fails: overlay menu over the viewport, representations, UI-thread
//! stalls while the render thread is slow, ten synthetic clicks, hide/show.

#[cfg(not(windows))]
fn main() {
    eprintln!("The viewport has a Windows backend only (ADR-0003).");
}

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(windows)]
mod app {
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    use gpui_kit::*;
    use mold_mol::{Arena, PoseId};
    use mold_render::host::{Event, Msg, PickReport, RenderHost, Stats};
    use mold_render::{Representation, guess_bonds};
    use mold_ui_viewport::backend::{native_backend, wgpu_backends};
    use mold_ui_viewport::element::{Viewport, ViewportElement};

    const BUDGET_MS: f32 = 100.0;

    pub fn run() {
        let self_test = std::env::args().any(|arg| arg == "--self-test");
        let paths: Vec<PathBuf> = std::env::args()
            .skip(1)
            .filter(|arg| !arg.starts_with("--"))
            .map(PathBuf::from)
            .collect();
        let arena = match load(&paths) {
            Ok(arena) => Arc::new(arena),
            Err(error) => {
                eprintln!("loading structures: {error}");
                std::process::exit(2);
            }
        };
        application().run(move |cx: &mut App| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Mold · viewport (mold-render)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                Demo::new(arena, self_test, window, cx)
            });
            if let Err(error) = opened {
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

    /// The given files, one pose per model; or the EGFR fixture twice, the
    /// second copy moved aside.
    fn load(paths: &[PathBuf]) -> Result<Arena, String> {
        let mut arena = Arena::new();
        let options = Default::default();
        if paths.is_empty() {
            let fixture =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/egfr/reference.cif");
            for _ in 0..2 {
                mold_mol::io::read_file(&mut arena, &fixture, &options)
                    .map_err(|e| format!("{}: {e}", fixture.display()))?;
            }
            if let Some(second) = arena.pose_positions_mut(PoseId(1)) {
                for p in second {
                    p[0] += 70.0;
                }
            }
        } else {
            for path in paths {
                mold_mol::io::read_file(&mut arena, path, &options)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        if arena.atom_count() == 0 {
            return Err("no atoms".into());
        }
        Ok(arena)
    }

    struct Demo {
        // Dropped before `host`: the render thread outlives the surface.
        viewport: Option<Rc<Viewport>>,
        host: Rc<RenderHost>,
        arena: Arc<Arena>,
        bonds: usize,
        representation: Representation,
        /// `None`: all poses.
        shown_pose: Option<PoseId>,
        failure: Option<String>,
        stats: Stats,
        last_pick: Option<PickReport>,
        latencies_ms: Vec<f32>,
        context_menu: Option<Point<Pixels>>,
        worst_ui_stall_ms: f32,
        test: SelfTest,
    }

    #[derive(Default)]
    struct SelfTest {
        enabled: bool,
        failures: Vec<String>,
        frames_when_hidden: Option<u64>,
    }

    impl Demo {
        fn new(
            arena: Arc<Arena>,
            self_test: bool,
            window: &mut Window,
            cx: &mut App,
        ) -> Entity<Self> {
            let (events_tx, events) = mpsc::channel();
            let host = match RenderHost::new(wgpu_backends(), events_tx) {
                Ok(host) => Rc::new(host),
                Err(error) => {
                    eprintln!("viewport: {error}");
                    std::process::exit(2);
                }
            };
            println!("viewport adapter: {}", host.adapter);
            let started = Instant::now();
            let bonds = guess_bonds(&arena);
            println!(
                "{} atoms in {} poses, {} bonds guessed in {:.0} ms",
                arena.atom_count(),
                arena.pose_count(),
                bonds.len(),
                started.elapsed().as_secs_f32() * 1000.0
            );
            let bond_count = bonds.len();
            host.send(Msg::LoadScene {
                arena: arena.clone(),
                bonds,
            });
            let backend = native_backend().expect("this platform has a viewport backend");
            let (viewport, failure) = match Viewport::new(window, backend.as_ref(), &host) {
                Ok(viewport) => (Some(viewport), None),
                Err(error) => {
                    // Loud, and in the window too.
                    eprintln!("viewport: {error}");
                    (None, Some(error))
                }
            };
            cx.new(|cx| {
                pump_events(events, cx);
                measure_ui_stalls(cx);
                if self_test {
                    run_self_test(cx);
                }
                Self {
                    viewport,
                    host,
                    arena,
                    bonds: bond_count,
                    representation: Representation::default(),
                    shown_pose: None,
                    failure,
                    stats: Stats::default(),
                    last_pick: None,
                    latencies_ms: Vec::new(),
                    context_menu: None,
                    worst_ui_stall_ms: 0.0,
                    test: SelfTest {
                        enabled: self_test,
                        ..Default::default()
                    },
                }
            })
        }

        fn send(&self, msg: Msg) {
            self.host.send(msg);
        }

        fn visible(&self) -> bool {
            self.viewport.as_ref().is_some_and(|v| v.is_visible())
        }

        fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
            if let Some(viewport) = &self.viewport {
                viewport.set_visible(visible);
            }
            if !visible {
                self.context_menu = None;
            }
            println!("viewport {}", if visible { "shown" } else { "hidden" });
            cx.notify();
        }

        fn set_representation(&mut self, representation: Representation, cx: &mut Context<Self>) {
            self.representation = representation;
            self.send(Msg::SetRepresentation(representation));
            cx.notify();
        }

        /// All poses → pose 0 → pose 1 → … → all.
        fn next_pose(&mut self, cx: &mut Context<Self>) {
            let count = self.arena.pose_count() as u32;
            self.shown_pose = match self.shown_pose {
                None if count > 1 => Some(PoseId(0)),
                Some(PoseId(i)) if i + 1 < count => Some(PoseId(i + 1)),
                _ => None,
            };
            self.send(Msg::ShowOnly(self.shown_pose));
            cx.notify();
        }

        /// "pose 1 · atom #2400 · CA LEU 718 A".
        fn describe(&self, report: &PickReport) -> Option<String> {
            let hit = report.hit?;
            let arena = &self.arena;
            let atom = hit.atom as usize;
            let residue = &arena.residues()[arena.residue_indices()[atom] as usize];
            let chain = &arena.chains()[arena.chain_indices()[atom] as usize];
            Some(format!(
                "pose {} · atom #{atom} · {} {} {} {}",
                hit.pose.0,
                arena.atom_names()[atom],
                residue.name,
                residue.seq_num,
                chain.id
            ))
        }

        fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
            match event {
                Event::Ready => println!("viewport: first frame presented"),
                Event::CommitComposition => {
                    if let Some(viewport) = &self.viewport {
                        viewport.commit();
                    }
                    return;
                }
                Event::Pressed => {
                    if self.context_menu.take().is_none() {
                        return;
                    }
                }
                Event::ContextMenu { window_x, window_y } => {
                    self.context_menu = Some(point(px(window_x), px(window_y)));
                }
                Event::SceneLoaded {
                    atoms,
                    bonds,
                    poses,
                } => println!("scene on the GPU: {atoms} atoms, {bonds} bonds, {poses} poses"),
                Event::Picked(report) => {
                    match (self.describe(&report), report.click_to_present_ms) {
                        (Some(what), Some(ms)) => {
                            self.latencies_ms.push(ms);
                            println!(
                                "pick: {what} | GPU readback {:.0} us | click -> re-aimed frame \
                                 presented {ms:.1} ms (budget {BUDGET_MS} ms) | centring error \
                                 {:.2} px",
                                report.pick_us,
                                report.centring_error_px.unwrap_or(f32::NAN),
                            );
                        }
                        _ => println!("pick: nothing under the pointer ({:.0} us)", report.pick_us),
                    }
                    self.last_pick = Some(report);
                }
                Event::Stats(stats) => self.stats = stats,
                Event::Failed(message) => {
                    eprintln!("viewport failed: {message}");
                    self.test.failures.push(format!("render thread: {message}"));
                    self.failure = Some(message);
                }
            }
            cx.notify();
        }

        fn hud(&self) -> impl IntoElement {
            let pick = match &self.last_pick {
                Some(report) => match (self.describe(report), report.click_to_present_ms) {
                    (Some(what), Some(ms)) => {
                        format!("{what} · click → re-aimed {ms:.1} ms (budget {BUDGET_MS})")
                    }
                    _ => "nothing under the pointer".to_string(),
                },
                None => "left-click an atom".to_string(),
            };
            let fps = if self.stats.fps < 0.5 {
                format!("on demand · idle · {} frames", self.stats.frames_total)
            } else {
                format!(
                    "{:.0} fps · {:.2} ms/frame on the render thread (incl. vsync wait) · {} frames",
                    self.stats.fps, self.stats.cpu_frame_ms, self.stats.frames_total
                )
            };
            let poses = match self.shown_pose {
                None => format!("all {} poses", self.arena.pose_count()),
                Some(pose) => format!("pose {} of {}", pose.0, self.arena.pose_count()),
            };
            div()
                .absolute()
                .top_3()
                .left_3()
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(rgba(0xffffff30))
                .bg(rgba(0x0b0e14c0))
                .text_xs()
                .text_color(rgb(0xe6e9ef))
                .child(
                    div()
                        .text_color(rgb(0xf5b942))
                        .child("GPUI overlay (deferred), above the wgpu surface"),
                )
                .child(format!(
                    "{} atoms · {} bonds · {poses} · {}",
                    self.arena.atom_count(),
                    self.bonds,
                    self.representation.label()
                ))
                .child(format!(
                    "{} · {}x{}",
                    self.host.adapter, self.stats.width, self.stats.height
                ))
                .child(fps)
                .child(pick)
                .child(format!(
                    "worst UI-thread stall {:.1} ms",
                    self.worst_ui_stall_ms
                ))
        }

        fn context_menu(&self, at: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
            let item = |id: &'static str, label: &'static str| {
                div()
                    .id(id)
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .text_sm()
                    .text_color(rgb(0xe6e9ef))
                    .cursor_pointer()
                    .hover(|row| row.bg(rgb(0x3b82f6)))
                    .child(label)
            };
            anchored().position(at).snap_to_window().child(
                div()
                    .id("context-menu")
                    .occlude()
                    .w(px(220.))
                    .p_1()
                    .rounded_lg()
                    .bg(rgb(0x1c2029))
                    .border_1()
                    .border_color(rgb(0x3a3f4b))
                    .shadow_xl()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(0x9aa3b2))
                            .child("GPUI context menu"),
                    )
                    .child(
                        item("cm-fit", "Fit to view").on_click(cx.listener(|this, _, _, cx| {
                            this.context_menu = None;
                            this.send(Msg::Fit);
                            cx.notify();
                        })),
                    )
                    .child(item("cm-spin", "Spin on / off").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.context_menu = None;
                            let spin = this.stats.fps < 0.5;
                            this.send(Msg::SetSpin(spin));
                            cx.notify();
                        },
                    )))
                    .child(item("cm-hide", "Hide viewport").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.context_menu = None;
                            this.set_visible(false, cx);
                        },
                    )))
                    .child(item("cm-close", "Close menu").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.context_menu = None;
                            cx.notify();
                        },
                    ))),
            )
        }
    }

    impl Render for Demo {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let entity = cx.entity();
            let visible = self.visible();
            let mut toolbar = div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .bg(rgb(0x161a22))
                .border_b_1()
                .border_color(rgb(0x2a2f3a))
                .child(Button::new("view-menu").label("View ▾").dropdown_menu(
                    move |menu, _, _| {
                        let spin_on = entity.clone();
                        let spin_off = entity.clone();
                        let slow = entity.clone();
                        let toggle = entity.clone();
                        menu.label("GPUI Kit popup menu")
                            .item(PopupMenuItem::new("Spin").on_click(move |_, _, cx| {
                                spin_on.read(cx).send(Msg::SetSpin(true));
                            }))
                            .item(
                                PopupMenuItem::new("Stop spinning").on_click(move |_, _, cx| {
                                    spin_off.read(cx).send(Msg::SetSpin(false));
                                    spin_off.read(cx).send(Msg::SetFrameDelay(Duration::ZERO));
                                }),
                            )
                            .item(PopupMenuItem::new("Spin with 150 ms frames").on_click(
                                move |_, _, cx| {
                                    slow.read(cx).send(Msg::SetSpin(true));
                                    slow.read(cx)
                                        .send(Msg::SetFrameDelay(Duration::from_millis(150)));
                                },
                            ))
                            .separator()
                            .item(PopupMenuItem::new("Hide / show viewport").on_click(
                                move |_, _, cx| {
                                    toggle.update(cx, |this, cx| {
                                        let visible = this.visible();
                                        this.set_visible(!visible, cx);
                                    });
                                },
                            ))
                    },
                ));
            for (id, representation) in [
                ("rep-spheres", Representation::SpaceFilling),
                ("rep-ball", Representation::BallAndStick),
                ("rep-sticks", Representation::Sticks),
            ] {
                let button = Button::new(id).label(representation.label());
                let button = if self.representation == representation {
                    button.primary()
                } else {
                    button
                };
                toolbar = toolbar.child(button.on_click(cx.listener(move |this, _, _, cx| {
                    this.set_representation(representation, cx);
                })));
            }
            let poses_label = match self.shown_pose {
                None => "Poses: all".to_string(),
                Some(pose) => format!("Pose {}", pose.0),
            };
            toolbar =
                toolbar
                    .child(
                        Button::new("poses")
                            .label(poses_label)
                            .on_click(cx.listener(|this, _, _, cx| this.next_pose(cx))),
                    )
                    .child(
                        Button::new("fit")
                            .label("Fit")
                            .on_click(cx.listener(|this, _, _, _| this.send(Msg::Fit))),
                    )
                    .child(
                        Button::new("toggle")
                            .label(if visible {
                                "Hide viewport"
                            } else {
                                "Show viewport"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_visible(!visible, cx);
                            })),
                    )
                    .child(div().text_sm().text_color(rgb(0x9aa3b2)).child(
                        "click: pick + re-aim · drag: orbit · middle-drag: pan · wheel: zoom",
                    ));

            let mut area = div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .bg(rgb(0x0a0c10));
            match &self.viewport {
                Some(viewport) if visible => {
                    area = area.child(ViewportElement {
                        viewport: viewport.clone(),
                    });
                }
                _ => {
                    area = area.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0x9aa3b2))
                            .child(if self.viewport.is_some() {
                                "viewport hidden (empty-bounds path)"
                            } else {
                                "no viewport"
                            }),
                    );
                }
            }
            if visible {
                area = area.child(deferred(self.hud()).priority(1));
            }
            if let Some(failure) = &self.failure {
                area = area.child(
                    deferred(
                        div()
                            .absolute()
                            .bottom_3()
                            .left_3()
                            .right_3()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgba(0x7f1d1de6))
                            .text_sm()
                            .text_color(rgb(0xffffff))
                            .child(failure.clone()),
                    )
                    .priority(2),
                );
            }
            if let Some(at) = self.context_menu {
                area = area.child(deferred(self.context_menu(at, cx)).priority(3));
            }
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(rgb(0x0f1217))
                .text_color(rgb(0xe6e9ef))
                .child(toolbar)
                .child(area)
        }
    }

    /// Render-thread events → UI thread, without blocking it: a background
    /// task waits on the channel.
    fn pump_events(events: mpsc::Receiver<Event>, cx: &mut Context<Demo>) {
        cx.spawn(async move |this, cx| {
            let mut events = events;
            loop {
                let (back, event) = cx
                    .background_spawn(async move {
                        let event = events.recv();
                        (events, event)
                    })
                    .await;
                events = back;
                let Ok(event) = event else { break };
                if this
                    .update(cx, |this, cx| this.on_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// A 16 ms tick on the UI thread: how late it fires is how long the UI
    /// thread was blocked.
    fn measure_ui_stalls(cx: &mut Context<Demo>) {
        cx.spawn(async move |this, cx| {
            const TICK: Duration = Duration::from_millis(16);
            let started = Instant::now();
            loop {
                let tick = Instant::now();
                cx.background_executor().timer(TICK).await;
                let late = tick.elapsed().saturating_sub(TICK).as_secs_f32() * 1000.0;
                // The first two seconds are start-up, not the steady state.
                if started.elapsed() < Duration::from_secs(2) {
                    continue;
                }
                let alive = this.update(cx, |this, _| {
                    this.worst_ui_stall_ms = this.worst_ui_stall_ms.max(late);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Phases, each held long enough to screenshot. Prints `PHASE <name>`.
    fn run_self_test(cx: &mut Context<Demo>) {
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let phase =
                |name: &str| println!("PHASE {name} at {:.1} s", started.elapsed().as_secs_f32());
            let executor = cx.background_executor().clone();
            let wait = move |ms: u64| executor.timer(Duration::from_millis(ms));
            wait(2500).await;

            phase("menu-over-viewport");
            this.update(cx, |this, cx| {
                this.context_menu = Some(point(px(560.), px(330.)));
                this.send(Msg::SetSpin(true));
                cx.notify();
            })
            .ok();
            wait(3000).await;

            for representation in Representation::ALL {
                phase(&format!("representation-{representation:?}"));
                this.update(cx, |this, cx| {
                    this.context_menu = None;
                    this.set_representation(representation, cx);
                })
                .ok();
                wait(1500).await;
            }

            phase("slow-frames");
            this.update(cx, |this, cx| {
                this.worst_ui_stall_ms = 0.0;
                this.send(Msg::SetFrameDelay(Duration::from_millis(150)));
                cx.notify();
            })
            .ok();
            wait(2500).await;
            this.update(cx, |this, cx| {
                let stall = this.worst_ui_stall_ms;
                println!("UI thread worst stall during 150 ms render frames: {stall:.1} ms");
                if stall > 50.0 {
                    this.test.failures.push(format!(
                        "UI thread stalled {stall:.1} ms behind slow frames"
                    ));
                }
                this.send(Msg::SetFrameDelay(Duration::ZERO));
                this.send(Msg::SetSpin(false));
                this.set_representation(Representation::SpaceFilling, cx);
            })
            .ok();
            wait(500).await;

            phase("picking");
            let atoms = this
                .update(cx, |this, _| this.arena.atom_count() as u32)
                .unwrap_or(0);
            for atom in (0..atoms).step_by((atoms as usize / 10).max(1)).take(10) {
                this.update(cx, |this, _| {
                    // Frame everything first, so every chosen atom is in view.
                    this.send(Msg::Fit);
                    this.send(Msg::ClickAtom {
                        atom,
                        at: Instant::now(),
                    })
                })
                .ok();
                wait(300).await;
            }
            wait(1500).await;

            phase("hidden");
            this.update(cx, |this, cx| {
                this.set_visible(false, cx);
                // Would draw every frame if the hidden viewport rendered at all.
                this.send(Msg::SetSpin(true));
            })
            .ok();
            wait(800).await;
            this.update(cx, |this, _| {
                this.test.frames_when_hidden = Some(this.stats.frames_total)
            })
            .ok();
            wait(2200).await;
            this.update(cx, |this, _| {
                let before = this.test.frames_when_hidden.unwrap_or_default();
                if this.stats.frames_total != before {
                    this.test.failures.push(format!(
                        "the hidden viewport kept drawing ({before} -> {} frames)",
                        this.stats.frames_total
                    ));
                }
            })
            .ok();

            phase("shown");
            this.update(cx, |this, cx| this.set_visible(true, cx)).ok();
            wait(3000).await;

            this.update(cx, |this, _| {
                let before = this.test.frames_when_hidden.unwrap_or_default();
                if this.stats.frames_total <= before {
                    this.test
                        .failures
                        .push("the viewport did not draw again after showing it".into());
                }
                let mut latencies = this.latencies_ms.clone();
                latencies.sort_by(f32::total_cmp);
                if latencies.len() < 10 {
                    this.test.failures.push(format!(
                        "{} of 10 synthetic clicks picked an atom and re-aimed the camera",
                        latencies.len()
                    ));
                }
                if let (Some(min), Some(max)) = (latencies.first(), latencies.last()) {
                    let median = latencies[latencies.len() / 2];
                    println!(
                        "click -> re-aimed frame presented: min {min:.1} / median {median:.1} / \
                         max {max:.1} ms over {} clicks (budget {BUDGET_MS} ms)",
                        latencies.len()
                    );
                    if *max > BUDGET_MS {
                        this.test.failures.push(format!("a click took {max:.1} ms"));
                    }
                }
                if !this.test.enabled {
                    return;
                }
                if this.test.failures.is_empty() {
                    println!("SELF-TEST PASSED");
                    std::process::exit(0);
                }
                for failure in &this.test.failures {
                    println!("FAILED: {failure}");
                }
                std::process::exit(1);
            })
            .ok();
        })
        .detach();
    }
}
