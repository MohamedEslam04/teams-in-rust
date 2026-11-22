//! The card host on a Wayland compositor with wlr-layer-shell (KWin, Sway, Hyprland).
//!
//! One overlay-layer surface, anchored to the bottom-right corner with a margin, holds the whole
//! stack. The compositor places it (above the panel's exclusive zone, on the output it picks,
//! usually the active one) and keeps it above normal windows, also when IronTeams is minimised
//! or in the tray. `keyboard_interactivity: on_demand` lets a click in the reply box take the
//! keyboard without the card stealing it when it appears. The surface is destroyed when the last
//! card goes and made again for the next one.
//!
//! Slint draws through a small custom platform: a `MinimalSoftwareWindow` rendered by the software
//! renderer into a premultiplied RGBA buffer, copied into a wl_shm ARGB8888 buffer. Pointer and
//! keyboard events from the seat (smithay-client-toolkit, xkbcommon for the key text) become
//! Slint window events. The event loop is calloop: the Wayland socket, the cards from stdin (a
//! channel fed by a reader thread) and Slint's timers.

use super::geometry::MARGIN;
use super::host::{self, Core};
use super::protocol::{self, Backend, FromHost, ToHost};
use crate::CardHost;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Key, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::{EventLoop, channel};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::{GlobalListContents, registry_queue_init};
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{
    CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, registry_handlers,
};
use std::cell::Cell;
use std::io::BufRead;
use std::rc::Rc;
use std::time::Duration;

/// The registry listener of [`probe`]; the globals are read from the list it fills.
struct Probe;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Whether the compositor advertises the layer shell.
pub fn probe() -> bool {
    let Ok(conn) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, _queue)) = registry_queue_init::<Probe>(&conn) else {
        return false;
    };
    globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == "zwlr_layer_shell_v1"))
}

/// Slint's platform in the host: the one window, drawn by [`State::draw`].
struct Platform {
    window: Rc<MinimalSoftwareWindow>,
}

impl slint::platform::Platform for Platform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

struct State {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<State>,
    window: Rc<MinimalSoftwareWindow>,
    host: CardHost,
    /// The stack changed: resize, make or drop the surface.
    dirty: Rc<Cell<bool>>,
    layer: Option<LayerSurface>,
    /// The compositor configured the current surface: it may be drawn.
    configured: bool,
    /// The surface size asked for, logical pixels.
    size: (u32, u32),
    /// The buffer scale of the outputs the surface is on.
    scale: i32,
    pixels: Vec<PremultipliedRgbaColor>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer>,
    exit: bool,
}

/// Physical pixels of a logical length at `scale`.
fn phys(l: u32, scale: i32) -> u32 {
    l * scale.max(1) as u32
}

impl State {
    fn on_message(&mut self, msg: ToHost, core: &Core) {
        if host::apply(core, msg) {
            self.exit = true;
        }
    }

    /// Makes, resizes or drops the surface to fit the stack.
    fn relayout(&mut self) {
        if !self.dirty.replace(false) {
            return;
        }
        let h = self.host.get_wanted_height().ceil() as u32;
        if h == 0 {
            // The last card went: the surface goes with it.
            self.layer = None;
            self.configured = false;
            return;
        }
        let w = self.host.get_wanted_width().ceil() as u32;
        let pad = self.host.get_pad().round() as i32;
        let margin = MARGIN as i32 - pad;
        if self.layer.is_none() {
            let surface = self.compositor.create_surface(&self.qh);
            let layer = self.layer_shell.create_layer_surface(
                &self.qh,
                surface,
                Layer::Overlay,
                Some("ironteams-notifications"),
                None,
            );
            layer.set_anchor(Anchor::BOTTOM | Anchor::RIGHT);
            layer.set_margin(0, margin, margin, 0);
            layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
            self.layer = Some(layer);
            self.configured = false;
        }
        if let Some(layer) = &self.layer {
            if (w, h) != self.size || !self.configured {
                layer.set_size(w, h);
                // Drawn again once the compositor has configured the new size.
                self.configured = false;
            }
            // Clicks on the shadow around the cards go to what is under it.
            if let Ok(region) = Region::new(&self.compositor) {
                region.add(pad, pad, w as i32 - 2 * pad, h as i32 - 2 * pad);
                layer
                    .wl_surface()
                    .set_input_region(Some(region.wl_region()));
            }
            layer.commit();
        }
        self.size = (w, h);
        self.resize_window();
    }

