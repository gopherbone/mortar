// SPDX-License-Identifier: GPL-2.0-or-later
//! A minimal Wayland client for mortar-shell: one fullscreen xdg_toplevel.
//! `activated` from the toplevel configure says whether we are on screen
//! (the shell deactivates apps behind its overlay and in the background),
//! which the app turns into pause/resume. Keys from a real keyboard come
//! through wl_keyboard as evdev codes.
//!
//! Buffers, best first (as gamescope does it: the game at its own size, the
//! display scales, nothing is composited):
//!
//! - **plane-scaled** (Display smooth / sharp bilinear): the core's frame
//!   at its own size (sharp bilinear: first nearest-scaled by a whole
//!   multiple), with black borders in proportion when letterboxed, in a KMS
//!   dumb buffer (a dma-buf, `dumb.rs`) with a wp_viewport destination of
//!   the whole screen. The shell puts it on the primary plane and the
//!   display engine's scaler scales it: for smooth the frontend copies
//!   ~0.3 MB a frame and the shell nothing.
//! - **screen-size dma-buf** (sharp, integer, the menus): the CPU nearest
//!   scaler writes straight into a screen-size dumb buffer, which the shell
//!   scans out unscaled (no copy in the shell).
//! - **wl_shm** (no display device, e.g. the sandbox tests, or
//!   MORTAR_PLAY_PRESENT=shm): two shared-memory buffers, scaled on the CPU;
//!   the shell copies the damaged area into scanout.
//!
//! A frame goes into whichever buffer the compositor has released; if all
//! are still held it is dropped rather than waited for.

use super::dumb::{Card, DumbBuf};
use super::{Display, PEvent, ui_to_xrgb};
use crate::video::{self, Dst, Filter, Frame, Scaler};
use mortar_ui::gfx::Ui;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::Duration;
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop, event_created_child};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Shm,
    /// Screen-size dma-buf, CPU scaled.
    Dma,
    /// Frame-size dma-buf, scaled by the display (viewport).
    Native,
}

enum Store {
    Shm { _pool: wl_shm_pool::WlShmPool, _fd: OwnedFd, ptr: *mut u32, len: usize },
    Dma(DumbBuf),
}

struct Buf {
    buffer: wl_buffer::WlBuffer,
    store: Store,
    /// Row stride in pixels.
    stride: usize,
    busy: bool,
    /// What the area outside the game rect holds: Some(dst) when the borders
    /// were cleared for that layout.
    layout: Option<Dst>,
}

impl Buf {
    fn px(&mut self) -> &mut [u32] {
        let (ptr, len) = match &self.store {
            Store::Shm { ptr, len, .. } => (*ptr, *len),
            Store::Dma(d) => (d.ptr, d.len),
        };
        unsafe { std::slice::from_raw_parts_mut(ptr, len / 4) }
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        self.buffer.destroy();
        if let Store::Shm { ptr, len, .. } = self.store {
            unsafe { libc::munmap(ptr as *mut _, len) };
        }
    }
}

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    configured: bool,
    size: (u32, u32),
    activated: Option<bool>,
    /// We have been on screen (activated) at least once.
    shown: bool,
    bufs: Vec<Buf>,
    events: Vec<PEvent>,
    /// The answer to the last zwp_linux_buffer_params_v1.create: Some(buffer)
    /// or None (failed).
    created: Option<Option<wl_buffer::WlBuffer>>,
}

pub struct WaylandDisplay {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    surface: wl_surface::WlSurface,
    _xdg: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    viewport: Option<wp_viewport::WpViewport>,
    /// The viewport destination last set (None: unset, the buffer's size).
    dest: Option<(u32, u32)>,
    /// The display device for dma-bufs; None: wl_shm.
    card: Option<Card>,
    scaler: Scaler,
    bufs_key: (u32, u32, Kind),
    fallback: (u32, u32),
    logged: Option<(u32, u32, Kind)>,
    /// Game frames shown / dropped because every buffer was still held.
    shown: u64,
    dropped: u64,
}

fn shm_buffer(shm: &wl_shm::WlShm, w: u32, h: u32, qh: &QueueHandle<State>) -> Result<Buf, String> {
    let len = (w * h * 4) as usize;
    unsafe {
        let fd = libc::memfd_create(c"mortar-play".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
        }
        let fd = OwnedFd::from_raw_fd(fd);
        if libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), len as libc::off_t) != 0 {
            return Err(format!("ftruncate: {}", std::io::Error::last_os_error()));
        }
        let ptr = libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, std::os::fd::AsRawFd::as_raw_fd(&fd), 0);
        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Xrgb8888, qh, ());
        Ok(Buf { buffer, store: Store::Shm { _pool: pool, _fd: fd, ptr: ptr as *mut u32, len }, stride: w as usize, busy: false, layout: None })
    }
}

