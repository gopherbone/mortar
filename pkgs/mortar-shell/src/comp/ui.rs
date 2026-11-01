// SPDX-License-Identifier: GPL-2.0-or-later
//! The compositor side of the shell UI (shellui.rs): what the UI is told
//! (`View`), what it asks for (`Out`), its scenes, the app images it shows
//! (CPU copies for the software renderer and screenshots), sounds, haptics
//! and screenshots.

use super::state::Shell;
use crate::art::Art;
use crate::config::GamepadMode;
use crate::haptics::Haptic;
use crate::raster::{pixmap_from_xrgb, Images};
use crate::scene::{Layer, Scene, Src};
use crate::sfx::Sfx;
use crate::shellui::{LauncherInfo, Out, Radios, RunningInfo, Toast, ToastIcon, View};
use mortar_ui::status::{Backlight, Sys};
use resvg::tiny_skia::Pixmap;
use smithay::backend::allocator::Buffer;
use smithay::backend::renderer::utils::{CommitCounter, RendererSurfaceStateUserData};
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface};
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shm::with_buffer_contents;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// The MENU backdrop is blurred again at most this often.
pub const BLUR_EVERY: Duration = Duration::from_millis(66);

/// A surface's current buffer and commit.
pub fn surface_buffer(s: &WlSurface) -> Option<(WlBuffer, CommitCounter)> {
    with_states(s, |st| {
        let d = st.data_map.get::<RendererSurfaceStateUserData>()?;
        let d = d.lock().unwrap();
        let b: WlBuffer = (**d.buffer()?).clone();
        Some((b, d.current_commit()))
    })
}

/// The rfkill switches for MENU > Quick.
pub fn read_radios() -> Radios {
    use crate::radio::{state, Radio};
    Radios { wifi: state(Radio::Wifi), bluetooth: state(Radio::Bluetooth) }
}

/// CPU copies of app buffers and layers, for the software renderer and
/// screenshots (converted once per commit).
#[derive(Default)]
pub struct CpuImages {
    apps: HashMap<u32, (ObjectId, CommitCounter, Pixmap)>,
    /// Blurred copies (the MENU backdrop): source commit, when, pixmap.
    blurs: HashMap<u32, (ObjectId, CommitCounter, Instant, Pixmap)>,
    pub osk: Option<Pixmap>,
    pub page: Option<Pixmap>,
}

impl Images for CpuImages {
    fn get(&self, s: Src) -> Option<&Pixmap> {
        match s {
            Src::App(u) => self.apps.get(&u).map(|e| &e.2),
            Src::Blur(u) => self.blurs.get(&u).map(|e| &e.3),
            Src::Layer(Layer::Osk) => self.osk.as_ref(),
            Src::Layer(Layer::Page) => self.page.as_ref(),
        }
    }
}

/// Copy a buffer's pixels (wl_shm or a linear XRGB dma-buf).
fn buffer_pixmap(b: &WlBuffer) -> Option<Pixmap> {
    if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(b) {
        use smithay::backend::allocator::dmabuf::DmabufMappingMode;
        let size = dmabuf.size();
        let stride = dmabuf.strides().next()? as usize;
        let off = dmabuf.offsets().next()? as usize;
        let map = dmabuf.map_plane(0, DmabufMappingMode::READ).ok()?;
        let len = map.length();
        if off + stride * size.h as usize > len {
            return None;
        }
        // SAFETY: the mapping is `len` bytes, checked above
        let data = unsafe { std::slice::from_raw_parts((map.ptr() as *const u8).add(off), stride * size.h as usize) };
        return pixmap_from_xrgb(data, size.w as usize, size.h as usize, stride, 1);
    }
    with_buffer_contents(b, |ptr, len, d| {
        if !matches!(d.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
            return None;
        }
        let (w, h, stride, off) = (d.width as usize, d.height as usize, d.stride as usize, d.offset as usize);
        if off + stride * h > len {
            return None;
        }
        // SAFETY: bounds checked; the client may scribble concurrently
        let data = unsafe { std::slice::from_raw_parts(ptr.add(off), stride * h) };
        pixmap_from_xrgb(data, w, h, stride, 1)
    })
    .ok()
    .flatten()
}

