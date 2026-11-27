//! The GPUI side: a custom `Element` that takes part in layout and draws
//! nothing itself. Its bounds place a GPUI Fast native surface, below GPUI's
//! overlay layer, whose content a [`ViewportBackend`] hands to wgpu.
//!
//! ```text
//! front  GPUI overlay     deferred draws: menus, popovers, HUD
//!        wgpu swap chain  the viewport, own DX12 device and render thread
//! back   GPUI base        the root view: toolbar, panels
//! ```
//!
//! Only `deferred(..)` content reaches the overlay layer. A plain child drawn
//! "on top of" the viewport in the element tree lands in the base layer and is
//! hidden behind the swap chain.
//!
//! The platform-neutral flow is gpui-fast's: `enable_window_composition` →
//! `create_native_surface` → `set_bounds` / `on_input`. Ported from
//! `javagg/gpui-fast-wgpu-demo` (`src/viewport.rs`, `src/viewport_element.rs`).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use gpui_kit::{
    App, Bounds, DevicePixels, DispatchEventResult, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, MouseButton, Pixels, PlatformInput, Point, ScrollDelta, Style,
    Window, WindowCompositionSurface, relative,
};

use crate::backend::ViewportBackend;
use mold_render::host::{Button, Msg, Pointer, PointerKind, RenderHost};

/// One viewport's native surface in the window, and the channel to the render
/// thread that draws it.
pub struct Viewport {
    commands: Sender<Msg>,
    surface: WindowCompositionSurface,
    /// Where the element was last laid out, and at what scale factor.
    geometry: Rc<Cell<(Bounds<Pixels>, f32)>>,
    size: Cell<(u32, u32)>,
    visible: Cell<bool>,
    needs_commit: bool,
}

impl std::fmt::Debug for Viewport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Viewport")
            .field("size", &self.size.get())
            .field("visible", &self.visible.get())
            .finish_non_exhaustive()
    }
}

impl Viewport {
    /// Creates the native surface, attaches the render thread's swap chain to
    /// it through `backend`, and routes the surface's pointer input to the
    /// render thread. Fails with a readable message rather than a blank view.
    pub fn new(
        window: &Window,
        backend: &dyn ViewportBackend,
        host: &RenderHost,
    ) -> Result<Rc<Self>, String> {
        let composition = window.enable_window_composition().map_err(|e| {
            format!(
                "GPUI window composition is unavailable ({e:#}). The viewport needs gpui-fast's \
                 composition; is GPUI_DISABLE_DIRECT_COMPOSITION set?"
            )
        })?;
        let surface = composition
            .create_native_surface()
            .map_err(|e| format!("creating a native surface: {e:#}"))?;
        let platform = surface
            .platform_surface()
            .map_err(|e| format!("the native surface has no platform surface: {e:#}"))?;
        let handle = platform
            .platform_handle()
            .map_err(|e| format!("reading the native surface handle: {e:#}"))?;
        let wgpu_surface = backend
            .create_surface(host.instance(), handle)
            .map_err(|e| e.to_string())?;
        host.attach(wgpu_surface);

        let viewport = Rc::new(Self {
            commands: host.sender(),
            surface: surface.clone(),
            geometry: Rc::new(Cell::new((Bounds::default(), window.scale_factor()))),
            size: Cell::new((0, 0)),
            visible: Cell::new(true),
            needs_commit: backend.needs_commit_after_present(),
        });
        // The first configure (in `attach`) set the swap chain as the visual's
        // content; commit so it is part of the tree before the first present.
        viewport.commit();

        composition
            .on_input(surface.id(), {
                let commands = host.sender();
                let geometry = viewport.geometry.clone();
                move |event| {
                    let (bounds, scale) = geometry.get();
                    forward_input(event, &commands, bounds, scale)
                }
            })
            .map_err(|e| format!("routing the viewport's input: {e:#}"))?;
        Ok(viewport)
    }

    pub fn send(&self, msg: Msg) {
        self.commands.send(msg).ok();
    }

    pub fn is_visible(&self) -> bool {
        self.visible.get()
    }

