// SPDX-License-Identifier: GPL-2.0-or-later
//! The compositor state: Wayland globals and their handlers, the app list,
//! what is visible, and input dispatch.

use super::backend::Backend;
use super::input::{Inputs, Kind};
use super::pads::Pads;
use crate::apps::{Apps, LaunchEnv};
use crate::config::{parse_volume, Config, GamepadMode};
use crate::osk::{self, Osk, OskOut, PadKey, Tap};
use crate::overlay::{self, Action, LauncherInfo, Overlay, RunningInfo, View};
use crate::router::{Receiver, Routed, Router};

use mortar_ui::gfx::{Fonts, Rect, Ui};
use mortar_ui::input::{codes, map_evdev_key, Button, Repeater};
use mortar_ui::status::{Backlight, Sys};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{PopupKind, PopupManager, Window};
use smithay::input::keyboard::{FilterResult, KeyboardHandle, Keycode, XkbConfig};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Buffer as BufferCoords, Rectangle, Serial, Size, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler, CompositorState,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell};

use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Options the compositor was started with.
pub struct Opts {
    pub backend: String,
    pub config: Config,
    pub socket: Option<String>,
    pub ctl: Option<PathBuf>,
    pub direct_input: bool,
    pub size: (i32, i32),
}

pub struct Shell {
    pub dh: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Shell>,
    pub signal: LoopSignal,
    pub socket_name: String,
    pub start: Instant,
    pub running: bool,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    #[allow(dead_code)] // keeps the xdg-output global alive
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Shell>,
    pub data_device_state: DataDeviceState,
    pub dmabuf_state: DmabufState,
    #[allow(dead_code)]
    pub dmabuf_global: Option<DmabufGlobal>,
    #[allow(dead_code)]
    pub seat: Seat<Shell>,
    pub keyboard: KeyboardHandle<Shell>,
    pub popups: PopupManager,

    pub output: Output,
    pub size: (i32, i32),
    pub backend: Backend,

    pub cfg: Config,
    pub apps: Apps<Window>,
    /// Launched by a user action and should be shown when it maps.
    pub pending_focus: Option<u32>,
    pub overlay: Overlay,
    pub osk: Osk,
    pub router: Router,
    pub pads: Pads,
    pub inputs: Inputs,
    pub inputs_removed: Vec<PathBuf>,
    /// Opaque copies of visible app buffers (backend::opaque_copy).
    pub opaque: std::collections::HashMap<smithay::reexports::wayland_server::backend::ObjectId, super::backend::OpaqueCopy>,
    #[allow(dead_code)]
    pub viewporter_state: smithay::wayland::viewporter::ViewporterState,
    repeater: Repeater,
    /// Pad buttons for keyboard-mode apps that are currently pressed as keys
    /// (released when the receiver changes).
    held_keys: Vec<u16>,
    /// Logo key pressed with nothing else since: toggles the overlay on
    /// release.
    logo_alone: bool,

    pub ui: Ui,
    pub overlay_buf: MemoryRenderBuffer,
    pub osk_buf: MemoryRenderBuffer,
    pub osk_h: i32,
    pub osd_buf: Option<MemoryRenderBuffer>,
    pub osd_w: i32,
    osd_until: Option<Instant>,
    /// The shell screen (home/overlay) must be redrawn.
    pub ui_dirty: bool,
    pub osk_dirty: bool,
    /// Something on screen changed; the backend should render a frame.
    pub needs_redraw: bool,
    /// Screen content changed while nothing was rendered: shown/hidden
    /// windows need a reconfigure.
    pub sys: Sys,
    sys_at: Instant,
    pub volume: Option<u8>,
    last_bg_tick: Instant,
}