/// Upload a region of a pixmap into a memory buffer (RGBA = Abgr8888).
pub fn upload_pm(buf: &mut smithay::backend::renderer::element::memory::MemoryRenderBuffer, pm: &Pixmap) {
    use smithay::utils::{Buffer as BufferCoords, Rectangle};
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let data = pm.data();
    let mut ctx = buf.render();
    let _ = ctx.draw(|mem: &mut [u8]| {
        let n = mem.len().min(data.len());
        mem[..n].copy_from_slice(&data[..n]);
        Ok::<_, ()>(vec![Rectangle::<i32, BufferCoords>::from_size((w as i32, h as i32).into())])
    });
}

impl Shell {
    /// The surface shown for app `uid` (its newest window).
    pub fn app_surface(&self, uid: u32) -> Option<WlSurface> {
        self.apps.get(uid)?.windows.last()?.toplevel().map(|t| t.wl_surface().clone())
    }

    fn art_for(&self, entry: Option<&str>, name: &str, icon: mortar_ui::icons::Icon) -> Art {
        entry
            .and_then(|id| self.cfg.app(id))
            .and_then(|e| e.art)
            .unwrap_or_else(|| Art::guess(entry.unwrap_or(""), name, icon))
    }

    /// What the UI is told, in owned form (see `with_view`).
    pub fn view_parts(&self) -> (Vec<RunningInfo>, Vec<LauncherInfo>) {
        let area = self.app_area();
        let running = self
            .apps
            .ordered()
            .into_iter()
            .filter_map(|u| self.apps.get(u))
            .map(|a| {
                let w = a.windows.last();
                let has_buffer = w.and_then(|w| w.toplevel()).is_some_and(|t| surface_buffer(t.wl_surface()).is_some());
                let geom = w
                    .map(|w| {
                        let g = w.geometry();
                        let (bw, bh) = w
                            .toplevel()
                            .and_then(|t| {
                                with_states(t.wl_surface(), |st| {
                                    st.data_map.get::<RendererSurfaceStateUserData>().and_then(|d| d.lock().unwrap().view()).map(|v| (v.dst.w, v.dst.h))
                                })
                            })
                            .unwrap_or((g.size.w, g.size.h));
                        let x = ((area.w - g.size.w) / 2).max(0) - g.loc.x;
                        let y = ((area.h - g.size.h) / 2).max(0) - g.loc.y;
                        [x as f32, y as f32, bw as f32, bh as f32]
                    })
                    .unwrap_or([0.0, 0.0, self.size.0 as f32, self.size.1 as f32]);
                RunningInfo {
                    uid: a.uid,
                    entry: a.entry_id.clone(),
                    name: a.name.clone(),
                    art: self.art_for(a.entry_id.as_deref(), &a.name, a.icon),
                    has_buffer,
                    closing: a.closing.is_some(),
                    geom,
                }
            })
            .collect();
        let launcher = self
            .cfg
            .apps
            .iter()
            .filter(|e| e.available())
            .map(|e| {
                let art = self.art_for(Some(&e.id), &e.name, e.icon);
                LauncherInfo {
                    id: e.id.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    art,
                    running: self.apps.by_entry(&e.id).is_some(),
                    category: e.category.unwrap_or_else(|| crate::config::Category::guess(art, &e.id, &e.name)),
                    single_instance: e.single_instance,
                }
            })
            .collect();
        (running, launcher)
    }

    pub fn keyboard_app(&self) -> bool {
        self.visible_app().and_then(|u| self.apps.get(u)).is_some_and(|a| a.gamepad == GamepadMode::Keyboard)
    }

    /// Run `f` with the UI and a view of the current state.
    pub fn with_view<T>(&mut self, f: impl FnOnce(&mut crate::shellui::ShellUi, &View) -> T) -> T {
        let (running, launcher) = self.view_parts();
        let host = self.host.clone();
        let timezone = self.timezone.clone();
        let osk_h = if self.osk.visible { self.osk_height() as f32 } else { 0.0 };
        let update = self.update.view.clone();
        let v = View {
            running: &running,
            launcher: &launcher,
            sys: &self.sys,
            volume: self.volume,
            current: self.visible_app(),
            keyboard_app: self.keyboard_app(),
            osk_h,
            host: &host,
            timezone: &timezone,
            radios: self.radios,
            update: update.as_ref(),
        };
        f(&mut self.sui, &v)
    }

