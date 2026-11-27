//! The selection bus: one [`Selection`] broadcast to every panel; no panel owns it.
//!
//! GPUI-free. The `app` crate owns the single instance (through the GPUI wrapper,
//! `view::SelectionHub`, on Windows); panels and the viewport register, publish and
//! take deliveries. Headless code (tests, a future render-thread bridge) can use
//! [`SelectionBus`] directly.
//!
//! Semantics:
//! - **Selection is state, not an event stream.** Each registered panel has one
//!   pending slot; a newer publication replaces an undelivered older one
//!   (latest wins). A slow panel never sees a backlog, only the newest value.
//! - **Every publish reaches every registered panel exactly once**, the publisher
//!   included: its own copy is its *echo*, recognisable with
//!   [`Publication::is_from`]. A panel normally ignores its echo.
//! - **Publishing the value that is already current is a no-op**: no revision, no
//!   deliveries. This is what stops two panels that both republish on change from
//!   looping: the second republish of the same value goes nowhere.
//! - **Late registration sees the current value**: a panel registered after a
//!   publish finds the latest publication pending.

use std::collections::BTreeSet;
use std::sync::Arc;

use mold_mol::Selection;

/// Who publishes and receives on the bus: one per panel (table, plot, viewport,
/// the app itself). Issued by [`SelectionBus::register`]; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PanelId(u32);

/// One value on the bus: the selection, who published it, and its revision
/// (strictly increasing per bus, starting at 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub selection: Arc<Selection>,
    pub origin: PanelId,
    pub revision: u64,
}

impl Publication {
    /// Whether `panel` published this one, i.e. whether it is `panel`'s own echo.
    pub fn is_from(&self, panel: PanelId) -> bool {
        self.origin == panel
    }
}

static EMPTY: Selection = Selection {
    poses: BTreeSet::new(),
    atom_mask: None,
};

#[derive(Debug)]
struct Slot {
    name: String,
    pending: Option<Publication>,
}

/// The bus. See the module docs for the delivery rules.
#[derive(Debug, Default)]
pub struct SelectionBus {
    /// Indexed by `PanelId.0`; `None` once unregistered.
    slots: Vec<Option<Slot>>,
    latest: Option<Publication>,
}