    /// Tells Slint the window's size in physical pixels.
    fn resize_window(&self) {
        self.window.set_size(PhysicalSize::new(
            phys(self.size.0, self.scale),
            phys(self.size.1, self.scale),
        ));
        self.window.request_redraw();
    }

    /// Draws the stack when Slint has something new to show.
    fn draw(&mut self) {
        let Some(layer) = &self.layer else { return };
        if !self.configured {
            return;
        }
        let (w, h) = (phys(self.size.0, self.scale), phys(self.size.1, self.scale));
        if w == 0 || h == 0 {
            return;
        }
        let (pixels, pool, scale) = (&mut self.pixels, &mut self.pool, self.scale);
        self.window.draw_if_needed(|renderer| {
            pixels.clear();
            pixels.resize((w * h) as usize, PremultipliedRgbaColor::default());
            let _ = renderer.render(pixels.as_mut_slice(), w as usize);
            let Ok((buffer, canvas)) =
                pool.create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
            else {
                return;
            };
            // wl_shm ARGB8888 is premultiplied, little endian: B, G, R, A in memory.
            for (px, out) in pixels.iter().zip(canvas.as_chunks_mut::<4>().0) {
                *out = [px.blue, px.green, px.red, px.alpha];
            }
            let surface = layer.wl_surface();
            surface.set_buffer_scale(scale);
            surface.damage_buffer(0, 0, w as i32, h as i32);
            if buffer.attach_to(surface).is_ok() {
                layer.commit();
            }
        });
    }

    fn ours(&self, surface: &wl_surface::WlSurface) -> bool {
        self.layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
    }
}

/// The Slint key for a key event: the named keys by their keysym, the rest by their text.
fn key_text(e: &KeyEvent) -> Option<SharedString> {
    let key = match e.keysym {
        Keysym::Return | Keysym::KP_Enter => Key::Return,
        Keysym::BackSpace => Key::Backspace,
        Keysym::Delete | Keysym::KP_Delete => Key::Delete,
        Keysym::Escape => Key::Escape,
        Keysym::Tab => Key::Tab,
        Keysym::ISO_Left_Tab => Key::Backtab,
        Keysym::Left | Keysym::KP_Left => Key::LeftArrow,
        Keysym::Right | Keysym::KP_Right => Key::RightArrow,
        Keysym::Up | Keysym::KP_Up => Key::UpArrow,
        Keysym::Down | Keysym::KP_Down => Key::DownArrow,
        Keysym::Home | Keysym::KP_Home => Key::Home,
        Keysym::End | Keysym::KP_End => Key::End,
        Keysym::Shift_L => Key::Shift,
        Keysym::Shift_R => Key::ShiftR,
        Keysym::Control_L => Key::Control,
        Keysym::Control_R => Key::ControlR,
        Keysym::Alt_L | Keysym::Alt_R => Key::Alt,
        Keysym::Super_L | Keysym::Super_R => Key::Meta,
        _ => {
            return e
                .utf8
                .as_deref()
                .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
                .map(SharedString::from);
        }
    };
    Some(key.into())
}

fn button(code: u32) -> PointerEventButton {
    // linux/input-event-codes.h
    match code {
        0x110 => PointerEventButton::Left,
        0x111 => PointerEventButton::Right,
        0x112 => PointerEventButton::Middle,
        _ => PointerEventButton::Other,
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if !self.ours(surface) || new_factor == self.scale || new_factor < 1 {
            return;
        }
        self.scale = new_factor;
        self.window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: new_factor as f32,
        });
        self.resize_window();
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        // The compositor took the surface away (an output went): make a new one if cards remain.
        self.layer = None;
        self.configured = false;
        self.dirty.set(true);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        if w > 0 && h > 0 {
            self.size = (w, h);
        }
        self.configured = true;
        self.resize_window();
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seats.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seats
                .get_pointer_with_theme(qh, &seat, self.shm.wl_shm(), surface, ThemeSpec::default())
                .ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if self.ours(surface) {
            self.window
                .dispatch_event(WindowEvent::WindowActiveChanged(true));
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if self.ours(surface) {
            self.window
                .dispatch_event(WindowEvent::WindowActiveChanged(false));
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if let Some(text) = key_text(&event) {
            self.window.dispatch_event(WindowEvent::KeyPressed { text });
        }
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if let Some(text) = key_text(&event) {
            self.window
                .dispatch_event(WindowEvent::KeyPressRepeated { text });
        }
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if let Some(text) = key_text(&event) {
            self.window
                .dispatch_event(WindowEvent::KeyReleased { text });
        }
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if !self.ours(&event.surface) {
                continue;
            }
            let position = LogicalPosition::new(event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(p) = &self.pointer {
                        let _ = p.set_cursor(conn, CursorIcon::Default);
                    }
                    self.window
                        .dispatch_event(WindowEvent::PointerMoved { position });
                }
                PointerEventKind::Leave { .. } => {
                    self.window.dispatch_event(WindowEvent::PointerExited);
                }
                PointerEventKind::Motion { .. } => {
                    self.window
                        .dispatch_event(WindowEvent::PointerMoved { position });
                }
                PointerEventKind::Press { button: b, .. } => {
                    self.window.dispatch_event(WindowEvent::PointerPressed {
                        position,
                        button: button(b),
                    });
                }
                PointerEventKind::Release { button: b, .. } => {
                    self.window.dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: button(b),
                    });
                }
                PointerEventKind::Axis { .. } => {}
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_keyboard!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_registry!(State);