impl Shell {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dh: DisplayHandle,
        loop_handle: LoopHandle<'static, Shell>,
        signal: LoopSignal,
        socket_name: String,
        output: Output,
        size: (i32, i32),
        backend: Backend,
        cfg: Config,
        inputs: Inputs,
    ) -> Result<Shell, String> {
        let compositor_state = CompositorState::new::<Shell>(&dh);
        let xdg_shell_state = XdgShellState::new::<Shell>(&dh);
        let shm_state = ShmState::new::<Shell>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Shell>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Shell>(&dh);
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = backend.dmabuf_formats().map(|f| dmabuf_state.create_global::<Shell>(&dh, f));
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        let xkb = XkbConfig { layout: &cfg.keyboard_layout, ..Default::default() };
        let keyboard = seat.add_keyboard(xkb, 400, 30).map_err(|e| format!("keyboard: {e:?}"))?;
        output.create_global::<Shell>(&dh);

        let fonts = Fonts::load(&mortar_ui::gfx::font_dir())?;
        let ui = Ui::new(size.0 as u32, size.1 as u32, fonts);
        let osk_h = osk::height(&ui) as i32;
        let overlay_buf = opaque_buffer(size.0, size.1);
        let osk_buf = opaque_buffer(size.0, osk_h);

        let viewporter_dh = dh.clone();
        Ok(Shell {
            dh,
            loop_handle,
            signal,
            socket_name,
            start: Instant::now(),
            running: true,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            dmabuf_state,
            dmabuf_global,
            seat,
            keyboard,
            popups: PopupManager::default(),
            output,
            size,
            backend,
            cfg,
            apps: Apps::default(),
            pending_focus: None,
            overlay: Overlay::default(),
            osk: Osk::default(),
            router: Router::default(),
            pads: Pads::default(),
            inputs,
            inputs_removed: Vec::new(),
            opaque: Default::default(),
            viewporter_state: smithay::wayland::viewporter::ViewporterState::new::<Shell>(&viewporter_dh),
            repeater: Repeater::default(),
            held_keys: Vec::new(),
            logo_alone: false,
            ui,
            overlay_buf,
            osk_buf,
            osk_h,
            osd_buf: None,
            osd_w: 0,
            osd_until: None,
            ui_dirty: true,
            osk_dirty: true,
            needs_redraw: true,
            sys: Sys::read(),
            sys_at: Instant::now(),
            volume: None,
            last_bg_tick: Instant::now(),
        })
    }

    // ----------------------------------------------------- what's shown ----

    /// The shell's own screen covers everything (home or MENU overlay).
    pub fn shell_screen(&self) -> bool {
        self.overlay.visible || self.visible_app().is_none()
    }

    /// The app whose window is on screen (when no shell screen covers it).
    pub fn visible_app(&self) -> Option<u32> {
        self.apps.focused().filter(|u| self.apps.get(*u).is_some_and(|a| !a.windows.is_empty()))
    }

    /// The window shown for the visible app: its newest.
    pub fn visible_window(&self) -> Option<&Window> {
        self.apps.get(self.visible_app()?)?.windows.last()
    }

    fn receiver(&self) -> Receiver {
        if self.shell_screen() {
            return Receiver::Overlay;
        }
        match self.visible_app().and_then(|u| self.apps.get(u)) {
            Some(a) => Receiver::App(a.uid, a.gamepad),
            None => Receiver::Nobody,
        }
    }

    fn keyboard_app_visible(&self) -> bool {
        matches!(self.receiver(), Receiver::App(_, GamepadMode::Keyboard))
    }

    /// Area the visible app gets: the screen, minus the on-screen keyboard.
    pub fn app_area(&self) -> Size<i32, smithay::utils::Logical> {
        let h = if self.osk.visible { self.size.1 - self.osk_height() } else { self.size.1 };
        (self.size.0, h).into()
    }

    pub fn osk_height(&self) -> i32 {
        self.osk_h
    }

    /// Send every window its state: fullscreen at the app area, activated
    /// only when visible.
    pub fn configure_all(&mut self) {
        let vis = if self.shell_screen() { None } else { self.visible_window().cloned() };
        let full: Size<i32, smithay::utils::Logical> = self.size.into();
        let area = self.app_area();
        for a in &self.apps.list {
            let keyboard = a.gamepad == GamepadMode::Keyboard;
            for w in &a.windows {
                let Some(tl) = w.toplevel() else { continue };
                let active = vis.as_ref() == Some(w);
                tl.with_pending_state(|s| {
                    s.states.set(xdg_toplevel::State::Fullscreen);
                    // suspended: not on screen (SDL3 turns it into
                    // SDL_EVENT_WINDOW_OCCLUDED, so apps can idle)
                    if active {
                        s.states.set(xdg_toplevel::State::Activated);
                        s.states.unset(xdg_toplevel::State::Suspended);
                    } else {
                        s.states.unset(xdg_toplevel::State::Activated);
                        s.states.set(xdg_toplevel::State::Suspended);
                    }
                    s.size = Some(if keyboard && active { area } else { full });
                });
                if tl.is_initial_configure_sent() {
                    tl.send_pending_configure();
                }
            }
        }
    }

    /// Recompute focus after anything changed what is visible.
    pub fn refocus(&mut self) {
        let mut out = Vec::new();
        self.router.retarget(self.receiver(), &mut out);
        self.apply_routed(out, None);
        self.release_held_keys();
        if !self.keyboard_app_visible() && self.osk.visible {
            self.osk.visible = false;
        }
        let target = if self.shell_screen() {
            None
        } else {
            self.visible_window().and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()))
        };
        if self.keyboard.current_focus() != target {
            let serial = SERIAL_COUNTER.next_serial();
            let kb = self.keyboard.clone();
            kb.set_focus(self, target, serial);
        }
        self.configure_all();
        self.ui_dirty = true;
        self.needs_redraw = true;
    }

    pub fn show_app(&mut self, uid: u32) {
        self.apps.focus(uid);
        self.overlay.visible = false;
        self.overlay.clear_toast();
        self.refocus();
    }

    pub fn open_overlay(&mut self) {
        self.sys = Sys::read();
        self.volume = self.read_volume();
        let (running, launcher, current) = self.view_data();
        let v = make_view(&self.apps, &self.sys, self.volume, &running, &launcher, current.as_ref());
        self.overlay.open(&v);
        self.refocus();
    }

    pub fn hide_overlay(&mut self) {
        self.overlay.visible = false;
        self.refocus();
    }

    // ------------------------------------------------------ view data ----

    pub fn view_data(&self) -> (Vec<RunningInfo>, Vec<LauncherInfo>, Option<RunningInfo>) {
        let vis = self.visible_app();
        let running: Vec<RunningInfo> = self
            .apps
            .ordered()
            .into_iter()
            .filter_map(|u| self.apps.get(u))
            .map(|a| RunningInfo {
                uid: a.uid,
                name: a.name.clone(),
                icon: a.icon,
                visible: Some(a.uid) == vis,
                windows: a.windows.len(),
                closing: a.closing.is_some(),
            })
            .collect();
        let launcher = self
            .cfg
            .apps
            .iter()
            .map(|e| LauncherInfo {
                id: e.id.clone(),
                name: e.name.clone(),
                description: e.description.clone(),
                icon: e.icon,
                running: self.apps.by_entry(&e.id).is_some(),
            })
            .collect();
        let current = running.iter().find(|r| r.visible).cloned();
        (running, launcher, current)
    }


    // ------------------------------------------------------------ apps ----

    pub fn launch(&mut self, id: &str) -> Result<u32, String> {
        let e = self.cfg.app(id).cloned().ok_or(format!("unknown app '{id}'"))?;
        if e.single_instance
            && let Some(a) = self.apps.by_entry(id)
        {
            let uid = a.uid;
            if !a.windows.is_empty() {
                self.show_app(uid);
            } else {
                self.pending_focus = Some(uid);
            }
            return Ok(uid);
        }
        // the app's virtual pads must exist before it enumerates devices
        let slot_guess = (0..crate::apps::MAX_SLOTS).find(|s| !self.apps.list.iter().any(|a| a.slot == *s));
        if e.gamepad == GamepadMode::Forward
            && let Some(s) = slot_guess
        {
            self.pads.add_slot(s);
        }
        let extra: Vec<(String, String)> = self.cfg.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let env = LaunchEnv { wayland_display: &self.socket_name, extra };
        let pads = &self.pads;
        let uid = self.apps.launch(&e, &env, |slot| {
            let bl = pads.blacklist(slot);
            vec![
                ("SDL_JOYSTICK_BLACKLIST_DEVICES".into(), bl.clone()),
                ("SDL_GAMECONTROLLER_IGNORE_DEVICES".into(), bl),
            ]
        })?;
        tracing::info!(app = %e.name, uid, pid = ?self.apps.get(uid).and_then(|a| a.pid), "launched");
        self.pending_focus = Some(uid);
        self.ui_dirty = true;
        Ok(uid)
    }

    pub fn close_app(&mut self, uid: u32) {
        self.apps.close(uid, |w| {
            if let Some(t) = w.toplevel() {
                t.send_close();
            }
        });
        self.ui_dirty = true;
    }

    fn window_for_surface(&self, s: &WlSurface) -> Option<(u32, Window)> {
        for a in &self.apps.list {
            for w in &a.windows {
                if w.toplevel().is_some_and(|t| t.wl_surface() == s) {
                    return Some((a.uid, w.clone()));
                }
            }
        }
        None
    }

    // ------------------------------------------------------- periodic ----

    /// How long the event loop may sleep.
    pub fn timeout(&self) -> Duration {
        let mut t = Duration::from_millis(500);
        if let Some(r) = self.repeater.timeout() {
            t = t.min(r);
        }
        let now = Instant::now();
        if let Some(d) = self.overlay.deadline() {
            t = t.min(d.saturating_duration_since(now));
        }
        if let Some(d) = self.osd_until {
            t = t.min(d.saturating_duration_since(now));
        }
        if self.apps.list.len() > 1 && self.cfg.background_fps > 0 {
            t = t.min(Duration::from_millis(1000 / self.cfg.background_fps as u64));
        }
        t
    }

    pub fn tick(&mut self) {
        // input hot-unplug
        for p in std::mem::take(&mut self.inputs_removed) {
            super::input::close_device(self, &p);
        }
        // repeats for overlay / OSK navigation
        let mut evs = Vec::new();
        self.repeater.poll(&mut evs);
        for e in evs {
            if let mortar_ui::input::Event::Press { button, .. } = e {
                self.nav_press(button, true);
            }
        }
        // app lifecycle
        let had_vis = self.visible_app();
        let gone = self.apps.tick();
        for uid in &gone {
            tracing::info!(uid, "app gone");
        }
        if !gone.is_empty() {
            let used: Vec<u8> = self.apps.list.iter().map(|a| a.slot).collect();
            for s in 0..crate::apps::MAX_SLOTS {
                if !used.contains(&s) {
                    self.pads.remove_slot(s);
                }
            }
            if self.pending_focus.is_some_and(|p| gone.contains(&p)) {
                self.pending_focus = None;
            }
            self.ui_dirty = true;
            if had_vis.is_some_and(|v| gone.contains(&v)) {
                self.refocus();
            }
        }
        // toasts / OSD
        if self.overlay.expire() {
            self.ui_dirty = true;
        }
        if self.osd_until.is_some_and(|t| Instant::now() >= t) {
            self.osd_until = None;
            self.osd_buf = None;
            self.needs_redraw = true;
        }
        // status while a shell screen shows
        if self.shell_screen() && self.sys_at.elapsed() > Duration::from_secs(2) {
            let new = Sys::read();
            if format!("{new:?}") != format!("{:?}", self.sys) {
                self.ui_dirty = true;
            }
            self.sys = new;
            self.sys_at = Instant::now();
        }
        // background apps: throttled frame callbacks
        if self.cfg.background_fps > 0
            && self.last_bg_tick.elapsed() >= Duration::from_millis(1000 / self.cfg.background_fps as u64)
        {
            self.last_bg_tick = Instant::now();
            let vis = if self.shell_screen() { None } else { self.visible_window().cloned() };
            let t = self.start.elapsed();
            for a in &self.apps.list {
                for w in &a.windows {
                    if Some(w) != vis.as_ref() {
                        w.send_frame(&self.output, t, Some(Duration::ZERO), |_, _| None);
                    }
                }
            }
        }
        self.redraw_ui();
        if self.needs_redraw {
            super::backend::render(self);
        }
    }

    /// Redraw the shell's own screens into their buffers when dirty.
    fn redraw_ui(&mut self) {
        if self.shell_screen() && self.ui_dirty {
            self.ui_dirty = false;
            let (running, launcher, current) = self.view_data();
            let v = make_view(&self.apps, &self.sys, self.volume, &running, &launcher, current.as_ref());
            self.overlay.draw(&mut self.ui, &v);
            let full = Rect::new(0.0, 0.0, self.ui.width(), self.ui.height());
            upload(&mut self.overlay_buf, &self.ui, full);
            self.needs_redraw = true;
            self.osk_dirty = true; // the canvas was overwritten
        }
        if self.osk.visible && self.osk_dirty && !self.shell_screen() {
            self.osk_dirty = false;
            let h = self.osk_height() as f32;
            let area = Rect::new(0.0, self.ui.height() - h, self.ui.width(), h);
            self.osk.draw(&mut self.ui, area);
            upload(&mut self.osk_buf, &self.ui, area);
            self.ui_dirty = true;
            self.needs_redraw = true;
        }
    }

    fn osd(&mut self, text: &str) {
        if self.shell_screen() {
            self.ui_dirty = true;
            return;
        }
        let r = overlay::draw_osd(&mut self.ui, text).snap();
        let mut buf = opaque_buffer(r.w as i32, r.h as i32);
        upload(&mut buf, &self.ui, r);
        self.osd_buf = Some(buf);
        self.osd_w = r.w as i32;
        self.osd_until = Some(Instant::now() + Duration::from_millis(1500));
        self.ui_dirty = true;
        self.osk_dirty = true;
        self.needs_redraw = true;
    }

    // ---------------------------------------------------------- input ----

    /// A key event from a classified device.
    pub fn on_key(&mut self, kind: Kind, code: u16, value: i32) {
        // volume keys work everywhere
        if code == codes::KEY_VOLUMEUP || code == codes::KEY_VOLUMEDOWN {
            if value != 0 {
                self.do_action(Action::Volume(if code == codes::KEY_VOLUMEUP { 5 } else { -5 }));
            }
            return;
        }
        match kind {
            Kind::Pad(pad) => {
                let rx = self.receiver();
                let out = self.router.key(code, value, rx);
                self.apply_routed(out, Some(pad));
            }
            Kind::Keyboard => self.keyboard_key(code, value),
            Kind::Keys => {}
        }
    }

    /// Hats (USB pads): forwarded to the visible app, or D-pad presses for
    /// the overlay.
    pub fn on_abs(&mut self, kind: Kind, code: u16, value: i32) {
        let Kind::Pad(pad) = kind else { return };
        match self.receiver() {
            Receiver::App(uid, GamepadMode::Forward) => {
                if let Some(a) = self.apps.get(uid) {
                    self.pads.emit(a.slot, pad, evdev::EventType::ABSOLUTE, code, value);
                }
            }
            Receiver::Overlay if code == 0x10 || code == 0x11 => {
                let b = match (code, value.signum()) {
                    (0x10, -1) => Button::Left,
                    (0x10, 1) => Button::Right,
                    (0x11, -1) => Button::Up,
                    (0x11, 1) => Button::Down,
                    _ => {
                        self.repeater = Repeater::default();
                        return;
                    }
                };
                self.repeater.press(b);
                self.nav_press(b, false);
            }
            _ => {}
        }
    }

    /// Inject a pad button (control socket, tests): as if from pad 0.
    pub fn inject_button(&mut self, b: Button, value: i32) {
        if let Some(code) = button_code(b) {
            self.on_key(Kind::Pad(0), code, value);
        }
    }

    fn apply_routed(&mut self, out: Vec<Routed>, pad: Option<u16>) {
        for r in out {
            match r {
                Routed::ToggleOverlay => {
                    if self.overlay.visible {
                        self.hide_overlay();
                    } else if self.visible_app().is_some() {
                        self.open_overlay();
                    } else {
                        // home: MENU flips Launch/Settings
                        self.nav_press(Button::Menu, false);
                    }
                }
                Routed::Overlay(b, down) => {
                    if down {
                        self.repeater.press(b);
                        self.nav_press(b, false);
                    } else {
                        self.repeater.release(b);
                    }
                }
                Routed::Forward(uid, code, value) => {
                    if let (Some(a), Some(pad)) = (self.apps.get(uid), pad) {
                        self.pads.emit(a.slot, pad, evdev::EventType::KEY, code, value);
                    }
                }
                Routed::ReleaseAll(uid, codes) => {
                    if let Some(a) = self.apps.get(uid) {
                        self.pads.release_all(a.slot, &codes);
                    }
                }
                Routed::Keys(_, b, down) => self.keys_mode(b, down),
            }
        }
    }

    /// Navigation press for the overlay or the OSK (`repeat` from the
    /// software repeater).
    fn nav_press(&mut self, b: Button, repeat: bool) {
        if self.shell_screen() {
            let (running, launcher, current) = self.view_data();
            let v = make_view(&self.apps, &self.sys, self.volume, &running, &launcher, current.as_ref());
            let act = self.overlay.button(b, &v);
            tracing::debug!(?b, ?act, "overlay");
            self.ui_dirty = true;
            if !repeat || matches!(act, Action::Brightness(_) | Action::Volume(_)) {
                self.do_action(act);
            }
        } else if self.osk.visible {
            self.osk_key(b);
        }
    }

    fn do_action(&mut self, a: Action) {
        match a {
            Action::None => {}
            Action::Hide => self.hide_overlay(),
            Action::Focus(uid) => self.show_app(uid),
            Action::Close(uid) => self.close_app(uid),
            Action::Launch(id) => match self.launch(&id) {
                Ok(uid) => {
                    // the new window takes over when it maps
                    if self.visible_app() != Some(uid) {
                        let name = self.cfg.app(&id).map(|e| e.name.clone()).unwrap_or(id);
                        self.overlay.toast(format!("Starting {name}…"));
                    }
                }
                Err(e) => {
                    tracing::warn!("launch {id}: {e}");
                    self.overlay.toast(e);
                }
            },
            Action::Brightness(d) => {
                if let Some(b) = Backlight::find() {
                    let cur = b.get().unwrap_or(60) as i32;
                    if let Err(e) = b.set((cur + d).clamp(5, 100) as u8) {
                        self.overlay.toast(format!("Brightness: {e}"));
                    }
                    self.sys.brightness = b.get();
                    let txt = format!("Brightness {}%", self.sys.brightness.unwrap_or(0));
                    self.osd(&txt);
                } else {
                    self.overlay.toast("No backlight");
                }
            }
            Action::Volume(d) => {
                let step = if d >= 0 { format!("{d}%+") } else { format!("{}%-", -d) };
                let argv: Vec<String> = self.cfg.actions.volume_set.iter().map(|a| a.replace("{step}", &step)).collect();
                run_quiet(&argv);
                self.volume = self.read_volume();
                let txt = match self.volume {
                    Some(v) => format!("Volume {v}%"),
                    None => "Volume".into(),
                };
                self.osd(&txt);
            }
            Action::ShowKeyboard => {
                self.overlay.visible = false;
                self.osk.show();
                self.osk_dirty = true;
                self.refocus();
            }
            Action::Sleep => run_detached(&self.cfg.actions.sleep.clone()),
            Action::Reboot => run_detached(&self.cfg.actions.reboot.clone()),
            Action::PowerOff => run_detached(&self.cfg.actions.poweroff.clone()),
            Action::Logout => {
                tracing::info!("log out requested");
                self.running = false;
                self.signal.stop();
            }
        }
        self.ui_dirty = true;
    }

    fn read_volume(&self) -> Option<u8> {
        let a = &self.cfg.actions.volume_get;
        let out = std::process::Command::new(a.first()?).args(&a[1..]).output().ok()?;
        parse_volume(&String::from_utf8_lossy(&out.stdout))
    }

    // ------------------------------------------- keyboard-mode apps ----

    fn keys_mode(&mut self, b: Button, down: bool) {
        if self.osk.visible {
            if down {
                self.repeater.press(b);
                self.osk_key(b);
            } else {
                self.repeater.release(b);
            }
            return;
        }
        match osk::pad_key(b) {
            Some(PadKey::Key(code)) => {
                if down {
                    self.held_keys.push(code);
                } else {
                    self.held_keys.retain(|c| *c != code);
                }
                self.send_key(code, down);
            }
            Some(PadKey::CtrlKey(code)) if down => self.tap(Tap { code, shift: false, ctrl: true, alt: false }),
            Some(PadKey::ShowOsk) if down => {
                self.osk.show();
                self.osk_dirty = true;
                self.release_held_keys();
                self.configure_all();
                self.needs_redraw = true;
            }
            _ => {}
        }
    }

    fn osk_key(&mut self, b: Button) {
        match self.osk.button(b) {
            Some(OskOut::Tap(t)) => self.tap(t),
            Some(OskOut::Hide) => {
                self.configure_all();
                self.needs_redraw = true;
            }
            None => {}
        }
        self.osk_dirty = true;
    }

    fn release_held_keys(&mut self) {
        for c in std::mem::take(&mut self.held_keys) {
            self.send_key(c, false);
        }
    }

    fn tap(&mut self, t: Tap) {
        let mods: Vec<u16> = [(t.ctrl, osk::k::LEFTCTRL), (t.alt, osk::k::LEFTALT), (t.shift, osk::k::LEFTSHIFT)]
            .iter()
            .filter(|m| m.0)
            .map(|m| m.1)
            .collect();
        for m in &mods {
            self.send_key(*m, true);
        }
        self.send_key(t.code, true);
        self.send_key(t.code, false);
        for m in mods.iter().rev() {
            self.send_key(*m, false);
        }
    }

    /// A key down/up to the focused surface (evdev code).
    pub fn send_key(&mut self, code: u16, down: bool) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.start.elapsed().as_millis() as u32;
        let state = if down { smithay::backend::input::KeyState::Pressed } else { smithay::backend::input::KeyState::Released };
        let kb = self.keyboard.clone();
        kb.input::<(), _>(self, Keycode::new(code as u32 + 8), state, serial, time, |_, _, _| FilterResult::Forward);
    }

    /// A key from a real keyboard.
    fn keyboard_key(&mut self, code: u16, value: i32) {
        const KEY_LEFTMETA: u16 = 125;
        const KEY_RIGHTMETA: u16 = 126;
        if value == 2 {
            return; // clients repeat themselves
        }
        let down = value == 1;
        if code == KEY_LEFTMETA || code == KEY_RIGHTMETA {
            if down {
                self.logo_alone = true;
            } else if std::mem::take(&mut self.logo_alone) {
                let out = vec![Routed::ToggleOverlay];
                self.apply_routed(out, None);
            }
            return;
        }
        self.logo_alone = false;
        if self.shell_screen() {
            if let Some(b) = map_evdev_key(code) {
                if down {
                    self.nav_press(b, false);
                }
            }
            return;
        }
        self.send_key(code, down);
    }

    // ------------------------------------------------------ rendering ----

    /// Frame callbacks for the visible window, after a frame was shown.
    pub fn frame_done(&mut self) {
        if self.shell_screen() {
            return;
        }
        if let Some(w) = self.visible_window().cloned() {
            w.send_frame(&self.output, self.start.elapsed(), Some(Duration::ZERO), |_, _| None);
        }
    }
}