    /// Advance the UI clock and apply what it asks for.
    pub fn ui_step(&mut self) {
        let now = Instant::now();
        let dt = if self.manual_time { 0.0 } else { now.duration_since(self.last_step).as_secs_f64() };
        self.last_step = now;
        let outs = self.with_view(|ui, v| ui.step(dt, v));
        self.handle_outs(outs);
        if let Some((t, _)) = &self.pending_toast
            && Instant::now() >= *t
        {
            let (_, toast) = self.pending_toast.take().unwrap();
            self.sui.show_toast(toast);
            let outs = self.sui.take_out();
            self.handle_outs(outs);
        }
        self.sync_cover();
    }

    /// Manual UI time (tests): advance by `ms`.
    pub fn ui_advance(&mut self, ms: u32) {
        let mut left = ms as f64 / 1000.0;
        while left > 1e-9 {
            let dt = left.min(1.0 / 120.0);
            let outs = self.with_view(|ui, v| ui.step(dt, v));
            self.handle_outs(outs);
            left -= dt;
        }
        self.sync_cover();
        self.needs_redraw = true;
    }

    /// The UI started or stopped covering the app: refocus once.
    pub fn sync_cover(&mut self) {
        let has_app = self.visible_app().is_some();
        let now = (self.sui.captures_input(has_app), self.sui.covers_app(has_app));
        if now != self.last_cover {
            self.last_cover = now;
            self.refocus();
        }
    }

    pub fn ui_button(&mut self, b: mortar_ui::input::Button, repeat: bool) {
        let outs = self.with_view(|ui, v| ui.button(b, repeat, v));
        self.handle_outs(outs);
        self.sync_cover();
        self.needs_redraw = true;
    }

    pub fn ui_menu_key(&mut self) {
        let outs = self.with_view(|ui, v| ui.menu_key(v));
        self.handle_outs(outs);
        self.sync_cover();
        self.needs_redraw = true;
    }

    pub fn play_sfx(&self, s: Sfx) {
        if self.sui.prefs.sounds && !self.dark {
            self.sfx.play(s, self.sui.prefs.sound_volume);
        }
    }

    pub fn play_haptic(&self, h: Haptic) {
        if self.sui.prefs.haptics && !self.dark {
            self.haptics.play(h);
        }
    }