/// Runs the host on the layer shell until the app closes its stdin.
pub fn run() -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let pool = SlotPool::new(400 * 300 * 4, &shm)?;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Platform {
        window: window.clone(),
    }))
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let host = CardHost::new()?;
    host.show()?;
    super::host::apply_language();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });

    let dirty = Rc::new(Cell::new(false));
    let d = dirty.clone();
    let core = Rc::new(Core::new(|m| host::emit(&m), move || d.set(true)));
    host::wire(&host, &core);

    let mut state = State {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        qh: qh.clone(),
        window,
        host,
        dirty,
        layer: None,
        configured: false,
        size: (0, 0),
        scale: 1,
        pixels: Vec::new(),
        keyboard: None,
        pointer: None,
        exit: false,
    };

    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|e| anyhow::anyhow!("{}", e.error))?;
    let (tx, rx) = channel::channel::<ToHost>();
    let c = core.clone();
    handle
        .insert_source(rx, move |event, _, state: &mut State| match event {
            channel::Event::Msg(msg) => state.on_message(msg, &c),
            channel::Event::Closed => state.exit = true,
        })
        .map_err(|e| anyhow::anyhow!("{}", e.error))?;
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Some(msg) = protocol::parse::<ToHost>(&line)
                && tx.send(msg).is_err()
            {
                break;
            }
        }
        // Dropping the sender tells the loop that the app is gone.
    });

    host::emit(&FromHost::Ready {
        backend: Backend::LayerShell,
    });
    while !state.exit {
        slint::platform::update_timers_and_animations();
        state.relayout();
        state.draw();
        conn.flush()?;
        let timeout = if state.window.has_active_animations() {
            Some(Duration::from_millis(16))
        } else {
            slint::platform::duration_until_next_timer_update()
        };
        event_loop.dispatch(timeout, &mut state)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(keysym: Keysym, utf8: Option<&str>) -> KeyEvent {
        KeyEvent {
            time: 0,
            raw_code: 0,
            keysym,
            utf8: utf8.map(str::to_owned),
        }
    }

    #[test]
    fn named_keys_become_slint_keys_and_letters_their_text() {
        let text = |k, u| key_text(&event(k, u)).map(|s| s.to_string());
        assert_eq!(
            text(Keysym::Return, Some("\r")),
            Some(SharedString::from(Key::Return).to_string())
        );
        assert_eq!(
            text(Keysym::BackSpace, Some("\u{8}")),
            Some(SharedString::from(Key::Backspace).to_string())
        );
        assert_eq!(
            text(Keysym::Escape, None),
            Some(SharedString::from(Key::Escape).to_string())
        );
        assert_eq!(text(Keysym::a, Some("a")), Some("a".to_string()));
        // An Arabic layout delivers the letter as text.
        assert_eq!(text(Keysym::Arabic_meem, Some("م")), Some("م".to_string()));
        // Keys without text (F1, a dead key) send nothing.
        assert_eq!(text(Keysym::F1, None), None);
        assert_eq!(text(Keysym::F1, Some("\u{1b}")), None);
    }

    #[test]
    fn pointer_buttons() {
        assert_eq!(button(0x110), PointerEventButton::Left);
        assert_eq!(button(0x111), PointerEventButton::Right);
        assert_eq!(button(0x112), PointerEventButton::Middle);
        assert_eq!(button(0x113), PointerEventButton::Other);
    }
}