/// A buffer for the shell's own (opaque) drawings.
fn opaque_buffer(w: i32, h: i32) -> MemoryRenderBuffer {
    let full = Rectangle::<i32, BufferCoords>::from_size((w, h).into());
    MemoryRenderBuffer::new(Fourcc::Abgr8888, (w, h), 1, smithay::utils::Transform::Normal, Some(vec![full]))
}

/// Copy a region of the UI canvas into a memory buffer (RGBA = Abgr8888).
pub fn upload(buf: &mut MemoryRenderBuffer, ui: &Ui, r: Rect) {
    let r = r.snap();
    let (x0, y0, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
    let stride = ui.pm.width() as usize * 4;
    let data = ui.pm.data();
    let mut ctx = buf.render();
    let _ = ctx.draw(|mem: &mut [u8]| {
        for row in 0..h {
            let src = &data[(y0 + row) * stride + x0 * 4..(y0 + row) * stride + (x0 + w) * 4];
            mem[row * w * 4..(row + 1) * w * 4].copy_from_slice(src);
        }
        Ok::<_, ()>(vec![Rectangle::<i32, BufferCoords>::from_size((w as i32, h as i32).into())])
    });
}

fn make_view<'a>(
    apps: &'a Apps<Window>,
    sys: &'a Sys,
    volume: Option<u8>,
    running: &'a [RunningInfo],
    launcher: &'a [LauncherInfo],
    current: Option<&'a RunningInfo>,
) -> View<'a> {
    let keyboard_app = current.and_then(|c| apps.get(c.uid)).is_some_and(|a| a.gamepad == GamepadMode::Keyboard);
    View { running, launcher, sys, volume, current, keyboard_app }
}

