//! TinyHand for Linux: a tiny hand on a stick that points at things on your screen.
//!
//! A Wayland layer-shell overlay covers each output while the pointer is on. It swallows
//! clicks (they become taps), draws ink while dragging, and shows a small toolbar.

mod arrow;
mod cursor;
mod ipc;
mod render;
mod tray;

use ipc::{Cmd, Settings};
use render::{Action, Layout, Library, Stroke, ToolbarState, LASER};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{channel, EventLoop},
        calloop_wayland_source::WaylandSource,
        client::{
            globals::registry_queue_init,
            protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
            Connection, QueueHandle,
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::path::PathBuf;
use std::time::Instant;
use tiny_skia::PixmapMut;

const BTN_LEFT: u32 = 0x110;

/// Everything about one screen's overlay that isn't Wayland plumbing. Units are buffer pixels.
struct View {
    w: f32,
    h: f32,
    k: f32,
    cursor: Option<(f32, f32)>,
    grip_x: Option<f32>,
    press: f32,
    held: bool,
    strokes: Vec<Stroke>,
    drawing: bool,
    down_at: Option<(f32, f32)>,
    popup: bool,
    hover: Option<usize>,
    over_ui: bool,
    /// Click-through: the pointer belongs to the apps underneath, except over the toolbar.
    through: bool,
    last: Instant,
    animating: bool,
}

impl View {
    fn new() -> View {
        View {
            w: 0.0,
            h: 0.0,
            k: 1.0,
            cursor: None,
            grip_x: None,
            press: 0.0,
            held: false,
            strokes: Vec::new(),
            drawing: false,
            down_at: None,
            popup: false,
            hover: None,
            over_ui: false,
            through: false,
            last: Instant::now(),
            animating: false,
        }
    }

    fn release(&mut self) {
        self.held = false;
        self.down_at = None;
        if self.drawing {
            self.drawing = false;
            if let Some(s) = self.strokes.last_mut() {
                s.ended = Some(Instant::now());
            }
        }
    }

    /// Advances the animation and paints a frame. Returns true if another frame is wanted.
    fn paint(&mut self, pm: &mut PixmapMut, lib: &mut Library, set: &Settings, pinned: &[String]) -> bool {
        let now = Instant::now();
        // Upstream's easing constants are per tick of a 120 Hz timer.
        let dt = if self.animating { now.duration_since(self.last).as_secs_f32().min(0.1) } else { 1.0 / 60.0 };
        let ticks = dt * 120.0;
        self.last = now;
        let mut animating = false;

        if !self.held {
            self.press *= 0.82f32.powf(ticks);
            if self.press < 0.002 {
                self.press = 0.0;
            }
        }
        animating |= self.press > 0.0;

        let keep = set.fade_seconds <= 0.0;
        let hold = set.fade_seconds.max(0.5);
        if !keep {
            self.strokes.retain(|s| s.alpha(now, hold).is_some());
        }
        for s in &self.strokes {
            let alpha = if keep { 1.0 } else { s.alpha(now, hold).unwrap_or(0.0) };
            s.draw(pm, alpha, self.k);
            animating |= !keep && s.ended.is_some();
        }

        let layout = Layout::new(self.w, self.h, self.k, pinned, self.popup);
        let st = ToolbarState { style: &set.style, sound_on: set.sound_on, through: self.through, popup: self.popup, hover: self.hover };
        render::draw_toolbar(pm, lib, &layout, &st, self.k);

        if let Some(c) = self.cursor {
            let target = (c.0 + 0.30 * self.w).clamp(0.15 * self.w, 1.05 * self.w);
            let gx = match self.grip_x {
                Some(g) => g + (target - g) * (1.0 - 0.92f32.powf(ticks)),
                None => target,
            };
            self.grip_x = Some(gx);
            animating |= (target - gx).abs() > 0.5;
            if !self.over_ui {
                if set.style == LASER {
                    render::draw_laser(pm, c, self.h, set.size, self.press);
                } else if let Some(hand) = lib.hand(&set.style) {
                    let pose = render::pose(&hand.m, c, gx, self.h, set.size, self.press);
                    render::draw_hand(pm, hand, &pose, set.shadow);
                }
            }
        }
        self.animating = animating;
        animating
    }
}

struct Overlay {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    size: (u32, u32),
    scale: i32,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    /// The (click-through, popup) pair the surface's input region was last set for.
    region_for: Option<(bool, bool)>,
    view: View,
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    conn: Connection,
    qh: QueueHandle<App>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<ThemedPointer>,
    mods: Modifiers,
    overlays: Vec<Overlay>,
    lib: Library,
    settings: Settings,
    pinned: Vec<String>,
    tray: Option<tray::Handle>,
    through: bool,
    tracker: Option<cursor::Tracker>,
    arrow: arrow::ArrowHider,
    shift_down: Option<Instant>,
    last_shift_tap: Option<Instant>,
    exit: bool,
}

impl App {
    fn active(&self) -> bool {
        !self.overlays.is_empty()
    }

    fn activate(&mut self) {
        if self.active() {
            return;
        }
        self.lib.load_thumbs();
        for output in self.output_state.outputs() {
            let surface = self.compositor.create_surface(&self.qh);
            let layer = self.layer_shell.create_layer_surface(
                &self.qh,
                surface,
                Layer::Overlay,
                Some("tinyhand"),
                Some(&output),
            );
            layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            layer.set_size(0, 0);
            layer.set_exclusive_zone(-1); // cover panels too
            layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
            layer.commit();
            let scale = self.output_state.info(&output).map(|i| i.scale_factor).unwrap_or(1).max(1);
            self.overlays.push(Overlay {
                layer,
                output,
                size: (0, 0),
                scale,
                configured: false,
                frame_pending: false,
                dirty: false,
                region_for: None,
                view: View::new(),
            });
        }
    }

    fn deactivate(&mut self) {
        self.overlays.clear();
    }

    fn tracking(&self) -> bool {
        self.tracker.as_ref().is_some_and(|t| t.loaded())
    }

    /// Brings the overlays and the cursor feed in line with `through` and whether we're on.
    fn apply_mode(&mut self) {
        let through = self.through;
        let want = through && self.active();
        if let Some(t) = &mut self.tracker {
            if want {
                if let Err(e) = t.start() {
                    eprintln!("tinyhand: can't follow the cursor in click-through ({e}); the hand is hidden instead");
                }
            } else {
                t.stop();
            }
        }
        let tracking = self.tracking();
        // Only hide the arrow when the hand is really there to stand in for it.
        self.arrow.set(want && tracking && self.settings.hide_arrow);
        for ov in &mut self.overlays {
            // Tapping needs the keyboard for Esc; in click-through, typing belongs to the apps.
            ov.layer.set_keyboard_interactivity(if through {
                KeyboardInteractivity::OnDemand
            } else {
                KeyboardInteractivity::Exclusive
            });
            if through != ov.view.through {
                ov.view.through = through;
                ov.view.release();
                if through && !tracking {
                    ov.view.cursor = None;
                }
            }
        }
    }

    /// A cursor position from the KWin script, in global compositor coordinates.
    fn cursor_moved(&mut self, x: f32, y: f32) {
        if !self.through {
            return;
        }
        for i in 0..self.overlays.len() {
            let ov = &mut self.overlays[i];
            let info = self.output_state.info(&ov.output);
            let (ox, oy) = info.as_ref().and_then(|i| i.logical_position).unwrap_or((0, 0));
            let (lx, ly) = (x - ox as f32, y - oy as f32);
            let inside = lx >= 0.0 && ly >= 0.0 && lx < ov.size.0 as f32 && ly < ov.size.1 as f32;
            let k = ov.scale as f32;
            ov.view.cursor = inside.then_some((lx * k, ly * k));
            self.redraw(i);
        }
    }

    /// Publishes the current state to the tray and state.json, and repaints.
    fn sync(&mut self) {
        let (active, sound_on, through) = (self.active(), self.settings.sound_on, self.through);
        ipc::write_state(active, &self.settings.style, sound_on, through);
        if let Some(t) = &self.tray {
            t.update(|t| {
                t.active = active;
                t.sound_on = sound_on;
                t.through = through;
            });
        }
        for i in 0..self.overlays.len() {
            self.redraw(i);
        }
    }

    fn set_style(&mut self, id: String) {
        if !self.pinned.contains(&id) {
            // Show a hand picked from the grid in the last hand slot of the bar.
            if let Some(slot) = self.pinned.iter().rposition(|p| p != LASER) {
                self.pinned[slot] = id.clone();
            }
        }
        self.settings.style = id;
        self.settings.save();
        for ov in &mut self.overlays {
            ov.view.popup = false;
        }
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::On => self.activate(),
            Cmd::Off => self.deactivate(),
            Cmd::Toggle => {
                if self.active() {
                    self.deactivate()
                } else {
                    self.activate()
                }
            }
            Cmd::Clear => self.clear_ink(),
            Cmd::Style(id) => {
                self.set_style(id);
                self.activate();
            }
            Cmd::Sound(v) => {
                self.settings.sound_on = v.unwrap_or(!self.settings.sound_on);
                self.settings.save();
            }
            Cmd::Through(v) => self.through = v.unwrap_or(!self.through),
            Cmd::Quit => self.exit = true,
        }
        self.apply_mode();
        self.sync();
    }

    fn clear_ink(&mut self) {
        for ov in &mut self.overlays {
            ov.view.strokes.clear();
            ov.view.drawing = false;
        }
    }

    fn act(&mut self, i: usize, action: Action) {
        match action {
            Action::Style(id) => self.set_style(id),
            Action::More => {
                if let Some(ov) = self.overlays.get_mut(i) {
                    ov.view.popup = !ov.view.popup;
                }
            }
            Action::Sound => {
                self.settings.sound_on = !self.settings.sound_on;
                self.settings.save();
            }
            Action::Through => self.through = !self.through,
            Action::Exit => self.deactivate(),
        }
        self.apply_mode();
        self.sync();
    }

    fn tap_sound(&self) {
        if !self.settings.sound_on {
            return;
        }
        let path = self.lib.tap_sound();
        std::thread::spawn(move || {
            for player in ["pw-play", "paplay", "aplay"] {
                let child = std::process::Command::new(player)
                    .arg(&path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                if let Ok(mut child) = child {
                    let _ = child.wait();
                    break;
                }
            }
        });
    }

    fn redraw(&mut self, i: usize) {
        let Some(ov) = self.overlays.get_mut(i) else { return };
        ov.dirty = true;
        if ov.configured && !ov.frame_pending {
            self.draw(i);
        }
    }

    fn draw(&mut self, i: usize) {
        let App { overlays, pool, lib, settings, pinned, qh, compositor, .. } = self;
        let Some(ov) = overlays.get_mut(i) else { return };
        let (pw, ph) = (ov.size.0 as i32 * ov.scale, ov.size.1 as i32 * ov.scale);
        if pw <= 0 || ph <= 0 {
            return;
        }
        let Ok((buffer, canvas)) = pool.create_buffer(pw, ph, pw * 4, wl_shm::Format::Argb8888) else {
            return;
        };
        let canvas = &mut canvas[..(pw * ph * 4) as usize];
        canvas.fill(0);
        let Some(mut pm) = PixmapMut::from_bytes(canvas, pw as u32, ph as u32) else { return };
        ov.view.w = pw as f32;
        ov.view.h = ph as f32;
        ov.view.k = ov.scale as f32;
        ov.view.paint(&mut pm, lib, settings, pinned);

        let surface = ov.layer.wl_surface();
        let mode = (ov.view.through, ov.view.popup);
        if ov.region_for != Some(mode) {
            ov.region_for = Some(mode);
            match (mode.0, Region::new(compositor)) {
                (true, Ok(region)) => {
                    // Only the toolbar takes the pointer; everything else falls through.
                    let k = ov.view.k;
                    let layout = Layout::new(ov.view.w, ov.view.h, k, pinned, ov.view.popup);
                    for r in [Some(layout.bar), layout.grid].into_iter().flatten() {
                        let (x, y) = ((r.x / k).floor(), (r.y / k).floor());
                        let (w, h) = (((r.x + r.w) / k).ceil() - x, ((r.y + r.h) / k).ceil() - y);
                        region.add(x as i32, y as i32, w as i32, h as i32);
                    }
                    surface.set_input_region(Some(region.wl_region()));
                }
                _ => surface.set_input_region(None),
            }
        }
        surface.set_buffer_scale(ov.scale);
        surface.damage_buffer(0, 0, pw, ph);
        surface.frame(qh, surface.clone());
        let _ = buffer.attach_to(surface);
        ov.layer.commit();
        ov.frame_pending = true;
        ov.dirty = false;
    }

    fn overlay_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.overlays.iter().position(|o| o.layer.wl_surface() == surface)
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        if let Some(i) = self.overlay_of(surface) {
            if self.overlays[i].scale != factor && factor > 0 {
                self.overlays[i].scale = factor;
                self.redraw(i);
            }
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _time: u32) {
        if let Some(i) = self.overlay_of(surface) {
            let ov = &mut self.overlays[i];
            ov.frame_pending = false;
            if ov.dirty || ov.view.animating {
                self.draw(i);
            }
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.overlays.retain(|o| o.output != output);
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.overlays.retain(|o| &o.layer != layer);
        self.sync();
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _serial: u32) {
        let Some(i) = self.overlays.iter().position(|o| &o.layer == layer) else { return };
        let ov = &mut self.overlays[i];
        ov.size = configure.new_size;
        ov.region_for = None;
        ov.configured = true;
        ov.frame_pending = false;
        self.draw(i);
        if i == 0 {
            self.sync();
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme(qh, &seat, self.shm.wl_shm(), surface, ThemeSpec::default())
                .ok();
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if !matches!(event.keysym, Keysym::Shift_L | Keysym::Shift_R) {
            (self.shift_down, self.last_shift_tap) = (None, None);
        }
        let chord = self.mods.ctrl && self.mods.alt;
        if event.keysym == Keysym::Escape || (chord && event.keysym == Keysym::h) {
            self.command(Cmd::Off);
        } else if chord && event.keysym == Keysym::x {
            self.command(Cmd::Clear);
        }
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}

    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _serial: u32, modifiers: Modifiers, _layout: u32) {
        self.mods = modifiers;
        // Double-tap Shift flips click-through, like upstream.
        let m = modifiers;
        let now = Instant::now();
        if m.shift && !(m.ctrl || m.alt || m.logo) {
            self.shift_down.get_or_insert(now);
        } else if m.ctrl || m.alt || m.logo {
            (self.shift_down, self.last_shift_tap) = (None, None);
        } else if let Some(down) = self.shift_down.take() {
            if now.duration_since(down).as_secs_f32() >= 0.3 {
                self.last_shift_tap = None;
            } else if self.last_shift_tap.take().is_some_and(|t| down.duration_since(t).as_secs_f32() < 0.35) {
                self.command(Cmd::Through(None));
                return;
            } else {
                self.last_shift_tap = Some(now);
            }
        }
        // Holding Ctrl (or Super) while drawing puts an arrowhead on the line.
        let arrow = modifiers.ctrl || modifiers.logo;
        for i in 0..self.overlays.len() {
            let v = &mut self.overlays[i].view;
            if v.drawing {
                if let Some(s) = v.strokes.last_mut() {
                    s.arrow = arrow;
                }
                self.redraw(i);
            }
        }
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, conn: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        let mut action = None;
        let mut tapped = false;
        for event in events {
            let Some(i) = self.overlay_of(&event.surface) else { continue };
            let follows = self.through && self.tracking();
            let App { overlays, pinned, settings, pointer, mods, through, .. } = self;
            let ov = &mut overlays[i];
            let v = &mut ov.view;
            let p = (event.position.0 as f32 * v.k, event.position.1 as f32 * v.k);
            let layout = Layout::new(v.w, v.h, v.k, pinned, v.popup);
            ov.dirty = true;
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    let entered = matches!(event.kind, PointerEventKind::Enter { .. });
                    v.cursor = Some(p);
                    // Mid-drag, the toolbar doesn't interrupt the line.
                    let over = !v.held && layout.over_ui(p);
                    v.hover = if over { layout.hit(p) } else { None };
                    if over != v.over_ui || entered {
                        v.over_ui = over;
                        if let Some(ptr) = pointer {
                            // In click-through the arrow may be hidden system-wide, so the toolbar
                            // gets the link hand, which is always visible.
                            let icon = if *through { CursorIcon::Pointer } else { CursorIcon::Default };
                            let _ = if over { ptr.set_cursor(conn, icon) } else { ptr.hide_cursor() };
                        }
                    }
                    if let (true, Some(start)) = (v.held, v.down_at) {
                        let arrow = mods.ctrl || mods.logo;
                        if !v.drawing {
                            if (p.0 - start.0).hypot(p.1 - start.1) >= 4.0 * v.k {
                                v.strokes.push(Stroke {
                                    pts: vec![start, p],
                                    color: render::ink_color(&settings.style),
                                    width: (v.h * 0.0055).max(3.0 * v.k),
                                    arrow,
                                    ended: None,
                                });
                                v.drawing = true;
                            }
                        } else if let Some(s) = v.strokes.last_mut() {
                            let last = *s.pts.last().unwrap_or(&start);
                            if (p.0 - last.0).hypot(p.1 - last.1) >= 1.5 {
                                s.pts.push(p);
                            }
                            s.arrow = arrow;
                        }
                    }
                }
                PointerEventKind::Leave { .. } => {
                    // In click-through the pointer has only left the toolbar; KWin keeps
                    // telling us where it is.
                    if !follows {
                        v.cursor = None;
                    }
                    v.hover = None;
                    v.over_ui = false;
                    v.release();
                }
                PointerEventKind::Press { button: BTN_LEFT, .. } => {
                    if layout.over_ui(p) {
                        action = layout.hit(p).map(|h| (i, layout.items[h].1.clone()));
                    } else if !*through {
                        v.popup = false;
                        v.held = true;
                        v.press = 1.0;
                        v.down_at = Some(p);
                        tapped = true;
                    }
                }
                PointerEventKind::Release { button: BTN_LEFT, .. } => v.release(),
                _ => {}
            }
        }
        if tapped {
            (self.shift_down, self.last_shift_tap) = (None, None);
            self.tap_sound();
        }
        if let Some((i, action)) = action {
            self.act(i, action);
        }
        for i in 0..self.overlays.len() {
            if self.overlays[i].dirty {
                self.redraw(i);
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

fn assets_dir() -> Result<PathBuf, String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("TINYHAND_ASSETS") {
        dirs.push(d.into());
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(exe.join("assets"));
        dirs.push(exe.join("../share/tinyhand/assets"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/tinyhand/assets"));
    }
    dirs.push("/usr/share/tinyhand/assets".into());
    dirs.push(concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into());
    dirs.into_iter()
        .find(|d| d.join("hands.json").exists())
        .ok_or_else(|| "can't find the assets folder (set TINYHAND_ASSETS)".to_string())
}

fn run(initial: Option<Cmd>) -> Result<(), String> {
    let lib = Library::open(assets_dir()?)?;
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland session: {e}"))?;
    let (globals, mut event_queue) = registry_queue_init::<App>(&conn).map_err(|e| e.to_string())?;
    let qh = event_queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|_| "this compositor doesn't support wlr-layer-shell (GNOME doesn't; KDE, Sway, Hyprland do)")?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("wl_shm: {e}"))?;
    let pool = SlotPool::new(1920 * 1080 * 4 * 2, &shm).map_err(|e| e.to_string())?;

    let mut event_loop: EventLoop<App> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let (tx, rx) = channel::channel::<Cmd>();
    event_loop
        .handle()
        .insert_source(rx, |event, _, app| {
            if let channel::Event::Msg(cmd) = event {
                app.command(cmd);
            }
        })
        .map_err(|e| e.to_string())?;

    let ipc_tx = tx.clone();
    ipc::listen(move |cmd| {
        let _ = ipc_tx.send(cmd);
    })
    .map_err(|e| format!("control socket: {e}"))?;

    // Ctrl-C or a plain kill must still put the cursor theme back.
    let signal_tx = std::sync::Mutex::new(tx.clone());
    let _ = ctrlc::set_handler(move || {
        if let Ok(tx) = signal_tx.lock() {
            let _ = tx.send(Cmd::Quit);
        }
    });

    let settings = Settings::load();
    let icon = lib
        .icon(64)
        .map(|(size, data)| vec![ksni::Icon { width: size as i32, height: size as i32, data }])
        .unwrap_or_default();
    let tray = tray::spawn(tray::HandTray { tx: tx.clone(), active: false, sound_on: settings.sound_on, through: false, icon });

    let (cursor_tx, cursor_rx) = channel::channel::<(f32, f32)>();
    event_loop
        .handle()
        .insert_source(cursor_rx, |event, _, app| {
            if let channel::Event::Msg((x, y)) = event {
                app.cursor_moved(x, y);
            }
        })
        .map_err(|e| e.to_string())?;
    let cursor_tx = std::sync::Mutex::new(cursor_tx);
    let tracker = cursor::Tracker::new(move |x, y| {
        if let Ok(tx) = cursor_tx.lock() {
            let _ = tx.send((x, y));
        }
    })
    .map_err(|e| eprintln!("tinyhand: no session bus ({e}); click-through won't show the hand"))
    .ok();

    let mut pinned: Vec<String> = render::DEFAULT_PINNED.iter().map(|s| s.to_string()).collect();
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        conn: conn.clone(),
        qh,
        keyboard: None,
        pointer: None,
        mods: Modifiers::default(),
        overlays: Vec::new(),
        lib,
        settings,
        pinned: Vec::new(),
        tray,
        through: false,
        tracker,
        arrow: arrow::ArrowHider::new(),
        shift_down: None,
        last_shift_tap: None,
        exit: false,
    };
    if !pinned.contains(&app.settings.style) {
        pinned[4] = app.settings.style.clone();
    }
    app.pinned = pinned;

    // Learn about outputs and seats before the first command runs.
    for _ in 0..2 {
        event_queue.roundtrip(&mut app).map_err(|e| e.to_string())?;
    }
    WaylandSource::new(conn, event_queue).insert(event_loop.handle()).map_err(|e| e.to_string())?;
    app.sync();
    if let Some(cmd) = initial {
        app.command(cmd);
    }

    while !app.exit {
        event_loop.dispatch(None, &mut app).map_err(|e| e.to_string())?;
        let _ = app.conn.flush();
    }
    app.tracker = None; // unloads the KWin script
    app.arrow.set(false);
    ipc::cleanup();
    Ok(())
}

/// Paints one frame to a PNG without touching Wayland, to eyeball the renderer.
fn render_test(out: &str, style: &str) -> Result<(), String> {
    let mut lib = Library::open(assets_dir()?)?;
    lib.load_thumbs();
    let settings = Settings { style: style.to_string(), sound_on: true, size: 0.13, shadow: true, fade_seconds: 3.0, hide_arrow: true };
    let pinned: Vec<String> = render::DEFAULT_PINNED.iter().map(|s| s.to_string()).collect();
    let (w, h) = (1920u32, 1080u32);
    let mut pix = tiny_skia::Pixmap::new(w, h).ok_or("pixmap")?;
    pix.fill(tiny_skia::Color::from_rgba8(235, 232, 226, 255));
    let mut view = View::new();
    (view.w, view.h) = (w as f32, h as f32);
    view.cursor = Some((760.0, 430.0));
    view.popup = true;
    let pts = (0..60).map(|i| (300.0 + i as f32 * 7.0, 300.0 + (i as f32 / 6.0).sin() * 60.0)).collect();
    view.strokes.push(Stroke { pts, color: render::ink_color(style), width: 6.0, arrow: true, ended: None });
    view.paint(&mut pix.as_mut(), &mut lib, &settings, &pinned);
    for px in pix.data_mut().chunks_exact_mut(4) {
        px.swap(0, 2); // the renderer paints BGRA
    }
    pix.save_png(out).map_err(|e| e.to_string())
}

const USAGE: &str = "TinyHand: a tiny hand on a stick that points at things on your screen

usage: tinyhand [command]

  (none)                 start TinyHand in the tray
  on | off | toggle      turn the pointer on, off, or flip it
  clear                  clear the drawing
  style <id>             switch to a hand (or `laser`) and turn the pointer on
  sound on|off|toggle    tap sound
  clickthrough on|off|toggle
                         let clicks reach the apps underneath; the hand stays
  status                 print the state file
  quit                   stop TinyHand

tinyhand://… links work too, e.g. `tinyhand tinyhand://style/pink`.
Commands go to the running instance; if there is none, one is started.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("status") => {
            match std::fs::read_to_string(ipc::state_path()) {
                Ok(text) => print!("{text}"),
                Err(_) => println!("TinyHand is not running"),
            }
            Ok(())
        }
        Some("render-test") => render_test(
            args.get(1).map(String::as_str).unwrap_or("tinyhand-test.png"),
            args.get(2).map(String::as_str).unwrap_or("lavender"),
        ),
        None => match ipc::send("") {
            Ok(_) => {
                println!("TinyHand is already running");
                Ok(())
            }
            Err(_) => run(None),
        },
        Some(_) => match ipc::parse(&args) {
            Err(e) => Err(e),
            Ok(cmd) => match ipc::send(&args.join(" ")) {
                Ok(reply) if reply == "ok" => Ok(()),
                Ok(reply) => Err(reply),
                Err(_) if matches!(cmd, Cmd::Off | Cmd::Quit | Cmd::Clear) => Ok(()),
                Err(_) => run(Some(cmd)),
            },
        },
    };
    if let Err(e) = result {
        eprintln!("tinyhand: {e}");
        std::process::exit(1);
    }
}
