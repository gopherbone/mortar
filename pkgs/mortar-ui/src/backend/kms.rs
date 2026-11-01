// SPDX-License-Identifier: GPL-2.0-or-later
//! Linux: KMS dumb buffers + evdev. No GPU, no Mesa/GBM/EGL: the canvas is
//! copied into one of two XRGB8888 dumb buffers and page-flipped. This is the
//! same path `kms-gles pattern` proved on the Brick's sun4i-drm display.
//!
//! Environment:
//!   MORTAR_DRM_DEVICE  card to use (default: first /dev/dri/card* with a
//!                      connected connector)

use super::Backend;
use crate::gfx::Ui;
use crate::input::Event;
use drm::buffer::DrmFourcc;
use drm::control::{connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, PageFlipFlags};
use drm::Device;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::time::Duration;

use super::evdev_in;

struct Card(std::fs::File);
impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl Device for Card {}
impl ControlDevice for Card {}

pub struct Kms {
    card: Card,
    crtc: crtc::Handle,
    conn: connector::Handle,
    mode: drm::control::Mode,
    saved: Option<crtc::Info>,
    bufs: Vec<(DumbBuffer, framebuffer::Handle)>,
    front: usize,
    shown: bool,
    flip_pending: bool,
    w: u32,
    h: u32,
    input: evdev_in::Inputs,
    vt: Vt,
    released: bool,
}

fn open_card(path: &str) -> std::io::Result<Card> {
    Ok(Card(std::fs::OpenOptions::new().read(true).write(true).open(path)?))
}

impl Kms {
    pub fn open() -> Result<Kms, String> {
        let candidates: Vec<String> = match std::env::var("MORTAR_DRM_DEVICE") {
            Ok(p) => vec![p],
            Err(_) => {
                let mut v: Vec<String> = std::fs::read_dir("/dev/dri")
                    .map_err(|e| format!("/dev/dri: {e}"))?
                    .flatten()
                    .map(|e| e.path().to_string_lossy().into_owned())
                    .filter(|p| p.rsplit('/').next().is_some_and(|n| n.starts_with("card")))
                    .collect();
                v.sort();
                v
            }
        };
        let mut last_err = String::from("no DRM card found");
        for path in candidates {
            match Self::open_path(&path) {
                Ok(k) => return Ok(k),
                Err(e) => last_err = format!("{path}: {e}"),
            }
        }
        Err(last_err)
    }