    /// Shows or hides the surface. GPUI does not do this for an element that
    /// stops being drawn: its native surface stays where it was.
    ///
    /// Hiding gives the surface **empty bounds**, which GPUI neither shows nor
    /// routes input to. `PlatformSurfaceAttachment::set_visible` and
    /// `WindowComposition::remove_surface` cannot be used on Windows: both cast
    /// the visual to `IDCompositionVisual3`, which visuals from gpui-fast's
    /// `DCompositionCreateDevice` device don't implement (E_NOINTERFACE).
    pub fn set_visible(&self, visible: bool) {
        if self.visible.replace(visible) == visible {
            return;
        }
        let bounds = if visible {
            let (bounds, scale) = self.geometry.get();
            bounds.to_device_pixels(scale)
        } else {
            Bounds::default()
        };
        let placed = self
            .surface
            .platform_surface()
            .and_then(|surface| surface.set_bounds(bounds));
        if let Err(error) = placed {
            eprintln!("changing the viewport's visibility: {error:#}");
        }
        self.send(Msg::SetVisible(visible));
    }

    /// Commits GPUI's composition, if the backend needs it. Call on
    /// [`Event::CommitComposition`](mold_render::host::Event::CommitComposition).
    ///
    /// gpui-fast has no public commit. `set_parent_origin` commits the
    /// DirectComposition device unconditionally on Windows (it re-applies the
    /// bounds, and the Windows attachment commits), unlike `set_bounds`, which
    /// returns early while the bounds hold still. That is a backend
    /// implementation detail, not a documented guarantee (ADR-0003).
    pub fn commit(&self) {
        if !self.needs_commit {
            return;
        }
        let committed = self
            .surface
            .platform_surface()
            .and_then(|surface| surface.set_parent_origin(Point::default()));
        if let Err(error) = committed {
            eprintln!("committing the composition: {error:#}");
        }
    }

    /// Places the surface. Runs in prepaint, so it follows GPUI's layout.
    fn place(&self, bounds: Bounds<Pixels>, scale: f32) {
        self.geometry.set((bounds, scale));
        if !self.visible.get() {
            return;
        }
        let device: Bounds<DevicePixels> = bounds.to_device_pixels(scale);
        let placed = self
            .surface
            .platform_surface()
            .and_then(|surface| surface.set_bounds(device));
        if let Err(error) = placed {
            eprintln!("placing the viewport: {error:#}");
        }
        let size = (
            device.size.width.0.max(1) as u32,
            device.size.height.0.max(1) as u32,
        );
        if self.size.replace(size) != size {
            self.send(Msg::Resize {
                width: size.0,
                height: size.1,
            });
        }
    }
}

/// Passes a pointer event to the render thread in the surface's physical
/// pixels. Runs on the UI thread during dispatch and only forwards, so the
/// camera keeps up with the mouse however busy the UI thread is.
fn forward_input(
    event: &PlatformInput,
    commands: &Sender<Msg>,
    bounds: Bounds<Pixels>,
    scale: f32,
) -> DispatchEventResult {
    let at = Instant::now();
    let button = |button: MouseButton| match button {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Middle => Some(Button::Middle),
        MouseButton::Right => Some(Button::Right),
        MouseButton::Navigate(_) => None,
    };
    let (kind, position) = match event {
        PlatformInput::MouseDown(event) => match button(event.button) {
            Some(b) => (PointerKind::Down(b), event.position),
            None => return passed_on(),
        },
        PlatformInput::MouseUp(event) => match button(event.button) {
            Some(b) => (PointerKind::Up(b), event.position),
            None => return passed_on(),
        },
        PlatformInput::MouseMove(event) => (PointerKind::Move, event.position),
        PlatformInput::MouseExited(event) => (PointerKind::Leave, event.position),
        PlatformInput::Scroll(event) => {
            let lines = match event.delta {
                ScrollDelta::Lines(lines) => lines.y,
                ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
            };
            (PointerKind::Scroll(lines), event.position)
        }
        _ => return passed_on(),
    };
    let pointer = Pointer {
        kind,
        x: f32::from(position.x - bounds.origin.x) * scale,
        y: f32::from(position.y - bounds.origin.y) * scale,
        window_x: f32::from(position.x),
        window_y: f32::from(position.y),
        at,
    };
    commands.send(Msg::Pointer(pointer)).ok();
    DispatchEventResult {
        propagate: false,
        default_prevented: false,
    }
}

fn passed_on() -> DispatchEventResult {
    DispatchEventResult {
        propagate: true,
        default_prevented: false,
    }
}

/// The element: fills its parent, draws nothing, places the surface.
#[derive(Debug)]
pub struct ViewportElement {
    pub viewport: Rc<Viewport>,
}

impl Element for ViewportElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.viewport.place(bounds, window.scale_factor());
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