impl SelectionBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a panel. `name` is for display and logs ("pose table", "plot").
    /// If anything was published before, the latest publication is already pending
    /// for the new panel.
    pub fn register(&mut self, name: impl Into<String>) -> PanelId {
        let id = PanelId(u32::try_from(self.slots.len()).expect("fewer than 2^32 panels"));
        self.slots.push(Some(Slot {
            name: name.into(),
            pending: self.latest.clone(),
        }));
        id
    }

    /// Stop delivering to `panel` (closed). Its publications already on the bus
    /// stay. Unknown or already removed ids are ignored.
    pub fn unregister(&mut self, panel: PanelId) {
        if let Some(slot) = self.slots.get_mut(panel.0 as usize) {
            *slot = None;
        }
    }

    /// The name `panel` registered with, while it is registered.
    pub fn name(&self, panel: PanelId) -> Option<&str> {
        self.slot(panel).map(|s| s.name.as_str())
    }

    /// Publish `selection` from `from`. Returns the new publication, or `None` if
    /// `selection` equals the current one (nothing is delivered then).
    ///
    /// Publishing from an unregistered id is allowed; the publication is still
    /// delivered to every registered panel.
    pub fn publish(&mut self, from: PanelId, selection: Selection) -> Option<&Publication> {
        if selection == *self.current() {
            return None;
        }
        let publication = Publication {
            selection: Arc::new(selection),
            origin: from,
            revision: self.revision() + 1,
        };
        for slot in self.slots.iter_mut().flatten() {
            slot.pending = Some(publication.clone());
        }
        self.latest = Some(publication);
        self.latest.as_ref()
    }

    /// Take `panel`'s pending delivery, if any. At most one is pending at a time.
    pub fn take(&mut self, panel: PanelId) -> Option<Publication> {
        self.slot_mut(panel).and_then(|s| s.pending.take())
    }

    /// Whether `panel` has a delivery waiting.
    pub fn has_pending(&self, panel: PanelId) -> bool {
        self.slot(panel).is_some_and(|s| s.pending.is_some())
    }

    /// The current selection (empty before the first publish).
    pub fn current(&self) -> &Selection {
        self.latest.as_ref().map_or(&EMPTY, |p| &p.selection)
    }

    /// The latest publication, `None` before the first publish.
    pub fn latest(&self) -> Option<&Publication> {
        self.latest.as_ref()
    }

    /// Revision of the current selection; 0 before the first publish.
    pub fn revision(&self) -> u64 {
        self.latest.as_ref().map_or(0, |p| p.revision)
    }

    /// The registered panels, in registration order.
    pub fn panels(&self) -> impl Iterator<Item = (PanelId, &str)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (PanelId(i as u32), s.name.as_str())))
    }

    fn slot(&self, panel: PanelId) -> Option<&Slot> {
        self.slots.get(panel.0 as usize).and_then(Option::as_ref)
    }

    fn slot_mut(&mut self, panel: PanelId) -> Option<&mut Slot> {
        self.slots
            .get_mut(panel.0 as usize)
            .and_then(Option::as_mut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mold_mol::PoseId;

    fn sel(ids: &[u32]) -> Selection {
        Selection {
            poses: ids.iter().map(|&i| PoseId(i)).collect(),
            atom_mask: None,
        }
    }

    #[test]
    fn a_publish_reaches_every_other_panel_exactly_once() {
        let mut bus = SelectionBus::new();
        let table = bus.register("table");
        let plot = bus.register("plot");
        let viewport = bus.register("viewport");

        let rev = bus.publish(table, sel(&[3])).unwrap().revision;
        assert_eq!(rev, 1);

        for panel in [plot, viewport] {
            let got = bus.take(panel).expect("delivered");
            assert_eq!(*got.selection, sel(&[3]));
            assert_eq!(got.origin, table);
            assert!(!got.is_from(panel));
            assert_eq!(bus.take(panel), None, "exactly once");
        }
    }

    #[test]
    fn the_publisher_can_tell_its_own_echo() {
        let mut bus = SelectionBus::new();
        let table = bus.register("table");
        let _plot = bus.register("plot");
        bus.publish(table, sel(&[1, 2]));
        let echo = bus.take(table).expect("the publisher gets its echo");
        assert!(echo.is_from(table));
        assert_eq!(bus.take(table), None);
    }

    #[test]
    fn republishing_the_current_value_is_a_noop() {
        let mut bus = SelectionBus::new();
        let a = bus.register("a");
        let b = bus.register("b");
        bus.publish(a, sel(&[5]));
        bus.take(a);
        bus.take(b);
        assert!(bus.publish(b, sel(&[5])).is_none());
        assert_eq!(bus.revision(), 1);
        assert!(!bus.has_pending(a) && !bus.has_pending(b));
        // Clearing an empty bus is a no-op too.
        let mut fresh = SelectionBus::new();
        let c = fresh.register("c");
        assert!(fresh.publish(c, Selection::default()).is_none());
        assert_eq!(fresh.revision(), 0);
    }

    #[test]
    fn undelivered_publications_coalesce_latest_wins() {
        let mut bus = SelectionBus::new();
        let a = bus.register("a");
        let slow = bus.register("slow");
        bus.publish(a, sel(&[1]));
        bus.publish(a, sel(&[2]));
        bus.publish(a, sel(&[3]));
        let got = bus.take(slow).unwrap();
        assert_eq!(*got.selection, sel(&[3]));
        assert_eq!(got.revision, 3);
        assert_eq!(bus.take(slow), None);
    }

    #[test]
    fn late_subscribers_see_the_current_value() {
        let mut bus = SelectionBus::new();
        let early = bus.register("early");
        let nothing_yet = bus.register("before any publish");
        assert_eq!(bus.take(nothing_yet), None);
        assert!(bus.current().is_empty());

        bus.publish(early, sel(&[7, 8]));
        let late = bus.register("late");
        let got = bus.take(late).expect("the current value is pending");
        assert_eq!(*got.selection, sel(&[7, 8]));
        assert_eq!(got.origin, early);
        assert_eq!(bus.current(), &sel(&[7, 8]));
    }

    #[test]
    fn unregistered_panels_get_nothing_and_ids_are_not_reused() {
        let mut bus = SelectionBus::new();
        let a = bus.register("a");
        let b = bus.register("b");
        bus.unregister(b);
        bus.publish(a, sel(&[1]));
        assert_eq!(bus.take(b), None);
        assert_eq!(bus.name(b), None);
        let c = bus.register("c");
        assert_ne!(b, c);
        assert_eq!(bus.panels().map(|(_, n)| n).collect::<Vec<_>>(), ["a", "c"]);
    }

    /// A stand-in panel that republishes whenever its local selection changes, as
    /// a widget's "selection changed" callback would. `knows` models a panel that
    /// can only represent some poses (e.g. a filtered table) and so republishes a
    /// *different* value than it received.
    struct NaivePanel {
        id: PanelId,
        local: Selection,
        knows: fn(PoseId) -> bool,
        ignore_echo: bool,
    }

    impl NaivePanel {
        /// Handle one delivery; returns what the widget's change callback would publish.
        fn on_delivery(&mut self, p: &Publication) -> Option<Selection> {
            if self.ignore_echo && p.is_from(self.id) {
                return None;
            }
            let shown = Selection {
                poses: p
                    .selection
                    .poses
                    .iter()
                    .copied()
                    .filter(|&id| (self.knows)(id))
                    .collect(),
                atom_mask: None,
            };
            if shown == self.local {
                return None;
            }
            self.local = shown.clone();
            Some(shown) // "selection changed" → republish
        }
    }

    /// Pump deliveries until the bus is quiet; returns the number of rounds.
    fn pump(bus: &mut SelectionBus, panels: &mut [NaivePanel]) -> usize {
        for round in 1..=100 {
            let mut any = false;
            for p in panels.iter_mut() {
                while let Some(d) = bus.take(p.id) {
                    any = true;
                    if let Some(s) = p.on_delivery(&d) {
                        bus.publish(p.id, s);
                    }
                }
            }
            if !any {
                return round;
            }
        }
        panic!("the bus did not settle: feedback loop");
    }

    #[test]
    fn two_panels_that_republish_on_change_do_not_loop() {
        let mut bus = SelectionBus::new();
        let mut panels = [
            NaivePanel {
                id: bus.register("plot"),
                local: Selection::default(),
                knows: |_| true,
                ignore_echo: true,
            },
            // Only even poses pass this table's filter: it republishes a subset.
            NaivePanel {
                id: bus.register("filtered table"),
                local: Selection::default(),
                knows: |id| id.0 % 2 == 0,
                ignore_echo: true,
            },
        ];
        let app = bus.register("app");
        bus.publish(app, sel(&[1, 2, 3, 4]));
        let rounds = pump(&mut bus, &mut panels);
        assert!(rounds <= 4, "settled in {rounds} rounds");
        // The table narrowed the selection once; the plot followed; then quiet.
        assert_eq!(bus.current(), &sel(&[2, 4]));
        assert_eq!(panels[0].local, sel(&[2, 4]));
        assert_eq!(panels[1].local, sel(&[2, 4]));
        assert_eq!(bus.revision(), 2);
    }

    #[test]
    fn panels_that_even_republish_their_own_echo_still_settle() {
        let mut bus = SelectionBus::new();
        let mut panels: Vec<NaivePanel> = ["a", "b"]
            .into_iter()
            .map(|n| NaivePanel {
                id: bus.register(n),
                local: Selection::default(),
                knows: |_| true,
                ignore_echo: false,
            })
            .collect();
        bus.publish(panels[0].id, sel(&[9]));
        let rounds = pump(&mut bus, &mut panels);
        assert!(rounds <= 3, "settled in {rounds} rounds");
        assert_eq!(bus.revision(), 1, "only the original publish went out");
    }
}