/// Brick button -> the evdev code the built-in pad sends.
pub fn button_code(b: Button) -> Option<u16> {
    use codes::*;
    Some(match b {
        Button::Up => BTN_DPAD_UP,
        Button::Down => BTN_DPAD_DOWN,
        Button::Left => BTN_DPAD_LEFT,
        Button::Right => BTN_DPAD_RIGHT,
        Button::A => BTN_EAST,
        Button::B => BTN_SOUTH,
        Button::X => BTN_NORTH,
        Button::Y => BTN_WEST,
        Button::L1 => BTN_TL,
        Button::R1 => BTN_TR,
        Button::L2 => BTN_TL2,
        Button::R2 => BTN_TR2,
        Button::Select => BTN_SELECT,
        Button::Start => BTN_START,
        Button::Menu => BTN_MODE,
        Button::F1 => BTN_TRIGGER_HAPPY1,
        Button::F2 => BTN_TRIGGER_HAPPY2,
        Button::VolUp => KEY_VOLUMEUP,
        Button::VolDown => KEY_VOLUMEDOWN,
        Button::Power => return None,
    })
}

fn run_quiet(argv: &[String]) {
    if let Some((cmd, args)) = argv.split_first() {
        let _ = std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn run_detached(argv: &[String]) {
    if let Some((cmd, args)) = argv.split_first()
        && let Err(e) = std::process::Command::new(cmd).args(args).spawn()
    {
        tracing::warn!("{cmd}: {e}");
    }
}

// ------------------------------------------------------------- handlers ----

impl CompositorHandler for Shell {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().expect("client state").compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(p) = get_parent(&root) {
                root = p;
            }
            if let Some((_, w)) = self.window_for_surface(&root) {
                w.on_commit();
                if self.visible_window() == Some(&w) && !self.shell_screen() {
                    self.needs_redraw = true;
                }
            }
        }
        // initial configure for toplevels
        if let Some((uid, w)) = self.window_for_surface(surface)
            && let Some(tl) = w.toplevel()
        {
            if !tl.is_initial_configure_sent() {
                tl.send_configure();
            } else if self.pending_focus == Some(uid) && has_buffer(tl.wl_surface()) {
                // first buffer of an app the user just started: show it
                self.pending_focus = None;
                self.show_app(uid);
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(p)) = self.popups.find_popup(surface)
            && !p.is_initial_configure_sent()
        {
            let _ = p.send_configure();
        }
    }
}

fn has_buffer(s: &WlSurface) -> bool {
    smithay::wayland::compositor::with_states(s, |st| {
        st.data_map
            .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
            .is_some_and(|d| d.lock().unwrap().buffer().is_some())
    })
}

impl BufferHandler for Shell {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Shell {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Shell {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }
    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: smithay::backend::allocator::dmabuf::Dmabuf, notifier: ImportNotifier) {
        if self.backend.import_dmabuf(&dmabuf) {
            let _ = notifier.successful::<Shell>();
        } else {
            notifier.failed();
        }
    }
}

impl SeatHandler for Shell {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Shell> {
        &mut self.seat_state
    }
}

impl OutputHandler for Shell {}

// Clipboard (copy/paste between apps, e.g. into Claude Code in foot).
impl SelectionHandler for Shell {
    type SelectionUserData = ();
}
impl DataDeviceHandler for Shell {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl ClientDndGrabHandler for Shell {}
impl ServerDndGrabHandler for Shell {}

impl XdgShellHandler for Shell {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let pid = surface
            .wl_surface()
            .client()
            .and_then(|c| c.get_credentials(&self.dh).ok())
            .map(|c| c.pid as u32);
        let uid = pid
            .and_then(|p| self.apps.for_pid(p, crate::apps::proc_parent))
            .or_else(|| {
                // a client the shell did not start (e.g. from SSH): its own
                // app entry, shown at once
                let name = pid
                    .and_then(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "App".into());
                let uid = self.apps.add(None, name, mortar_ui::icons::Icon::Pad, GamepadMode::None)?;
                if let Some(a) = self.apps.get_mut(uid) {
                    a.pid = pid;
                }
                self.pending_focus = Some(uid);
                Some(uid)
            });
        let Some(uid) = uid else {
            tracing::warn!("too many apps; closing new window");
            surface.send_close();
            return;
        };
        tracing::info!(uid, ?pid, "new toplevel");
        let w = Window::new_wayland_window(surface.clone());
        self.output.enter(surface.wl_surface());
        let size = self.size;
        surface.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Fullscreen);
            s.size = Some(size.into());
        });
        if let Some(a) = self.apps.get_mut(uid) {
            a.windows.push(w);
        }
        if self.apps.get(uid).is_some_and(|a| a.windows.len() > 1) && self.visible_app() == Some(uid) {
            // a second window of the visible app replaces the first on screen
            self.refocus();
        }
        self.ui_dirty = true;
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.opaque.remove(&surface.wl_surface().id());
        let vis = self.visible_window().cloned();
        let mut was_visible = false;
        for a in &mut self.apps.list {
            a.windows.retain(|w| {
                let keep = w.toplevel() != Some(&surface);
                if !keep && vis.as_ref() == Some(w) {
                    was_visible = true;
                }
                keep
            });
        }
        // apps with no window and no process are dropped in tick()
        self.apps.tick();
        if was_visible {
            self.refocus();
        }
        self.ui_dirty = true;
        self.needs_redraw = true;
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        // name external apps after their app_id
        let id = smithay::wayland::compositor::with_states(surface.wl_surface(), |s| {
            s.data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().unwrap().app_id.clone())
        });
        if let (Some(id), Some((uid, _))) = (id, self.window_for_surface(surface.wl_surface()))
            && let Some(a) = self.apps.get_mut(uid)
            && a.entry_id.is_none()
        {
            a.name = id;
            self.ui_dirty = true;
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn fullscreen_request(&mut self, _surface: ToplevelSurface, _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>) {
        self.configure_all();
    }

    fn maximize_request(&mut self, _surface: ToplevelSurface) {
        self.configure_all();
    }
}

delegate_compositor!(Shell);
delegate_shm!(Shell);
delegate_dmabuf!(Shell);
delegate_data_device!(Shell);
delegate_seat!(Shell);
delegate_output!(Shell);
delegate_xdg_shell!(Shell);
smithay::delegate_viewporter!(Shell);
