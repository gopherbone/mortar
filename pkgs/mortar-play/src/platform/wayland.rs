// SPDX-License-Identifier: GPL-2.0-or-later
//! A minimal Wayland client for mortar-shell: one fullscreen xdg_toplevel,
//! two wl_shm XRGB8888 buffers, frames scaled on the CPU into whichever
//! buffer the compositor has released, damage limited to the game area.
//! `activated` from the toplevel configure says whether we are on screen
//! (the shell deactivates apps behind its overlay and in the background),
//! which the app turns into pause/resume. Keys from a real keyboard come
//! through wl_keyboard as evdev codes.

use super::{Display, PEvent, ui_to_xrgb};
use crate::video::{Dst, Frame, Scaler};
use mortar_ui::gfx::Ui;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::Duration;
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

struct Buf {
    buffer: wl_buffer::WlBuffer,
    _pool: wl_shm_pool::WlShmPool,
    _fd: OwnedFd,
    ptr: *mut u32,
    len: usize,
    busy: bool,
    /// What the area outside the game rect holds: Some(dst) when the borders
    /// were cleared for that layout.
    layout: Option<Dst>,
}

impl Buf {
    fn px(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len / 4) }
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        self.buffer.destroy();
        unsafe { libc::munmap(self.ptr as *mut _, self.len) };
    }
}

#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    configured: bool,
    size: (u32, u32),
    activated: Option<bool>,
    bufs: Vec<Buf>,
    events: Vec<PEvent>,
}

pub struct WaylandDisplay {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    surface: wl_surface::WlSurface,
    _xdg: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    scaler: Scaler,
    buf_size: (u32, u32),
    fallback: (u32, u32),
}

fn shm_buffer(shm: &wl_shm::WlShm, w: u32, h: u32, idx: usize, qh: &QueueHandle<State>) -> Result<Buf, String> {
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
        let buffer = pool.create_buffer(0, w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Xrgb8888, qh, idx);
        Ok(Buf { buffer, _pool: pool, _fd: fd, ptr: ptr as *mut u32, len, busy: false, layout: None })
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
        eprintln!("mortar-play: Wayland {}x{}", state.size.0, state.size.1);
        Ok(WaylandDisplay { conn, queue, qh, state, surface, _xdg: xdg, _toplevel: toplevel, scaler: Scaler::default(), buf_size: (0, 0), fallback: (w, h) })
    }

    fn screen(&self) -> (u32, u32) {
        if self.state.size.0 > 0 && self.state.size.1 > 0 { self.state.size } else { self.fallback }
    }

    /// A released buffer of the current size (recreating the pair on resize).
    fn free_buffer(&mut self) -> Option<usize> {
        let (w, h) = self.screen();
        if self.buf_size != (w, h) {
            self.state.bufs.clear();
            let shm = self.state.shm.clone()?;
            for i in 0..2 {
                match shm_buffer(&shm, w, h, i, &self.qh) {
                    Ok(b) => self.state.bufs.push(b),
                    Err(e) => {
                        eprintln!("mortar-play: {e}");
                        return None;
                    }
                }
            }
            self.buf_size = (w, h);
        }
        self.state.bufs.iter().position(|b| !b.busy)
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

    fn present_game(&mut self, f: &Frame, dst: Dst, _sharp: bool) {
        self.dispatch(None);
        let Some(i) = self.free_buffer() else { return }; // compositor still has both: drop this frame
        let stride = self.buf_size.0 as usize;
        let full = Dst { x: 0, y: 0, w: self.buf_size.0, h: self.buf_size.1 };
        let b = &mut self.state.bufs[i];
        let damage = if b.layout != Some(dst) {
            b.px().fill(0);
            b.layout = Some(dst);
            full
        } else {
            dst
        };
        let px = self.state.bufs[i].px();
        self.scaler.scale(f, dst, px, stride);
        self.commit(i, damage);
    }

    fn present_ui(&mut self, ui: &Ui) {
        self.dispatch(None);
        let Some(i) = self.free_buffer() else { return };
        let (w, h) = self.buf_size;
        if ui.pm.width() != w || ui.pm.height() != h {
            return; // the app re-renders at the new size
        }
        let px = self.state.bufs[i].px();
        ui_to_xrgb(ui, px, w as usize);
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
        "wayland-shm".into()
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
                _ => {}
            }
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);

impl Dispatch<wl_buffer::WlBuffer, usize> for State {
    fn event(s: &mut Self, _: &wl_buffer::WlBuffer, ev: wl_buffer::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = ev
            && let Some(b) = s.bufs.get_mut(*idx)
        {
            b.busy = false;
        }
    }
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
                if s.activated != Some(active) {
                    s.activated = Some(active);
                    s.events.push(PEvent::Visible(active));
                    if !active {
                        s.events.push(PEvent::ReleaseAll);
                    }
                }
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