impl WaylandDisplay {
    pub fn connect(w: u32, h: u32) -> Result<WaylandDisplay, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|e| format!("Wayland: {e}"))?;
        let compositor = state.compositor.clone().ok_or("Wayland: no wl_compositor")?;
        state.shm.as_ref().ok_or("Wayland: no wl_shm")?;
        let wm = state.wm.clone().ok_or("Wayland: no xdg_wm_base")?;
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("Games".into());
        toplevel.set_app_id("mortar-play".into());
        toplevel.set_fullscreen(None);
        surface.commit();
        for _ in 0..50 {
            if state.configured {
                break;
            }
            queue.blocking_dispatch(&mut state).map_err(|e| format!("Wayland: {e}"))?;
        }
        let shm_only = std::env::var("MORTAR_PLAY_PRESENT").is_ok_and(|v| v == "shm");
        let card = if shm_only || state.dmabuf.is_none() { None } else { Card::open() };
        let viewport = state.viewporter.as_ref().map(|v| v.get_viewport(&surface, &qh, ()));
        eprintln!(
            "mortar-play: Wayland {}x{}, {}{}",
            state.size.0,
            state.size.1,
            if card.is_some() { "dma-buf" } else { "wl_shm" },
            if viewport.is_some() { " + viewporter" } else { "" }
        );
        Ok(WaylandDisplay {
            conn,
            queue,
            qh,
            state,
            surface,
            _xdg: xdg,
            _toplevel: toplevel,
            viewport,
            dest: None,
            card,
            scaler: Scaler::default(),
            bufs_key: (0, 0, Kind::Shm),
            fallback: (w, h),
            logged: None,
            shown: 0,
            dropped: 0,
        })
    }

    fn screen(&self) -> (u32, u32) {
        if self.state.size.0 > 0 && self.state.size.1 > 0 { self.state.size } else { self.fallback }
    }

    /// A dma-buf wl_buffer (params.create, answered by created or failed).
    fn dma_buffer(&mut self, w: u32, h: u32) -> Result<Buf, String> {
        let card = self.card.as_ref().ok_or("no display device")?;
        let dmabuf = self.state.dmabuf.clone().ok_or("no zwp_linux_dmabuf_v1")?;
        let d = card.alloc(w, h)?;
        let params = dmabuf.create_params(&self.qh, ());
        params.add(d.fd.as_fd(), 0, 0, d.pitch, 0, 0); // modifier: linear
        params.create(w as i32, h as i32, DRM_FORMAT_XRGB8888, zwp_linux_buffer_params_v1::Flags::empty());
        self.state.created = None;
        for _ in 0..100 {
            if self.state.created.is_some() {
                break;
            }
            self.queue.blocking_dispatch(&mut self.state).map_err(|e| format!("Wayland: {e}"))?;
        }
        params.destroy();
        let buffer = self.state.created.take().flatten().ok_or("the compositor refused the dma-buf")?;
        let stride = (d.pitch / 4) as usize;
        Ok(Buf { buffer, store: Store::Dma(d), stride, busy: false, layout: None })
    }

    /// A released buffer of `kind` and size (recreating the set on a change).
    /// A dma-buf that can't be made switches this display to wl_shm for good
    /// (the next call gets shm buffers).
    fn free_buffer(&mut self, w: u32, h: u32, kind: Kind) -> Option<usize> {
        let kind = if self.card.is_none() { Kind::Shm } else { kind };
        if self.bufs_key != (w, h, kind) {
            self.state.bufs.clear();
            self.bufs_key = (0, 0, Kind::Shm);
            let n = if kind == Kind::Shm { 2 } else { 3 };
            for _ in 0..n {
                let b = match kind {
                    Kind::Shm => shm_buffer(self.state.shm.as_ref()?, w, h, &self.qh),
                    _ => self.dma_buffer(w, h),
                };
                match b {
                    Ok(b) => self.state.bufs.push(b),
                    Err(e) => {
                        eprintln!("mortar-play: {e}");
                        self.state.bufs.clear();
                        if kind != Kind::Shm {
                            eprintln!("mortar-play: no dma-buf presenting; using wl_shm");
                            self.card = None;
                            return self.free_buffer(w, h, Kind::Shm);
                        }
                        return None;
                    }
                }
            }
            self.bufs_key = (w, h, kind);
        }
        if self.logged != Some((w, h, kind)) {
            eprintln!("mortar-play: presenting {kind:?} {w}x{h}");
            self.logged = Some((w, h, kind));
        }
        self.state.bufs.iter().position(|b| !b.busy)
    }

    /// The viewport destination for the next commit (None: the buffer's size).
    fn set_dest(&mut self, dest: Option<(u32, u32)>) {
        if self.dest == dest {
            return;
        }
        if let Some(v) = &self.viewport {
            match dest {
                Some((w, h)) => v.set_destination(w as i32, h as i32),
                None => v.set_destination(-1, -1),
            }
        }
        self.dest = dest;
    }

    fn commit(&mut self, i: usize, damage: Dst) {
        let b = &mut self.state.bufs[i];
        b.busy = true;
        self.surface.attach(Some(&b.buffer), 0, 0);
        self.surface.damage_buffer(damage.x as i32, damage.y as i32, damage.w as i32, damage.h as i32);
        self.surface.commit();
        let _ = self.conn.flush();
    }

    fn dispatch(&mut self, block: Option<Duration>) {
        let _ = self.conn.flush();
        if let Some(guard) = self.queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut pfd = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&fd), events: libc::POLLIN, revents: 0 };
            let ms = block.map_or(0, |d| d.as_millis().min(i32::MAX as u128) as i32);
            let ready = unsafe { libc::poll(&mut pfd, 1, ms) } > 0;
            if ready {
                let _ = guard.read();
            }
        }
        let _ = self.queue.dispatch_pending(&mut self.state);
    }
}