    pub fn handle_outs(&mut self, outs: Vec<Out>) {
        for o in outs {
            tracing::debug!(?o, "ui");
            match o {
                Out::Sfx(s) => self.play_sfx(s),
                Out::Haptic(h) => self.play_haptic(h),
                Out::Launch(id) => match self.launch(&id) {
                    Ok(uid) => self.sui.launched(&id, uid),
                    Err(e) => {
                        tracing::warn!("launch {id}: {e}");
                        self.sui.launch_failed(&e);
                    }
                },
                Out::Focus(uid) => self.show_app(uid),
                Out::Close(uid) => self.close_app(uid),
                Out::Restart(uid) => {
                    if let Some(id) = self.apps.get(uid).and_then(|a| a.entry_id.clone()) {
                        self.restarts.push((uid, id));
                        self.close_app(uid);
                    }
                }
                Out::Radio(r, on) => {
                    if let Err(e) = crate::radio::set(r, on) {
                        tracing::warn!("{}: {e}", r.name());
                        self.sui.show_toast(Toast::info(format!("{}: {e}", r.name())));
                        self.play_sfx(Sfx::Deny);
                    }
                    self.radios = read_radios();
                }
                Out::Home => {
                    self.at_home = true;
                    self.refocus();
                }
                Out::Volume(d) => self.change_volume(d),
                Out::Brightness(d) => {
                    if let Some(b) = Backlight::find() {
                        let cur = b.get().unwrap_or(60) as i32;
                        if let Err(e) = b.set((cur + d).clamp(5, 100) as u8) {
                            self.sui.show_toast(Toast::info(format!("Brightness: {e}")));
                        }
                        self.sys.brightness = b.get();
                        let level = self.sys.brightness.unwrap_or(0);
                        if self.sui.menu_open() {
                            // MENU > Quick shows the value in its row
                            self.play_sfx(Sfx::Vol(level));
                        } else {
                            self.sui.osd("BRI", level);
                        }
                        self.play_haptic(Haptic::Tick);
                    } else {
                        self.sui.show_toast(Toast::info("No backlight"));
                    }
                }
                Out::ShowKeyboard => {
                    self.osk.show();
                    self.osk_dirty = true;
                    self.refocus();
                }
                Out::OpenSetup(p) => self.open_setup(p),
                Out::OpenTimeZone => {
                    let dir = std::path::PathBuf::from(&self.cfg.ui.zoneinfo);
                    let ids = crate::tz::list(&dir);
                    let off = crate::tz::offsets(&dir, &ids);
                    let zones = ids.into_iter().map(|id| crate::shellui::TzZone { offset: off.get(&id).copied().unwrap_or(0), id }).collect();
                    self.refresh_timezone();
                    self.sui.open_tz(zones, self.timezone.clone());
                }
                Out::SetTimeZone(zone) => self.set_timezone(&zone),
                Out::Sleep => {
                    // fade to dark; with Settings > Sleep = Suspend the
                    // compositor then suspends to RAM (state.rs suspend_tick)
                    self.sui.sleep();
                    self.sync_cover();
                }
                Out::Reboot => super::state::run_detached(&self.cfg.actions.reboot.clone()),
                Out::PowerOff => super::state::run_detached(&self.cfg.actions.poweroff.clone()),
                Out::Logout => {
                    tracing::info!("log out requested");
                    self.running = false;
                    self.signal.stop();
                }
                Out::Prefs => {
                    crate::config::save_prefs(&self.sui.prefs);
                    self.apply_prefs();
                }
                Out::UpdateCheck => {
                    if let Some(u) = &self.cfg.update {
                        super::state::run_detached(&u.check.clone());
                        // say how it ended when the status settles
                        self.update.asked = true;
                    }
                }
                Out::UpdateAuto(on) => {
                    if let Some(u) = &self.cfg.update {
                        let argv: Vec<String> = u.auto.iter().map(|a| a.replace("{on}", if on { "on" } else { "off" })).collect();
                        super::state::run_detached(&argv);
                        // shown at once; the status file follows
                        if let Some(v) = &mut self.update.view {
                            v.auto_check = on;
                        }
                        self.ui_dirty = true;
                    }
                }
            }
            let more = self.sui.take_out();
            if !more.is_empty() {
                self.handle_outs(more);
            }
            // something was asked: the screen may change
            self.needs_redraw = true;
        }
    }

    /// Settings that reach outside the UI (the toolkit theme for the
    /// keyboard and setup pages).
    pub fn apply_prefs(&mut self) {
        let t = if self.sui.prefs.night { mortar_ui::theme::DARK } else { mortar_ui::theme::LIGHT };
        mortar_ui::theme::set(t);
        self.osk_dirty = true;
        self.page_dirty = true;
    }

    fn open_setup(&mut self, which: crate::shellui::SetupPage) {
        use crate::shellui::SetupPage;
        use mortar_setup::ui::{SetupUi, SocketDevice};
        let Some(sock) = self.cfg.setup_socket.clone() else {
            let m = match which {
                SetupPage::Wifi => "Wi-Fi settings need mortar.setup.enable",
                SetupPage::Remote => "Remote access needs mortar.setup.enable",
                SetupPage::Bluetooth => "Bluetooth settings need mortar.setup.enable",
            };
            self.sui.show_toast(Toast::info(m));
            return;
        };
        let dev = Box::new(SocketDevice::new(&sock));
        if let Some(e) = &dev.error {
            self.sui.show_toast(Toast::info(format!("Setup service: {e}")));
            return;
        }
        let page = match which {
            SetupPage::Wifi => SetupUi::wifi(dev, self.sys.clone()),
            SetupPage::Remote => SetupUi::remote_access(dev, self.sys.clone()),
            SetupPage::Bluetooth => SetupUi::bluetooth(dev, self.sys.clone()),
        };
        self.sui.open_page(page);
        self.page_tick = Some(Instant::now() + Duration::from_secs(1));
        self.page_dirty = true;
    }

