// SPDX-License-Identifier: GPL-2.0-or-later
//! The compositor state: Wayland globals and their handlers, the app list,
//! what is visible, and input dispatch.

use super::backend::Backend;
use super::input::{Inputs, Kind};
use super::pads::Pads;
use crate::apps::{Apps, LaunchEnv};
use crate::config::{Config, GamepadMode};
use crate::osk::{self, Osk, OskOut, PadKey, Tap};
use crate::router::{Receiver, Routed, Router};
use crate::shellui::{Pace, ShellUi, Toast};

use mortar_ui::gfx::{Fonts, Rect, Ui};
use mortar_ui::input::{codes, map_evdev_key, Button, Repeater};
use mortar_ui::status::Sys;

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
    /// The shell UI (home, MENU, transitions, OSD, toasts, boot).
    pub sui: ShellUi,
    /// Fonts for scenes (taken out while a renderer borrows the shell).
    pub typo: Option<crate::scene::Typo>,
    pub raster: crate::raster::Raster,
    pub cpu_imgs: super::ui::CpuImages,
    /// The software renderer's frame.
    pub ui_pm: resvg::tiny_skia::Pixmap,
    /// The home screen is up (the front app keeps running behind it).
    pub at_home: bool,
    pub last_step: Instant,
    /// UI time only moves on `ctl time +MS` (tests).
    pub manual_time: bool,
    pub sfx: crate::sfx::Player,
    pub haptics: crate::haptics::Haptics,
    /// Apps whose images the last scene showed (their commits redraw).
    pub scene_apps: Vec<u32>,
    /// Bumped when a CPU layer (keyboard strip, setup page) changes.
    pub layer_gen: u64,
    pub page_dirty: bool,
    pub page_tick: Option<Instant>,
    pub pending_toast: Option<(Instant, Toast)>,
    pub last_cover: (bool, bool),
    last_ambient: Instant,
    last_card_tick: Instant,
    /// Wi-Fi and Bluetooth switches (MENU > Quick), read with the status.
    pub radios: crate::shellui::Radios,
    /// Apps closing to be started again (MENU > Y > Restart): uid, entry.
    pub restarts: Vec<(u32, String)>,
    /// Volume keys held (up, down), and whether they made a screenshot.
    vol_held: [bool; 2],
    vol_combo: bool,
    pub host: String,
    /// The current time zone (Settings > Time zone).
    pub timezone: String,
    /// The power key went down at (cleared on release), and whether the
    /// hold already opened the power menu.
    power_down: Option<Instant>,
    power_long: bool,
    /// Asleep and dark: the backlight is off.
    pub dark: bool,
    /// Dark with Settings > Sleep = Suspend: suspend to RAM at this time
    /// (the grace lets a second power tap just wake the screen).
    suspend_at: Option<Instant>,
    /// The running `actions.sleep` (reaped in `tick`).
    suspend_child: Option<std::process::Child>,
    /// CLOCK_BOOTTIME - CLOCK_MONOTONIC at the last check: it grows by the
    /// time spent suspended, which is how a resume is noticed.
    boot_offset: f64,
    /// /sys/power/suspend_stats/success, sampled while dark.
    suspend_count: Option<u64>,
    /// Resumes from suspend seen since start (ctl status).
    pub resumes: u32,
    /// Right after a resume: the press that woke the Brick (the power key)
    /// is swallowed, so its release does not put it back to sleep.
    resume_grace: Option<Instant>,
    power_swallow: bool,
    /// Last button/key input, for Settings > Sleep when idle.
    last_input: Instant,
    /// The UI is drawn on the GPU (else on the CPU).
    pub gpu_ui: bool,
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
    /// The shell's screens must be redrawn.
    pub ui_dirty: bool,
    pub osk_dirty: bool,
    /// Something on screen changed; the backend should render a frame.
    pub needs_redraw: bool,
    /// A dmabuf (GPU) app committed while the OSK covers it (or, on the CPU
    /// renderer, a UI layer), so its frame must be read by the CPU from
    /// write-combined memory (~60 ms a frame). Those redraws are held to
    /// `SLOW_COMPOSITE`.
    slow_redraw: bool,
    last_slow_redraw: Instant,
    /// Screen content changed while nothing was rendered: shown/hidden
    /// windows need a reconfigure.
    pub sys: Sys,
    sys_at: Instant,
    pub volume: Option<u8>,
    /// The sink is muted (the OSD shows a crossed speaker).
    pub muted: bool,
    /// Sets and reads the volume off the event loop.
    pub volume_worker: super::ui::VolumeWorker,
    /// mortar-setupd (Wi-Fi, Bluetooth) off the loop; None without the
    /// setup service. Its newest snapshot, for the MENU pages.
    pub net_worker: Option<super::bg::NetWorker>,
    pub net: Option<crate::shellui::NetState>,
    /// PipeWire streams (pw-dump, wpctl) off the loop, and per app.
    pub mixer_worker: super::bg::MixerWorker,
    pub mixer: Vec<crate::shellui::MixChan>,
    pub streams: Vec<crate::mixer::Stream>,
    /// Last volume-step haptic: held keys tick at most every 150 ms.
    pub volume_haptic_at: Option<std::time::Instant>,
    last_bg_tick: Instant,
    /// Settings > Updates (mortar-update's status file).
    pub update: super::ui::UpdateWatch,
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
        let typo = crate::scene::Typo::new(Fonts::load(&mortar_ui::gfx::font_dir())?);
        let headless = matches!(backend, Backend::Headless(_));
        let prefs = crate::config::load_prefs(cfg.default_prefs());
        let sui = ShellUi::new(prefs, cfg.ui.boot_animation);
        let sfx = crate::sfx::Player::new(if headless { None } else { Some(cfg.ui.audio_device.clone()) });
        let haptics = if headless { crate::haptics::Haptics::off() } else { crate::haptics::Haptics::start(cfg.ui.haptic_ms) };
        let gpu_ui = backend.has_gpu_ui();
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "brick".into());
        let ui_pm = resvg::tiny_skia::Pixmap::new(size.0 as u32, size.1 as u32).ok_or("canvas size")?;
        let osk_h = osk::height(&ui) as i32;
        let overlay_buf = opaque_buffer(size.0, size.1);
        let osk_buf = opaque_buffer(size.0, osk_h);

        let viewporter_dh = dh.clone();
        let volume_worker = super::ui::VolumeWorker::new(cfg.actions.volume_set.clone(), cfg.actions.volume_get.clone());
        let net_worker = cfg.setup_socket.clone().map(super::bg::NetWorker::new);
        let mixer_worker = super::bg::MixerWorker::new(cfg.actions.pw_dump.clone());
        let mut shell = Shell {
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
            sui,
            typo: Some(typo),
            raster: crate::raster::Raster::default(),
            cpu_imgs: Default::default(),
            ui_pm,
            at_home: false,
            last_step: Instant::now(),
            manual_time: false,
            sfx,
            haptics,
            scene_apps: Vec::new(),
            layer_gen: 0,
            page_dirty: false,
            page_tick: None,
            pending_toast: None,
            last_cover: (false, false),
            last_ambient: Instant::now(),
            last_card_tick: Instant::now(),
            radios: Default::default(),
            restarts: Vec::new(),
            vol_held: [false; 2],
            vol_combo: false,
            host,
            timezone: "UTC".into(),
            power_down: None,
            power_long: false,
            dark: false,
            suspend_at: None,
            suspend_child: None,
            boot_offset: boot_offset(),
            suspend_count: None,
            resumes: 0,
            resume_grace: None,
            power_swallow: false,
            last_input: Instant::now(),
            gpu_ui,
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
            ui_dirty: true,
            osk_dirty: true,
            needs_redraw: true,
            slow_redraw: false,
            last_slow_redraw: Instant::now(),
            sys: Sys::read(),
            sys_at: Instant::now(),
            volume: None,
            muted: false,
            volume_worker,
            volume_haptic_at: None,
            net_worker,
            net: None,
            mixer_worker,
            mixer: Vec::new(),
            streams: Vec::new(),
            last_bg_tick: Instant::now(),
            update: Default::default(),
        };
        shell.apply_prefs();
        shell.refresh_timezone();
        Ok(shell)
    }

    // ----------------------------------------------------- what's shown ----

    /// The shell's own screens take the buttons and cover the app (home,
    /// the MENU, a transition, the boot screen).
    pub fn shell_screen(&self) -> bool {
        self.sui.captures_input(self.visible_app().is_some())
    }

    /// Something of the shell's is on screen (drawn by a UI renderer).
    pub fn ui_layer(&self) -> bool {
        self.sui.layer_visible(self.visible_app().is_some()) || self.osk_on_gpu()
    }

    /// The on-screen keyboard over a GPU (dma-buf) app goes through the
    /// GLES renderer like the OSD: pixman would read the app's
    /// write-combined buffer (~50 MB/s), which held those frames to 5/s.
    /// Over shm apps (foot) the damage-tracked pixman copy stays.
    fn osk_on_gpu(&self) -> bool {
        self.gpu_ui
            && self.osk.visible
            && self.visible_window().and_then(|w| w.toplevel().map(|t| t.wl_surface().clone())).is_some_and(|s| is_dmabuf(&s))
    }

    /// The app in front (None: the home screen); its window may still be
    /// covered by the shell's screens.
    pub fn visible_app(&self) -> Option<u32> {
        if self.at_home {
            return None;
        }
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
        let has_app = self.visible_app().is_some();
        self.last_cover = (self.sui.captures_input(has_app), self.sui.covers_app(has_app));
    }

    pub fn show_app(&mut self, uid: u32) {
        self.apps.focus(uid);
        self.at_home = false;
        self.refocus();
    }

    // ------------------------------------------------------------ apps ----

    pub fn launch(&mut self, id: &str) -> Result<u32, String> {
        let e = self.cfg.app(id).cloned().ok_or(format!("unknown app '{id}'"))?;
        if !e.available() {
            return Err(format!("{} is not installed", e.name));
        }
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
        if !self.manual_time {
            match self.sui.pace(self.visible_app().is_some()) {
                Pace::Full => t = t.min(Duration::from_millis(8)),
                Pace::Ambient => t = t.min(AMBIENT.saturating_sub(self.last_ambient.elapsed())),
                Pace::Lava => t = t.min(LAVA_EVERY.saturating_sub(self.last_ambient.elapsed())),
                Pace::Idle => {}
            }
            if let Some(d) = self.sui.next_deadline() {
                t = t.min(Duration::from_secs_f64(d));
            }
        }
        if self.power_down.is_some() {
            t = t.min(Duration::from_millis(16));
        }
        if let Some(at) = self.suspend_at {
            t = t.min(at.saturating_duration_since(now));
        }
        if let Some(d) = self.idle_deadline() {
            t = t.min(d.saturating_duration_since(now));
        }
        if let Some(p) = self.pending_toast.as_ref() {
            t = t.min(p.0.saturating_duration_since(now));
        }
        if let Some(p) = self.page_tick {
            t = t.min(p.saturating_duration_since(now));
        }
        if !self.sui.live_previews().is_empty() && self.cfg.ui.card_fps > 0 {
            t = t.min(Duration::from_millis(1000 / self.cfg.ui.card_fps as u64));
        }
        if self.slow_redraw {
            t = t.min((self.last_slow_redraw + SLOW_COMPOSITE).saturating_duration_since(now));
        }
        if self.apps.list.len() > 1 && self.cfg.background_fps > 0 {
            t = t.min(Duration::from_millis(1000 / self.cfg.background_fps as u64));
        }
        t
    }

    pub fn tick(&mut self) {
        self.check_resume();
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
            // MENU > Y > Restart: the old instance is gone, start it again
            let (again, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.restarts).into_iter().partition(|(u, _)| gone.contains(u));
            self.restarts = keep;
            for (_, id) in again {
                if let Err(e) = self.launch(&id) {
                    tracing::warn!("restart {id}: {e}");
                    self.sui.show_toast(crate::shellui::Toast::info(format!("Restart failed: {e}")));
                }
            }
        }
        // the shell UI: time, transitions, toasts
        self.volume_poll();
        self.bg_poll();
        self.ui_step();
        self.power_hold_tick();
        match self.sui.sleep_phase() {
            crate::shellui::SleepPhase::Dark => self.set_dark(true),
            crate::shellui::SleepPhase::Awake | crate::shellui::SleepPhase::Waking => self.set_dark(false),
            crate::shellui::SleepPhase::Fading => {}
        }
        self.suspend_tick();
        self.idle_tick();
        self.page_poll();
        self.update_poll();
        // status while a shell screen shows
        if self.shell_screen() && self.sys_at.elapsed() > Duration::from_secs(2) {
            if self.status_refresh() {
                self.ui_dirty = true;
            }
            self.sys_at = Instant::now();
        }
        // the MENU's live preview: that app keeps drawing (slowly)
        let live = self.sui.live_previews();
        if !live.is_empty() && self.cfg.ui.card_fps > 0 && self.last_card_tick.elapsed() >= Duration::from_millis(1000 / self.cfg.ui.card_fps as u64) {
            self.last_card_tick = Instant::now();
            let t = self.start.elapsed();
            for uid in live {
                if let Some(w) = self.apps.get(uid).and_then(|a| a.windows.last()) {
                    w.send_frame(&self.output, t, Some(Duration::ZERO), |_, _| None);
                }
            }
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
        if self.slow_redraw && self.last_slow_redraw.elapsed() >= SLOW_COMPOSITE {
            self.slow_redraw = false;
            self.last_slow_redraw = Instant::now();
            self.needs_redraw = true;
        }
        // what the UI needs drawn
        let has_app = self.visible_app().is_some();
        if self.ui_layer() {
            if self.ui_dirty || self.sui.dirty {
                self.needs_redraw = true;
            }
            if !self.manual_time {
                match self.sui.pace(has_app) {
                    Pace::Full => self.needs_redraw = true,
                    Pace::Ambient if self.last_ambient.elapsed() >= AMBIENT => {
                        self.last_ambient = Instant::now();
                        self.needs_redraw = true;
                    }
                    Pace::Lava if self.last_ambient.elapsed() >= LAVA_EVERY => {
                        self.last_ambient = Instant::now();
                        self.needs_redraw = true;
                    }
                    _ => {}
                }
            }
        } else if self.sui.dirty {
            self.sui.dirty = false;
            self.needs_redraw = true;
        }
        self.ui_dirty = false;
        self.update_layers();
        if self.dark {
            // asleep: the last (black) frame stays; nothing is drawn
            self.needs_redraw = false;
            self.slow_redraw = false;
        }
        if self.needs_redraw {
            self.slow_redraw = false;
            super::backend::render(self);
        }
    }

    // ---------------------------------------------------------- input ----

    /// A key event from a classified device.
    pub fn on_key(&mut self, kind: Kind, code: u16, value: i32) {
        // a key may be the first thing seen after a resume
        self.check_resume();
        self.last_input = Instant::now();
        // the power key belongs to the shell (logind ignores it, see
        // modules/mortar-shell.nix): tap = sleep/wake, hold = power menu
        if code == codes::KEY_POWER {
            self.power_key(value);
            return;
        }
        // asleep: any press only wakes
        if self.sui.sleep_phase() != crate::shellui::SleepPhase::Awake
            && value == 1
            && (code == codes::KEY_VOLUMEUP || code == codes::KEY_VOLUMEDOWN)
        {
            self.ui_wake();
            return;
        }
        // volume keys work everywhere; both together take a screenshot
        if code == codes::KEY_VOLUMEUP || code == codes::KEY_VOLUMEDOWN {
            let i = if code == codes::KEY_VOLUMEUP { 0 } else { 1 };
            let step = if i == 0 { 5 } else { -5 };
            match value {
                1 => {
                    self.vol_held[i] = true;
                    if self.vol_held[1 - i] {
                        // undo the other key's step, quietly
                        self.vol_combo = true;
                        let back = if i == 0 { 5 } else { -5 };
                        if let Some(v) = self.volume {
                            self.set_volume((v as i32 + back).clamp(0, 100) as u8);
                        }
                        self.sui.hide_osd();
                        self.screenshot_combo();
                    } else {
                        self.change_volume(step);
                    }
                }
                2 if !self.vol_combo => self.change_volume(step),
                0 => {
                    self.vol_held[i] = false;
                    if !self.vol_held[1 - i] {
                        self.vol_combo = false;
                    }
                }
                _ => {}
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
        self.last_input = Instant::now();
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
                    self.sys = Sys::read();
                    // read back off the event loop (MENU > Quick shows it)
                    self.volume_worker.request(None);
                    self.radios = super::ui::read_radios();
                    self.ui_menu_key();
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
            self.ui_button(b, repeat);
            if self.sui.page_open() {
                self.page_dirty = true;
            }
        } else if self.osk.visible {
            self.osk_key(b);
        }
    }

    fn power_key(&mut self, value: i32) {
        use crate::shellui::SleepPhase;
        match value {
            1 if self.resume_grace.is_some_and(|t| Instant::now() < t) => {
                // the press that woke the Brick (it is already waking)
                self.power_swallow = true;
                self.power_down = None;
            }
            1 => {
                self.power_down = Some(Instant::now());
                self.power_long = false;
            }
            0 if std::mem::take(&mut self.power_swallow) => {}
            0 => {
                let held = self.power_down.take();
                self.sui.set_power_hold(None);
                if self.power_long || held.is_none() {
                    return;
                }
                if self.sui.sleep_phase() != SleepPhase::Awake {
                    self.ui_wake();
                } else if self.sui.power_menu_open() {
                    self.sui.close_power_menu();
                } else {
                    self.sui.sleep();
                }
                let outs = self.sui.take_out();
                self.handle_outs(outs);
                self.sync_cover();
                self.needs_redraw = true;
            }
            _ => {}
        }
    }

    /// While the power key is held: progress, then the power menu.
    fn power_hold_tick(&mut self) {
        let Some(t) = self.power_down else { return };
        if self.power_long {
            return;
        }
        let e = t.elapsed().as_secs_f32();
        if e >= POWER_HOLD {
            self.power_long = true;
            self.sui.set_power_hold(None);
            if self.sui.sleep_phase() != crate::shellui::SleepPhase::Awake {
                self.ui_wake();
            }
            self.sui.open_power_menu();
            let outs = self.sui.take_out();
            self.handle_outs(outs);
            self.sync_cover();
        } else if e >= 0.25 && self.sui.sleep_phase() == crate::shellui::SleepPhase::Awake {
            self.sui.set_power_hold(Some((e - 0.25) / (POWER_HOLD - 0.25)));
        }
        self.needs_redraw = true;
    }

    /// Wake from sleep (backlight on first, then the fade back in).
    pub fn ui_wake(&mut self) {
        self.sui.wake();
        self.set_dark(false);
        let outs = self.sui.take_out();
        self.handle_outs(outs);
        self.sync_cover();
        self.needs_redraw = true;
    }

    /// Backlight off when the sleep fade reaches black; on when waking.
    pub fn set_dark(&mut self, dark: bool) {
        if dark == self.dark {
            return;
        }
        self.dark = dark;
        if let Some(b) = mortar_ui::status::Backlight::find()
            && let Err(e) = b.power(!dark)
        {
            tracing::warn!("backlight power: {e}");
        }
        tracing::info!(dark, "sleep");
        self.suspend_at = if dark && self.sui.prefs.suspend {
            Some(Instant::now() + Duration::from_millis(self.cfg.ui.suspend_grace_ms))
        } else {
            None
        };
    }

    /// Dark long enough: suspend to RAM (`actions.sleep`). Also reaps it.
    fn suspend_tick(&mut self) {
        if let Some(c) = &mut self.suspend_child
            && let Ok(Some(st)) = c.try_wait()
        {
            if !st.success() {
                tracing::warn!(%st, "suspend command failed");
            }
            self.suspend_child = None;
        }
        let Some(at) = self.suspend_at else { return };
        if !self.dark || !self.sui.prefs.suspend {
            self.suspend_at = None;
            return;
        }
        if Instant::now() < at || self.suspend_child.is_some() {
            return;
        }
        self.suspend_at = None;
        tracing::info!("suspending");
        if let Some((cmd, args)) = self.cfg.actions.sleep.split_first() {
            match std::process::Command::new(cmd).args(args).spawn() {
                Ok(c) => self.suspend_child = Some(c),
                Err(e) => tracing::warn!("{cmd}: {e}"),
            }
        }
    }

    /// A resume shows up as a jump between CLOCK_BOOTTIME and
    /// CLOCK_MONOTONIC (deep suspend; UI time, `Instant`, does not move
    /// meanwhile) or, while dark, as a new count in
    /// /sys/power/suspend_stats/success (s2idle on the Brick keeps
    /// CLOCK_MONOTONIC running). On resume: wake the screen, swallow the
    /// waking press, redraw and give every app a frame callback (they
    /// stopped while dark).
    fn check_resume(&mut self) {
        let off = boot_offset();
        let slept = off - self.boot_offset;
        self.boot_offset = off;
        let mut resumed = slept >= 0.5;
        if self.dark {
            let n = std::fs::read_to_string("/sys/power/suspend_stats/success")
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            if n.is_some() && self.suspend_count.is_some() && n != self.suspend_count {
                resumed = true;
            }
            self.suspend_count = n;
        } else {
            self.suspend_count = None;
        }
        if !resumed {
            return;
        }
        self.resumes += 1;
        tracing::info!(slept_s = format!("{slept:.1}"), resumes = self.resumes, "resumed");
        self.suspend_at = None;
        self.last_input = Instant::now();
        self.resume_grace = Some(Instant::now() + Duration::from_millis(1500));
        // a press held across the suspend never got its release
        self.power_down = None;
        self.power_long = false;
        if self.sui.sleep_phase() != crate::shellui::SleepPhase::Awake {
            self.ui_wake();
        }
        self.ui_dirty = true;
        self.needs_redraw = true;
        let t = self.start.elapsed();
        for a in &self.apps.list {
            for w in &a.windows {
                w.send_frame(&self.output, t, Some(Duration::ZERO), |_, _| None);
            }
        }
    }

    /// Settings > Sleep when idle: when the screen goes to sleep by itself.
    fn idle_deadline(&self) -> Option<Instant> {
        let m = self.sui.prefs.idle_minutes;
        if m == 0 || self.sui.sleep_phase() != crate::shellui::SleepPhase::Awake || self.power_down.is_some() {
            return None;
        }
        Some(self.last_input + Duration::from_secs(m as u64 * 60))
    }

    fn idle_tick(&mut self) {
        if self.idle_deadline().is_some_and(|d| Instant::now() >= d) {
            tracing::info!(minutes = self.sui.prefs.idle_minutes, "idle: sleeping");
            self.sui.sleep();
            self.sync_cover();
            self.needs_redraw = true;
        }
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
        if self.sui.covers_app(self.visible_app().is_some()) {
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

/// CLOCK_BOOTTIME - CLOCK_MONOTONIC, seconds: the total time suspended.
fn boot_offset() -> f64 {
    let now = |clk| {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid timespec out-pointer
        unsafe { libc::clock_gettime(clk, &mut ts) };
        ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
    };
    now(libc::CLOCK_BOOTTIME) - now(libc::CLOCK_MONOTONIC)
}

pub fn run_quiet(argv: &[String]) {
    if let Some((cmd, args)) = argv.split_first() {
        let _ = std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

pub fn run_detached(argv: &[String]) {
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
            if let Some((uid, w)) = self.window_for_surface(&root) {
                w.on_commit();
                let shown = self.visible_window() == Some(&w) && !self.shell_screen();
                let in_scene = self.ui_layer() && self.scene_apps.contains(&uid);
                if shown || in_scene {
                    // the CPU renders this app's dma-buf from write-combined
                    // memory: hold those redraws to SLOW_COMPOSITE
                    let cpu_reads = is_dmabuf(&root) && !self.gpu_ui && (self.osk.visible || in_scene);
                    if cpu_reads && self.sui.pace(self.visible_app().is_some()) != Pace::Full {
                        self.slow_redraw = true;
                    } else {
                        self.needs_redraw = true;
                    }
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

/// Composited GPU-app frames while something covers the app: at most 5/s.
const SLOW_COMPOSITE: Duration = Duration::from_millis(200);
/// Hold the power key this long (s) for the power menu.
const POWER_HOLD: f32 = 0.8;
/// Frame interval when only the background drifts.
const AMBIENT: Duration = Duration::from_millis(50);
/// The lava lamp alone flows at ~30 fps (its slow wax needs no more).
const LAVA_EVERY: Duration = Duration::from_millis(33);

/// The surface's current buffer is a dmabuf (a GPU app).
fn is_dmabuf(s: &WlSurface) -> bool {
    smithay::wayland::compositor::with_states(s, |st| {
        st.data_map
            .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
            .and_then(|d| d.lock().unwrap().buffer().map(|b| smithay::wayland::dmabuf::get_dmabuf(b).is_ok()))
            .unwrap_or(false)
    })
}

fn has_buffer(s: &WlSurface) -> bool {
    smithay::wayland::compositor::with_states(s, |st| {
        st.data_map
            .get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
            .is_some_and(|d| d.lock().unwrap().buffer().is_some())
    })
}

impl BufferHandler for Shell {
    fn buffer_destroyed(&mut self, buffer: &WlBuffer) {
        // drop the GPU UI's import of it now, so its CMA pages go back
        if let Backend::Drm(d) = &mut self.backend
            && let Some(g) = &mut d.gpu
        {
            g.forget(&buffer.id());
        }
    }
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