impl Display for WaylandDisplay {
    fn size(&self) -> (u32, u32) {
        self.screen()
    }

    fn present_game(&mut self, f: &Frame, dst: Dst, filter: Filter) {
        self.dispatch(None);
        if f.is_empty() || dst.w == 0 || dst.h == 0 {
            return;
        }
        let (sw, sh) = self.screen();
        let full = Dst { x: 0, y: 0, w: sw, h: sh };
        // Smooth and sharp bilinear: the display engine scales (its filter is
        // a soft Lanczos-2). The buffer holds the frame (sharp bilinear: first
        // scaled up by a whole multiple, nearest) with black borders in the
        // same proportion as on screen, so it always covers the whole screen
        // and goes on the primary plane. Sharp and integer need nearest all
        // the way: CPU scaled into a screen-size buffer.
        if matches!(filter, Filter::Smooth | Filter::SharpBilinear) && self.viewport.is_some() && self.card.is_some() {
            let k = video::prescale(filter, f.w, f.h, dst);
            let (cw, ch) = (f.w * k, f.h * k);
            let pad = |c: u32, s: u32, d: u32| -> u32 { (((c as u64 * s as u64 + d as u64 / 2) / d as u64) as u32).max(c) };
            let (bw, bh) = (pad(cw, sw, dst.w), pad(ch, sh, dst.h));
            let Some(i) = self.free_buffer(bw, bh, Kind::Native) else {
                self.dropped += 1;
                return;
            };
            if self.bufs_key.2 == Kind::Native {
                let inner = Dst { x: (bw - cw) / 2, y: (bh - ch) / 2, w: cw, h: ch };
                let b = &mut self.state.bufs[i];
                let stride = b.stride;
                let damage = if b.layout != Some(inner) {
                    for row in b.px().chunks_mut(stride).take(bh as usize) {
                        row[..bw as usize].fill(0);
                    }
                    b.layout = Some(inner);
                    Dst { x: 0, y: 0, w: bw, h: bh }
                } else {
                    inner
                };
                let px = self.state.bufs[i].px();
                if k == 1 {
                    let w = f.w as usize;
                    for (y, row) in f.px.chunks_exact(w).enumerate() {
                        let o = (inner.y as usize + y) * stride + inner.x as usize;
                        px[o..o + w].copy_from_slice(row);
                    }
                } else {
                    self.scaler.scale(f, inner, px, stride);
                }
                self.set_dest(Some((sw, sh)));
                self.commit(i, damage);
                self.shown += 1;
                return;
            }
        }
        let Some(i) = self.free_buffer(sw, sh, Kind::Dma) else {
            self.dropped += 1; // all held: drop this frame
            return;
        };
        self.set_dest(None);
        let b = &mut self.state.bufs[i];
        let stride = b.stride;
        let damage = if b.layout != Some(dst) {
            // write-only (dma-bufs are write-combined)
            for row in b.px().chunks_mut(stride).take(sh as usize) {
                row[..sw as usize].fill(0);
            }
            b.layout = Some(dst);
            full
        } else {
            dst
        };
        let px = self.state.bufs[i].px();
        self.scaler.scale(f, dst, px, stride);
        self.commit(i, damage);
        self.shown += 1;
    }

