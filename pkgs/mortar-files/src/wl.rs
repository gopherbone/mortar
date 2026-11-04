// SPDX-License-Identifier: GPL-2.0-or-later
//! The window (Linux): one fullscreen xdg_toplevel drawn in wl_shm buffers,
//! keys from wl_keyboard. One poll() sleeps on the Wayland socket and the
//! workers' wake pipe, with a timeout only when the app has a deadline. Frames
//! are drawn when something changed, and while something moves, paced by the
//! compositor's frame callbacks; at rest nothing runs at all.

use crate::app::App;
use crate::view;
use mortar_ui::gfx::{Fonts, Ui};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_callback::{self, WlCallback},
    wl_compositor::WlCompositor,
    wl_keyboard::{self, WlKeyboard},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

struct Buf {
    id: usize,
    buffer: WlBuffer,
    ptr: *mut u32,
    len: usize,
    w: i32,
    h: i32,
    busy: bool,
    _fd: OwnedFd,
}

impl Drop for Buf {
    fn drop(&mut self) {
        self.buffer.destroy();
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len * 4);
        }
    }
}

#[derive(Default)]
struct State {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    wm: Option<XdgWmBase>,
    seat: Option<WlSeat>,
    keyboard: Option<WlKeyboard>,
    configured: bool,
    size: (i32, i32),
    activated: bool,
    closed: bool,
    frame_pending: bool,
    keys: Vec<(u16, bool)>,
    focus_lost: bool,
    repeat: Option<(i32, i32)>,
    bufs: Vec<Buf>,
    next_buf: usize,
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(st: &mut Self, reg: &WlRegistry, ev: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "wl_compositor" => st.compositor = Some(reg.bind(name, version.min(4), qh, ())),
                "wl_shm" => st.shm = Some(reg.bind(name, 1, qh, ())),
                "xdg_wm_base" => st.wm = Some(reg.bind(name, version.min(2), qh, ())),
                "wl_seat" if st.seat.is_none() => st.seat = Some(reg.bind(name, version.min(5), qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(_: &mut Self, wm: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(st: &mut Self, s: &XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            s.ack_configure(serial);
            st.configured = true;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(st: &mut Self, _: &XdgToplevel, ev: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            xdg_toplevel::Event::Configure { width, height, states } => {
                if width > 0 && height > 0 {
                    st.size = (width, height);
                }
                st.activated = states.chunks_exact(4).any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == xdg_toplevel::State::Activated as u32);
            }
            xdg_toplevel::Event::Close => st.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(st: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = ev
            && c.contains(wl_seat::Capability::Keyboard)
            && st.keyboard.is_none()
        {
            st.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(st: &mut Self, _: &WlKeyboard, ev: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_keyboard::Event::Key { key, state, .. } => {
                let down = matches!(state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                st.keys.push((key as u16, down));
            }
            wl_keyboard::Event::Keymap { fd, .. } => drop(fd), // fixed US map (keys.rs)
            wl_keyboard::Event::Leave { .. } => st.focus_lost = true,
            wl_keyboard::Event::RepeatInfo { rate, delay } => st.repeat = Some((rate, delay)),
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, usize> for State {
    fn event(st: &mut Self, _: &WlBuffer, ev: wl_buffer::Event, id: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = ev
            && let Some(b) = st.bufs.iter_mut().find(|b| b.id == *id)
        {
            b.busy = false;
        }
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn event(st: &mut Self, _: &WlCallback, ev: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = ev {
            st.frame_pending = false;
        }
    }
}

delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);

impl State {
    fn new_buffer(&self, w: i32, h: i32, id: usize, qh: &QueueHandle<State>) -> Result<Buf, String> {
        let len = (w * h) as usize;
        let fd = unsafe {
            let fd = libc::memfd_create(c"mortar-files".as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return Err("memfd_create failed".into());
            }
            OwnedFd::from_raw_fd(fd)
        };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), (len * 4) as libc::off_t) } != 0 {
            return Err("ftruncate failed".into());
        }
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len * 4, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err("mmap failed".into());
        }
        let shm = self.shm.as_ref().ok_or("no wl_shm")?;
        let pool = shm.create_pool(fd.as_fd(), (len * 4) as i32, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, id);
        pool.destroy();
        Ok(Buf { id, buffer, ptr: ptr as *mut u32, len, w, h, busy: false, _fd: fd })
    }
}

/// Open the window and run until the app is closed.
pub fn run(app: &mut App, wake_fd: i32) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut st = State { size: (1024, 768), ..Default::default() };
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;
    let compositor = st.compositor.clone().ok_or("no wl_compositor")?;
    let wm = st.wm.clone().ok_or("no xdg_wm_base")?;
    let surface = compositor.create_surface(&qh, ());
    let xs = wm.get_xdg_surface(&surface, &qh, ());
    let top = xs.get_toplevel(&qh, ());
    top.set_title("Files".into());
    top.set_app_id("mortar-files".into());
    surface.commit();
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;

    let font_dir = mortar_ui::gfx::font_dir();
    // one canvas per size: the short window under the keyboard comes and goes
    let mut uis: Vec<Ui> = Vec::new();
    let start = Instant::now();
    let mut held: Option<(u16, Instant)> = None;
    let mut activated = false;
    let mut drawn = false;

    loop {
        // ------------------------------------------------ sleep
        let ms = start.elapsed().as_millis() as u64;
        app.tick(ms);
        let mut timeout: Option<Duration> = app.next_deadline().map(|t| Duration::from_millis(t.saturating_sub(ms)));
        if let Some((_, t)) = held {
            let left = t.saturating_duration_since(Instant::now());
            timeout = Some(timeout.map_or(left, |x| x.min(left)));
        }
        conn.flush().map_err(|e| e.to_string())?;
        if let Some(guard) = queue.prepare_read() {
            let wfd = guard.connection_fd().as_raw_fd();
            let mut fds = [libc::pollfd { fd: wfd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: wake_fd, events: libc::POLLIN, revents: 0 }];
            let t = timeout.map_or(-1, |d| d.as_millis().min(60_000) as i32);
            let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, t) };
            if r > 0 && fds[0].revents & libc::POLLIN != 0 {
                guard.read().map_err(|e| e.to_string())?;
            } else if r > 0 && fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                return Err("compositor went away".into());
            } else {
                drop(guard);
            }
            if r > 0 && fds[1].revents & libc::POLLIN != 0 {
                crate::bg::drain(wake_fd);
            }
        }
        queue.dispatch_pending(&mut st).map_err(|e| e.to_string())?;
        if st.closed || app.done {
            return Ok(());
        }
        app.poll();

        // ------------------------------------------------ state from the shell
        if st.activated != activated {
            activated = st.activated;
            app.focus(activated);
            // the compositor may have sat on the callback while we were behind
            st.frame_pending = false;
        }
        app.resized(st.size.1);

        // ------------------------------------------------ keys
        if st.focus_lost {
            st.focus_lost = false;
            held = None;
            app.focus(false);
        }
        for (k, down) in std::mem::take(&mut st.keys) {
            let ms = start.elapsed().as_millis() as u64;
            app.tick(ms);
            if down && app.repeats(k) {
                let delay = st.repeat.map(|r| r.1).unwrap_or(380).max(150) as u64;
                held = Some((k, Instant::now() + Duration::from_millis(delay)));
            } else if !down && matches!(held, Some((h, _)) if h == k) {
                held = None;
            }
            app.key(k, down, false);
        }
        if let Some((k, t)) = held
            && Instant::now() >= t
        {
            let rate = st.repeat.map(|r| r.0).unwrap_or(14).clamp(1, 30) as u64;
            held = Some((k, t + Duration::from_millis(1000 / rate)));
            app.tick(start.elapsed().as_millis() as u64);
            if app.repeats(k) {
                app.key(k, true, true);
            } else {
                held = None;
            }
        }
        if app.done {
            return Ok(());
        }

        // ------------------------------------------------ draw
        // Behind other things there's nothing to draw for, and a job running
        // in the background costs no frames. The first frame comes regardless:
        // the shell shows an app when it has one.
        let wanted = app.dirty || app.animating();
        if !wanted || !st.configured || st.frame_pending || (drawn && !activated) {
            continue;
        }
        let (w, h) = st.size;
        let i = match uis.iter().position(|u| (u.width() as i32, u.height() as i32) == (w, h)) {
            Some(i) => i,
            None => {
                uis.push(Ui::new(w as u32, h as u32, Fonts::load(&font_dir)?));
                uis.len() - 1
            }
        };
        let u = &mut uis[i];
        st.bufs.retain(|b| b.busy || (b.w == w && b.h == h));
        let idx = match st.bufs.iter().position(|b| !b.busy && b.w == w && b.h == h) {
            Some(i) => i,
            None => {
                if st.bufs.len() >= 3 {
                    continue; // all busy; a release wakes the socket
                }
                st.next_buf += 1;
                let b = st.new_buffer(w, h, st.next_buf, &qh)?;
                st.bufs.push(b);
                st.bufs.len() - 1
            }
        };
        app.dirty = false;
        view::draw(app, u);
        let b = &mut st.bufs[idx];
        let px = unsafe { std::slice::from_raw_parts_mut(b.ptr, b.len) };
        for y in 0..h as usize {
            u.xrgb_row(y as u32, &mut px[y * w as usize..(y + 1) * w as usize]);
        }
        b.busy = true;
        surface.attach(Some(&b.buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.frame(&qh, ());
        st.frame_pending = true;
        surface.commit();
        drawn = true;
    }
}