    /// Re-read the zone /etc/localtime names (and make libc notice).
    pub fn refresh_timezone(&mut self) {
        mortar_ui::status::reload_timezone();
        self.timezone = crate::tz::current(&crate::config::expand_home(&self.cfg.ui.localtime));
    }

    fn set_timezone(&mut self, zone: &str) {
        let argv: Vec<String> = self.cfg.actions.timezone_set.iter().map(|a| a.replace("{zone}", zone)).collect();
        let r = argv
            .split_first()
            .ok_or("no timezoneSet command".to_string())
            .and_then(|(c, a)| std::process::Command::new(c).args(a).output().map_err(|e| format!("{c}: {e}")))
            .and_then(|o| if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) });
        match r {
            Ok(()) => {
                tracing::info!(zone, "time zone set");
                self.refresh_timezone();
                self.sys = Sys::read();
                let city = crate::tz::pretty(zone).0;
                let mut t = Toast::info(format!("Time zone set to {city}"));
                t.sub = format!("{} · {}", zone.to_uppercase(), self.sys.time);
                t.haptic = Some(Haptic::Tick);
                self.sui.show_toast(t);
            }
            Err(e) => {
                tracing::warn!(zone, "set time zone: {e}");
                self.sui.show_toast(Toast::info(format!("Time zone: {e}")));
                self.play_sfx(Sfx::Deny);
            }
        }
        self.ui_dirty = true;
    }

    pub fn change_volume(&mut self, d: i32) {
        let step = if d >= 0 { format!("{d}%+") } else { format!("{}%-", -d) };
        let argv: Vec<String> = self.cfg.actions.volume_set.iter().map(|a| a.replace("{step}", &step)).collect();
        super::state::run_quiet(&argv);
        self.volume = self.read_volume();
        let v = self.volume.unwrap_or(0);
        if !self.sui.menu_open() {
            self.sui.osd("VOL", v);
        }
        self.play_sfx(Sfx::Vol(v));
        self.play_haptic(Haptic::Tick);
        self.needs_redraw = true;
    }

    /// The setup page's periodic refresh.
    pub fn page_poll(&mut self) {
        if self.sui.page_open() && self.page_tick.is_some_and(|t| Instant::now() >= t) {
            if let Some(p) = self.sui.page_mut() {
                let changed = p.tick();
                let exited = p.exited();
                self.page_tick = Some(Instant::now() + p.next_tick());
                if exited {
                    self.sui.close_page();
                } else if changed {
                    self.page_dirty = true;
                }
            }
        }
    }

    /// Bring the CPU-drawn layers (keyboard strip, setup page) up to date.
    pub fn update_layers(&mut self) {
        if self.osk.visible && self.osk_dirty {
            self.osk_dirty = false;
            let h = self.osk_height() as f32;
            let area = mortar_ui::gfx::Rect::new(0.0, self.ui.height() - h, self.ui.width(), h);
            self.osk.draw(&mut self.ui, area);
            super::state::upload(&mut self.osk_buf, &self.ui, area);
            let (w, hh) = (self.ui.pm.width() as usize, h as usize);
            if let Some(mut strip) = Pixmap::new(w as u32, hh as u32) {
                let y0 = self.ui.pm.height() as usize - hh;
                strip.data_mut().copy_from_slice(&self.ui.pm.data()[y0 * w * 4..(y0 + hh) * w * 4]);
                self.cpu_imgs.osk = Some(strip);
                self.layer_gen += 1;
            }
            self.needs_redraw = true;
        }
        if self.sui.page_open() && self.page_dirty {
            self.page_dirty = false;
            let sys = self.sys.clone();
            if let Some(p) = self.sui.page_mut() {
                p.cx.sys = sys;
                p.draw(&mut self.ui);
            }
            self.cpu_imgs.page = Some(self.ui.pm.clone());
            self.layer_gen += 1;
            self.osk_dirty = true; // the canvas was overwritten
            self.needs_redraw = true;
        }
        if !self.sui.page_open() {
            self.cpu_imgs.page = None;
        }
    }

    /// This frame's scene.
    pub fn build_scene(&mut self) -> Scene {
        self.update_layers();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let typo = std::mem::replace(&mut self.typo, None);
        let scene = match &typo {
            Some(t) => self.with_view(|ui, v| ui.build(t, v, w, h)),
            None => Scene::new(w, h, mortar_ui::gfx::Color(0, 0, 0, 255)),
        };
        self.typo = typo;
        self.scene_apps = scene.apps();
        self.sui.dirty = false;
        scene
    }

    /// Refresh the CPU copies of the app images a scene uses.
    pub fn refresh_cpu_images(&mut self, scene: &Scene) {
        let used = scene.apps();
        self.cpu_imgs.apps.retain(|u, _| used.contains(u));
        for uid in used {
            let Some(s) = self.app_surface(uid) else { continue };
            let Some((buf, commit)) = surface_buffer(&s) else { continue };
            let fresh = self.cpu_imgs.apps.get(&uid).is_some_and(|e| e.0 == s.id() && e.1 == commit);
            if fresh {
                continue;
            }
            if let Some(pm) = buffer_pixmap(&buf) {
                self.cpu_imgs.apps.insert(uid, (s.id(), commit, pm));
            }
        }
        // blurred backdrops: again when the app committed, at most ~15/s
        let blurs = scene.blurs();
        self.cpu_imgs.blurs.retain(|u, _| blurs.contains(u));
        for uid in blurs {
            let Some((sid, commit, pm)) = self.cpu_imgs.apps.get(&uid) else { continue };
            let stale = match self.cpu_imgs.blurs.get(&uid) {
                Some((bs, bc, at, _)) => (bs != sid || bc != commit) && at.elapsed() >= BLUR_EVERY,
                None => true,
            };
            if stale && let Some(b) = crate::raster::blur_quarter(pm) {
                self.cpu_imgs.blurs.insert(uid, (sid.clone(), *commit, Instant::now(), b));
            }
        }
    }

    /// The software renderer: draw the scene into the overlay buffer.
    pub fn cpu_ui_frame(&mut self) {
        let scene = self.build_scene();
        self.refresh_cpu_images(&scene);
        if let Some(t) = &self.typo {
            self.raster.render(&mut self.ui_pm, &scene, t, &self.cpu_imgs);
        }
        upload_pm(&mut self.overlay_buf, &self.ui_pm);
    }

    /// The screen as it is now (without the shutter flash), on the CPU.
    pub fn capture(&mut self) -> Option<Pixmap> {
        // with no UI layer this is just the app (and keyboard strip)
        let scene = self.build_scene();
        self.refresh_cpu_images(&scene);
        let mut pm = Pixmap::new(self.size.0 as u32, self.size.1 as u32)?;
        let t = self.typo.as_ref()?;
        self.raster.render(&mut pm, &scene, t, &self.cpu_imgs);
        Some(pm)
    }

    /// The screenshot combo: capture, flash + shutter, write the PNG in the
    /// background, then a toast.
    pub fn screenshot_combo(&mut self) {
        let Some(pm) = self.capture() else {
            self.sui.show_toast(Toast::info("Screenshot failed"));
            return;
        };
        self.sui.screenshot();
        let outs = self.sui.take_out();
        self.handle_outs(outs);
        let dir = crate::config::expand_home(&self.cfg.ui.screenshot_dir);
        let name = format!("mortar-{}.png", stamp());
        let path = dir.join(&name);
        let p2 = path.clone();
        std::thread::spawn(move || {
            let r = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| save_rgb_png(&pm, &p2));
            match r {
                Ok(()) => tracing::info!(path = %p2.display(), "screenshot"),
                Err(e) => tracing::warn!(path = %p2.display(), "screenshot: {e}"),
            }
        });
        let shown = path.to_string_lossy().replace(&std::env::var("HOME").unwrap_or_default(), "~");
        let toast = Toast { icon: ToastIcon::Shot, title: "Screenshot saved".into(), sub: shown.to_uppercase(), secs: 2.4, sound: None, haptic: None };
        self.pending_toast = Some((Instant::now() + Duration::from_millis(180), toast));
        self.needs_redraw = true;
    }

    /// `ctl toast {json}` (apps: achievements etc).
    pub fn toast_json(&mut self, arg: &str) -> Result<(), String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct T {
            title: String,
            #[serde(default)]
            sub: String,
            #[serde(default)]
            kind: Option<String>,
            #[serde(default)]
            sound: Option<String>,
            #[serde(default)]
            secs: Option<f32>,
            #[serde(default)]
            haptic: Option<String>,
        }
        let t: T = serde_json::from_str(arg).map_err(|e| format!("toast: {e}"))?;
        let mut toast = match t.kind.as_deref() {
            Some("achievement") => Toast::achievement(t.title, t.sub),
            Some("info") | None => Toast { sub: t.sub, ..Toast::info(t.title) },
            Some(k) => return Err(format!("toast kind {k}: achievement | info")),
        };
        if let Some(s) = t.sound {
            toast.sound = Sfx::parse(&s);
        }
        if let Some(s) = t.secs {
            toast.secs = s.clamp(0.5, 30.0);
        }
        if let Some(h) = t.haptic {
            toast.haptic = match h.as_str() {
                "tick" => Some(Haptic::Tick),
                "double" => Some(Haptic::Double),
                "long" => Some(Haptic::Long),
                "none" => None,
                h => return Err(format!("toast haptic {h}: tick | double | long | none")),
            };
        }
        self.sui.show_toast(toast);
        let outs = self.sui.take_out();
        self.handle_outs(outs);
        self.needs_redraw = true;
        Ok(())
    }

    /// Re-read mortar-update's status when it changes (a stat every few
    /// seconds): toast once when an update is staged, and the outcome of a
    /// Check now.
    pub fn update_poll(&mut self) {
        let Some(path) = self.cfg.update.as_ref().map(|u| u.status.clone()) else { return };
        if self.update.at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            return;
        }
        self.update.at = Some(Instant::now());
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime == self.update.mtime && self.update.view.is_some() {
            return;
        }
        self.update.mtime = mtime;
        let st = std::fs::read_to_string(&path).ok().and_then(|s| crate::config::UpdateStatus::parse(&s)).unwrap_or_default();
        self.update.view = Some(st.view());
        let mut toast = None;
        if st.ready() {
            let top = st.available.as_ref().map(|a| a.toplevel.clone()).unwrap_or_default();
            if self.update.toasted.as_deref() != Some(top.as_str()) {
                self.update.toasted = Some(top);
                let mut t = Toast::info("Update ready — restart to apply");
                t.sub = st.available.as_ref().and_then(|a| a.changelog.first().cloned()).unwrap_or_else(|| "Settings › Updates".into());
                t.secs = 5.0;
                t.haptic = Some(Haptic::Double);
                toast = Some(t);
            }
            self.update.asked = false;
        } else if self.update.asked && !st.busy() && !st.state.is_empty() && st.state != "idle" {
            self.update.asked = false;
            let mut t = Toast::info(format!("Updates: {}", st.message));
            t.sub = st.detail.chars().take(80).collect();
            toast = Some(t);
        }
        if let Some(t) = toast {
            self.sui.show_toast(t);
            let outs = self.sui.take_out();
            self.handle_outs(outs);
        }
        self.ui_dirty = true;
        self.needs_redraw = true;
    }

    pub fn status_refresh(&mut self) -> bool {
        let new = Sys::read();
        let radios = read_radios();
        let changed = format!("{new:?}") != format!("{:?}", self.sys) || radios != self.radios;
        self.sys = new;
        self.radios = radios;
        changed
    }
}

/// What the shell knows about mortar-update.
#[derive(Default)]
pub struct UpdateWatch {
    pub view: Option<crate::shellui::UpdateView>,
    mtime: Option<std::time::SystemTime>,
    at: Option<Instant>,
    /// The staged update already announced.
    toasted: Option<String>,
    /// Check now was pressed: toast the result.
    asked: bool,
}

fn stamp() -> String {
    // SAFETY: libc time calls with a zeroed tm
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:04}{:02}{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

/// Write an opaque pixmap as an RGB PNG.
pub fn save_rgb_png(pm: &Pixmap, path: &Path) -> Result<(), String> {
    let (w, h) = (pm.width(), pm.height());
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in pm.data().chunks_exact(4) {
        rgb.extend_from_slice(&p[..3]);
    }
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut wr| wr.write_image_data(&rgb)).map_err(|e| e.to_string())
}