    fn present_ui(&mut self, ui: &Ui) {
        self.dispatch(None);
        let (w, h) = self.screen();
        if ui.pm.width() != w || ui.pm.height() != h {
            return; // the app re-renders at the new size
        }
        let Some(i) = self.free_buffer(w, h, Kind::Dma) else { return };
        self.set_dest(None);
        let stride = self.state.bufs[i].stride;
        let px = self.state.bufs[i].px();
        ui_to_xrgb(ui, px, stride);
        self.state.bufs[i].layout = None;
        self.commit(i, Dst { x: 0, y: 0, w, h });
    }

    fn poll(&mut self, out: &mut Vec<PEvent>) {
        self.dispatch(None);
        out.append(&mut self.state.events);
    }

    fn wait(&mut self, timeout: Duration) {
        self.dispatch(Some(timeout.min(Duration::from_millis(10))));
    }

    fn name(&self) -> String {
        if self.card.is_some() { "wayland-dmabuf".into() } else { "wayland-shm".into() }
    }
}

impl Drop for WaylandDisplay {
    fn drop(&mut self) {
        eprintln!("mortar-play: {} game frames presented, {} dropped (no free buffer)", self.shown, self.dropped);
    }
}

// ------------------------------------------------------------ dispatch ----

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(s: &mut Self, reg: &wl_registry::WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "wl_compositor" => s.compositor = Some(reg.bind(name, version.min(4), qh, ())),
                "wl_shm" => s.shm = Some(reg.bind(name, 1, qh, ())),
                "xdg_wm_base" => s.wm = Some(reg.bind(name, version.min(6), qh, ())),
                "wl_seat" if s.seat.is_none() => s.seat = Some(reg.bind(name, version.min(5), qh, ())),
                // v3: format/modifier events, no feedback objects
                "zwp_linux_dmabuf_v1" if version >= 2 => s.dmabuf = Some(reg.bind(name, version.min(3), qh, ())),
                "wp_viewporter" => s.viewporter = Some(reg.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(State: wp_viewporter::WpViewporter);
delegate_noop!(State: wp_viewport::WpViewport);

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(s: &mut Self, buf: &wl_buffer::WlBuffer, ev: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = ev
            && let Some(b) = s.bufs.iter_mut().find(|b| b.buffer == *buf)
        {
            b.busy = false;
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for State {
    fn event(s: &mut Self, _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ev: zwp_linux_buffer_params_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => s.created = Some(Some(buffer)),
            zwp_linux_buffer_params_v1::Event::Failed => s.created = Some(None),
            _ => {}
        }
    }

    event_created_child!(State, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, ()),
    ]);
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(_: &mut Self, wm: &xdg_wm_base::XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(s: &mut Self, x: &xdg_surface::XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            x.ack_configure(serial);
            s.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(s: &mut Self, _: &xdg_toplevel::XdgToplevel, ev: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            xdg_toplevel::Event::Configure { width, height, states } => {
                if width > 0 && height > 0 {
                    s.size = (width as u32, height as u32);
                }
                let active = states
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .any(|v| matches!(xdg_toplevel::State::try_from(v), Ok(xdg_toplevel::State::Activated)));
                // The shell shows an app once its first buffer arrives, so the
                // first configures say "not active": that is not a reason to
                // stop drawing. Only a deactivation after we were shown is.
                let was = s.activated.unwrap_or(false);
                if active != was && (active || s.shown) {
                    s.events.push(PEvent::Visible(active));
                    if !active {
                        s.events.push(PEvent::ReleaseAll);
                    }
                }
                s.shown |= active;
                s.activated = Some(active);
            }
            xdg_toplevel::Event::Close => s.events.push(PEvent::Quit),
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(s: &mut Self, seat: &wl_seat::WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = ev
            && c.contains(wl_seat::Capability::Keyboard)
            && s.keyboard.is_none()
        {
            s.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(s: &mut Self, _: &wl_keyboard::WlKeyboard, ev: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_keyboard::Event::Key { key, state: WEnum::Value(st), .. } => {
                if let Some(b) = mortar_ui::input::map_evdev_key(key as u16) {
                    s.events.push(if st == wl_keyboard::KeyState::Pressed { PEvent::Press(b) } else { PEvent::Release(b) });
                }
            }
            wl_keyboard::Event::Leave { .. } => s.events.push(PEvent::ReleaseAll),
            _ => {}
        }
    }
}