    fn open_path(path: &str) -> Result<Kms, String> {
        let card = open_card(path).map_err(|e| e.to_string())?;
        let _ = card.acquire_master_lock(); // usually implicit for the first opener
        let res = card.resource_handles().map_err(|e| format!("resources: {e}"))?;
        let con = res
            .connectors()
            .iter()
            .flat_map(|c| card.get_connector(*c, true))
            .find(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
            .ok_or("no connected connector")?;
        let mode = con.modes()[0];
        // keep the CRTC the console already uses, else the first compatible one
        let crtc = con
            .current_encoder()
            .and_then(|e| card.get_encoder(e).ok())
            .and_then(|e| e.crtc())
            .or_else(|| {
                con.encoders()
                    .iter()
                    .flat_map(|e| card.get_encoder(*e))
                    .flat_map(|e| res.filter_crtcs(e.possible_crtcs()))
                    .next()
            })
            .ok_or("no CRTC for connector")?;
        let saved = card.get_crtc(crtc).ok();
        let (w, h) = mode.size();
        let (w, h) = (w as u32, h as u32);
        let mut bufs = Vec::new();
        for _ in 0..2 {
            let mut db = card
                .create_dumb_buffer((w, h), DrmFourcc::Xrgb8888, 32)
                .map_err(|e| format!("dumb buffer: {e}"))?;
            {
                let mut map = card.map_dumb_buffer(&mut db).map_err(|e| format!("map: {e}"))?;
                map.as_mut().fill(0);
            }
            let fb = card.add_framebuffer(&db, 24, 32).map_err(|e| format!("addfb: {e}"))?;
            bufs.push((db, fb));
        }
        let input = evdev_in::Inputs::open();
        Ok(Kms {
            card,
            crtc,
            conn: con.handle(),
            mode,
            saved,
            bufs,
            front: 0,
            shown: false,
            flip_pending: false,
            w,
            h,
            input,
            vt: Vt::graphics(),
            released: false,
        })
    }

    fn wait_flip(&mut self) {
        if !self.flip_pending {
            return;
        }
        let mut pfd = libc::pollfd { fd: self.card.as_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // a flip completes within a frame; don't hang forever if it doesn't
        if unsafe { libc::poll(&mut pfd, 1, 100) } > 0 {
            let _ = self.card.receive_events().map(|evs| evs.count());
        }
        self.flip_pending = false;
    }
}

impl Backend for Kms {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn present(&mut self, ui: &Ui) -> Result<(), String> {
        if self.released {
            return Ok(());
        }
        self.wait_flip();
        let back = if self.shown { 1 - self.front } else { self.front };
        let (w, h) = (self.w.min(ui.pm.width()), self.h.min(ui.pm.height()));
        {
            let (db, _) = &mut self.bufs[back];
            let pitch = drm::buffer::Buffer::pitch(db) as usize;
            let mut map = self.card.map_dumb_buffer(db).map_err(|e| format!("map: {e}"))?;
            let bytes = map.as_mut();
            let mut row = vec![0u32; w as usize];
            for y in 0..h {
                ui.xrgb_row(y, &mut row);
                let dst = &mut bytes[y as usize * pitch..y as usize * pitch + w as usize * 4];
                for (d, p) in dst.chunks_exact_mut(4).zip(&row) {
                    d.copy_from_slice(&p.to_ne_bytes());
                }
            }
        }
        let fb = self.bufs[back].1;
        if !self.shown {
            self.card
                .set_crtc(self.crtc, Some(fb), (0, 0), &[self.conn], Some(self.mode))
                .map_err(|e| format!("set_crtc: {e}"))?;
            self.shown = true;
        } else if self.card.page_flip(self.crtc, fb, PageFlipFlags::EVENT, None).is_ok() {
            self.flip_pending = true;
        } else {
            // some drivers lack page flip; a modeset still works
            self.card
                .set_crtc(self.crtc, Some(fb), (0, 0), &[self.conn], Some(self.mode))
                .map_err(|e| format!("set_crtc: {e}"))?;
        }
        self.front = back;
        Ok(())
    }

    fn wait(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) {
        self.input.wait(timeout, out);
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        self.wait_flip();
        if let Some(s) = &self.saved {
            if let Some(fb) = s.framebuffer() {
                let _ = self.card.set_crtc(self.crtc, Some(fb), s.position(), &[self.conn], s.mode());
            }
        }
        for (db, fb) in self.bufs.drain(..) {
            let _ = self.card.destroy_framebuffer(fb);
            let _ = self.card.destroy_dumb_buffer(db);
        }
        let _ = self.card.release_master_lock();
        self.vt.restore();
        self.released = true;
    }
}

impl Drop for Kms {
    fn drop(&mut self) {
        self.release();
    }
}

/// Switch our VT to graphics mode so the kernel console doesn't draw over
/// (or restore its framebuffer on top of) the UI. Best effort: greetd gives
/// the greeter its VT as stdin; in dev runs there may be no VT at all.
struct Vt {
    fd: Option<i32>,
}

const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_TEXT: libc::c_int = 0;
const KD_GRAPHICS: libc::c_int = 1;

impl Vt {
    fn graphics() -> Vt {
        let fd = if unsafe { libc::isatty(0) } == 1 { Some(0) } else { None };
        if let Some(fd) = fd {
            if unsafe { libc::ioctl(fd, KDSETMODE as _, KD_GRAPHICS) } != 0 {
                return Vt { fd: None };
            }
        }
        Vt { fd }
    }
    fn restore(&mut self) {
        if let Some(fd) = self.fd.take() {
            unsafe { libc::ioctl(fd, KDSETMODE as _, KD_TEXT) };
        }
    }
}
