use std::rc::Rc;

use gpui_kit::*;
use mold_mol::Selection;

use crate::bus::{PanelId, Publication, SelectionBus};

/// The GPUI face of the selection bus: an entity wrapping the one
/// [`SelectionBus`]. The `app` crate creates it and hands the `Entity` to every
/// panel (and later the viewport); no panel owns it.
///
/// Delivery rules are the bus's (exactly once per panel, echo marked, latest
/// wins, republishing the current value is a no-op). GPUI only wakes panels: a
/// publish that changes the selection notifies this entity, and each panel
/// connected with [`SelectionHub::connect`] takes its own delivery from the bus.
#[derive(Debug, Default)]
pub struct SelectionHub {
    bus: SelectionBus,
}

impl SelectionHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bus, for reading (current value, revision, panel names).
    pub fn bus(&self) -> &SelectionBus {
        &self.bus
    }

    pub fn current(&self) -> &Selection {
        self.bus.current()
    }

    /// Register a panel by hand. Views normally use [`SelectionHub::connect`].
    pub fn register(&mut self, name: impl Into<String>) -> PanelId {
        self.bus.register(name)
    }

    pub fn unregister(&mut self, panel: PanelId) {
        self.bus.unregister(panel);
    }

    /// Publish from `from`. Returns the new revision, or `None` if `selection`
    /// is already current (nothing is delivered and nobody is woken).
    pub fn publish(
        &mut self,
        from: PanelId,
        selection: Selection,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let revision = self.bus.publish(from, selection).map(|p| p.revision);
        if revision.is_some() {
            cx.notify();
        }
        revision
    }

    /// `panel`'s pending delivery, if any.
    pub fn take(&mut self, panel: PanelId) -> Option<Publication> {
        self.bus.take(panel)
    }

    /// Connect the view being built in `cx` as panel `name`. `on_selection` runs
    /// once per delivery, own echoes included (check [`Publication::is_from`]);
    /// it may publish. A panel connected after a publish receives the current
    /// value right after construction. Dropping the returned [`Connection`] stops
    /// deliveries; releasing the view unregisters the panel.
    pub fn connect<V: 'static>(
        hub: &Entity<Self>,
        name: impl Into<String>,
        cx: &mut Context<V>,
        on_selection: impl Fn(&mut V, Publication, &mut Context<V>) + 'static,
    ) -> Connection {
        let panel = hub.update(cx, |h, _| h.register(name));
        let on_selection = Rc::new(on_selection);

        let deliver = {
            let f = on_selection.clone();
            move |this: &mut V, hub: Entity<SelectionHub>, cx: &mut Context<V>| {
                if let Some(p) = hub.update(cx, |h, _| h.take(panel)) {
                    f(this, p, cx);
                }
            }
        };
        let observe = cx.observe(hub, deliver.clone());

        // The current value, for a late connection: after construction, once the
        // view exists. (A notify sent now could arrive before `observe` is live.)
        let this = cx.weak_entity();
        let hub_for_init = hub.clone();
        cx.defer(move |cx| {
            let _ = this.update(cx, |v, cx| deliver(v, hub_for_init, cx));
        });

        let weak_hub = hub.downgrade();
        let release = cx.on_release(move |_, cx| {
            let _ = weak_hub.update(cx, |h, _| h.unregister(panel));
        });

        Connection {
            panel,
            _observe: observe,
            _release: release,
        }
    }
}

/// A view's connection to the [`SelectionHub`]. Keep it in the view.
pub struct Connection {
    pub panel: PanelId,
    _observe: Subscription,
    _release: Subscription,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("panel", &self.panel)
            .finish_non_exhaustive()
    }
}
