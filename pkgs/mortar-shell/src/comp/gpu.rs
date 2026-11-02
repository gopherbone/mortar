// SPDX-License-Identifier: GPL-2.0-or-later
//! The GPU path for the shell's UI layers: two sun4i dumb buffers (CMA,
//! scanout-capable, 3 MB each at 1024x768) rendered by `gl::Gl` and put on
//! the primary plane with a page flip on the DrmCompositor's own surface.
//! When no UI layer shows, the DrmCompositor takes over again (direct
//! scanout of a fullscreen GPU app, or the pixman copy of an shm app).
//!
//! App images come straight from the clients' buffers: dma-bufs (GPU apps)
//! are imported once per buffer (mortar-wlegl rotates 2-4) and sampled with
//! no copy; wl_shm buffers are uploaded when their surface commits.
//! Textures of buffers that went away or were not drawn for a while are
//! freed after the frame that last used them has finished (we glFinish
//! every frame), so client CMA is not held longer than needed.
//!
//! The MENU's backdrop (`Src::Blur`) is the app's texture blurred into two
//! quarter-size FBOs (gl.rs `blur`); it is redone only when the app's
//! buffer changed, at most every `BLUR_EVERY`, and freed with the MENU.

use super::gl::{Fbo, Gl, Tex, Target};
use super::ui::BLUR_EVERY;
use super::state::Shell;
use super::ui::surface_buffer;
use crate::damage::{Grid, Info};
use crate::scene::{Layer, Scene, Src};
use mortar_ui::gfx::Color;
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::drm::DrmDeviceFd;
use smithay::reexports::drm::buffer::Buffer as DrmBuffer;
use smithay::reexports::drm::control::{dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice};
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::shm::with_buffer_contents;
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::Path;
use std::time::Instant;

struct Buf {
    db: Option<DumbBuffer>,
    fb: framebuffer::Handle,
    _fd: OwnedFd,
    target: Option<Target>,
    /// What it shows (to redraw only what changed): the image contents'
    /// epoch, the clear colour and the items' fingerprints.
    drawn: Option<(u64, Color, Vec<Info>)>,
}

struct Entry {
    tex: Tex,
    /// The client buffer (dma-buf entries) or the commit uploaded (shm).
    buffer: Option<WlBuffer>,
    commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    last: u64,
}

/// The blurred backdrop of one app.
struct Blur {
    uid: u32,
    /// What it was made from: the source texture and the surface commit.
    key: (u32, Option<smithay::backend::renderer::utils::CommitCounter>),
    at: Instant,
    a: Fbo,
    b: Fbo,
}

pub struct GpuUi {
    gl: Gl,
    drm: DrmDeviceFd,
    bufs: Vec<Buf>,
    /// The buffer on screen (after its flip completed), and the one queued.
    front: Option<usize>,
    queued: Option<usize>,
    dmabufs: HashMap<ObjectId, Entry>,
    shm: HashMap<ObjectId, Entry>,
    layers: HashMap<Layer, (u64, Tex)>,
    /// Pictures (`Src::Pic`: game thumbnails), uploaded once per id and
    /// freed when no frame shows them.
    pics: HashMap<u32, Tex>,
    frame: u64,
    blur: Option<Blur>,
    pub render_ms: f32,
    /// Smoothed frame time (ms), and the last blur's GPU time (ms, after
    /// glFinish) and how many blurs ran.
    pub render_avg_ms: f32,
    pub blur_ms: f32,
    pub blur_count: u64,
    /// The newest backdrop blur's mean colour, for the MENU's veil.
    mean: Option<[f32; 3]>,
    /// Bumped whenever an image a frame may sample changed (a buffer
    /// that holds an older epoch is drawn in full).
    epoch: u64,
    /// The apps' buffers and commits last seen.
    seen: Vec<(u32, ObjectId, smithay::backend::renderer::utils::CommitCounter)>,
    /// Redraw only what changed (MORTAR_SHELL_FULL_REDRAW=1: always all).
    partial: bool,
    /// Share of the screen redrawn, smoothed (0..1).
    pub damage_avg: f32,
    /// `ctl gpucheck`: compare the next partial frame with a full one.
    check_next: bool,
    check: Option<String>,
}

/// A buffer's pixels as RGBA (slow: write-combined memory).
fn read_buf(drm: &DrmDeviceFd, b: &mut Buf) -> Option<(u32, u32, Vec<u8>)> {
    let db = b.db.as_mut()?;
    let (w, h) = db.size();
    let pitch = db.pitch() as usize;
    let m = drm.map_dumb_buffer(db).ok()?;
    let src = m.as_ref();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = &src[y * pitch..y * pitch + w as usize * 4];
        for p in row.chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    Some((w, h, out))
}

impl GpuUi {
    pub fn new(drm: &DrmDeviceFd, size: (i32, i32), dir: &Path, msaa: u32) -> Result<GpuUi, String> {
        let gl = Gl::new(dir, msaa)?;
        tracing::info!("GPU UI: {}", gl.info);
        let mut me = GpuUi {
            gl,
            drm: drm.clone(),
            bufs: Vec::new(),
            front: None,
            queued: None,
            dmabufs: HashMap::new(),
            shm: HashMap::new(),
            layers: HashMap::new(),
            pics: HashMap::new(),
            frame: 0,
            blur: None,
            render_ms: 0.0,
            render_avg_ms: 0.0,
            blur_ms: 0.0,
            blur_count: 0,
            mean: None,
            epoch: 0,
            seen: Vec::new(),
            partial: std::env::var("MORTAR_SHELL_FULL_REDRAW").map_or(true, |v| v.is_empty() || v == "0"),
            damage_avg: 1.0,
            check_next: false,
            check: None,
        };
        for _ in 0..2 {
            match me.alloc(size) {
                Ok(b) => me.bufs.push(b),
                Err(e) => {
                    me.destroy();
                    return Err(e);
                }
            }
        }
        Ok(me)
    }

    fn alloc(&self, size: (i32, i32)) -> Result<Buf, String> {
        let mut db = self
            .drm
            .create_dumb_buffer((size.0 as u32, size.1 as u32), Fourcc::Xrgb8888, 32)
            .map_err(|e| format!("dumb buffer: {e}"))?;
        let fb = match self.drm.add_framebuffer(&db, 24, 32) {
            Ok(fb) => fb,
            Err(e) => {
                let _ = self.drm.destroy_dumb_buffer(db);
                return Err(format!("framebuffer: {e}"));
            }
        };
        let fd = match self.drm.buffer_to_prime_fd(db.handle(), libc::O_CLOEXEC as u32 | libc::O_RDWR as u32) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = self.drm.destroy_framebuffer(fb);
                let _ = self.drm.destroy_dumb_buffer(db);
                return Err(format!("PRIME export: {e}"));
            }
        };
        // start black (the first frame may show before anything renders)
        if let Ok(mut m) = self.drm.map_dumb_buffer(&mut db) {
            m.as_mut().fill(0);
        }
        let target = match self.gl.target(fd.as_raw_fd(), size.0, size.1, db.pitch(), Fourcc::Xrgb8888 as u32) {
            Ok(t) => t,
            Err(e) => {
                let _ = self.drm.destroy_framebuffer(fb);
                let _ = self.drm.destroy_dumb_buffer(db);
                return Err(e);
            }
        };
        Ok(Buf { db: Some(db), fb, _fd: fd, target: Some(target), drawn: None })
    }

    /// The buffer on screen, as RGBA (debugging: `ctl gpushot`). Reads
    /// write-combined memory, so it is slow (~50 ms).
    pub fn read_front(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let i = self.front.or(self.queued)?;
        read_buf(&self.drm, self.bufs.get_mut(i)?)
    }

    /// Check the next partly redrawn frame against the same frame drawn in
    /// full (debugging: `ctl gpucheck`); returns the last check's result.
    pub fn check_next(&mut self) -> String {
        self.check_next = true;
        self.check.clone().unwrap_or_else(|| "no check yet".into())
    }

    fn free_buf(&self, mut b: Buf) {
        if let Some(t) = b.target.take() {
            self.gl.drop_target(t);
        }
        let _ = self.drm.destroy_framebuffer(b.fb);
        if let Some(db) = b.db.take() {
            let _ = self.drm.destroy_dumb_buffer(db);
        }
    }

    /// A page flip of ours completed.
    pub fn flipped(&mut self) {
        if let Some(q) = self.queued.take() {
            self.front = Some(q);
        }
    }

    /// The compositor showed one of its own buffers: ours are free.
    pub fn released(&mut self) {
        self.front = None;
        self.queued = None;
    }

    /// Render a frame; returns the framebuffer to flip to.
    pub fn render(&mut self, shell: &mut Shell, scene: &Scene) -> Result<framebuffer::Handle, String> {
        let t0 = Instant::now();
        self.frame += 1;
        let idx = (0..self.bufs.len()).find(|i| Some(*i) != self.front && Some(*i) != self.queued).ok_or("no free buffer")?;
        self.prepare(shell, scene);
        // what changed since this buffer was drawn
        let mut infos = Vec::new();
        let rects = match (self.partial, shell.typo.as_ref(), self.bufs[idx].drawn.take()) {
            (true, Some(t), Some((epoch, clear, old))) if epoch == self.epoch && clear == scene.clear => {
                self.gl.infos(scene, t, &mut infos);
                let mut g = Grid::new(scene.w, scene.h);
                g.diff(&old, &infos);
                g.rects()
            }
            (true, Some(t), _) => {
                self.gl.infos(scene, t, &mut infos);
                None
            }
            _ => None,
        };
        let share = rects.as_ref().map_or(1.0, |r| r.iter().map(|r| r[2] * r[3]).sum::<f32>() / (scene.w * scene.h));
        // `ctl gpucheck`: the same frame in full, into a spare buffer
        let mut spare = None;
        if self.check_next && rects.is_some() {
            self.check_next = false;
            match self.bufs[idx].target.as_ref().map(|t| (t.w, t.h)).map(|s| self.alloc(s)) {
                Some(Ok(b)) => spare = Some(b),
                Some(Err(e)) => self.check = Some(format!("no spare buffer: {e}")),
                None => {}
            }
        }
        self.damage_avg = self.damage_avg * 0.9 + share * 0.1;
        if let Some(m) = self.mean.take() {
            shell.backdrop_rgb = Some(m);
        }
        let gl = &mut self.gl;
        let dmabufs = &self.dmabufs;
        let shm = &self.shm;
        let layers = &self.layers;
        let pics = &self.pics;
        let surfaces: HashMap<u32, (ObjectId, Option<ObjectId>)> = scene
            .apps()
            .into_iter()
            .filter_map(|u| {
                let s = shell.app_surface(u)?;
                let b = surface_buffer(&s).map(|(b, _)| b.id());
                Some((u, (s.id(), b)))
            })
            .collect();
        let blur = &self.blur;
        let lookup = |src: Src| -> Option<(u32, bool, bool, i32, i32)> {
            let t = match src {
                Src::App(u) => {
                    let (sid, bid) = surfaces.get(&u)?;
                    bid.as_ref().and_then(|b| dmabufs.get(b)).or_else(|| shm.get(sid)).map(|e| &e.tex)?
                }
                Src::Blur(u) => {
                    let b = blur.as_ref().filter(|b| b.uid == u)?;
                    return Some((b.a.tex, false, true, b.a.w, b.a.h));
                }
                Src::Layer(l) => &layers.get(&l)?.1,
                Src::Pic(p) => pics.get(&p)?,
            };
            Some((t.tex, t.swizzle, t.opaque, t.w, t.h))
        };
        let target = self.bufs[idx].target.as_ref().ok_or("no target")?;
        let typo = shell.typo.take();
        let only = rects.as_deref().map(|r| (&infos[..], r));
        let r = match &typo {
            Some(t) => gl.render(target, scene, t, &lookup, only).and_then(|_| match spare.as_ref().and_then(|b| b.target.as_ref()) {
                Some(st) => gl.render(st, scene, t, &lookup, None),
                None => Ok(()),
            }),
            None => Err("no fonts".into()),
        };
        shell.typo = typo;
        if let Err(e) = r {
            if let Some(b) = spare {
                self.free_buf(b);
            }
            return Err(e);
        }
        // no implicit fencing in this DDK: the frame must be done before the flip
        gl.finish();
        gl.check("finish")?;
        if let Some(mut b) = spare {
            let n = rects.as_ref().map_or(0, |r| r.len());
            let (a, f) = (read_buf(&self.drm, &mut self.bufs[idx]), read_buf(&self.drm, &mut b));
            self.check = Some(match (a, f) {
                (Some((_, _, a)), Some((_, _, f))) if a.len() == f.len() => {
                    let d: Vec<u8> = a.iter().zip(&f).map(|(x, y)| x.abs_diff(*y)).collect();
                    let off = d.chunks_exact(4).filter(|p| p.iter().any(|c| *c > 2)).count();
                    format!("{off} px differ (most by {}) after redrawing {:.1}% in {n} rects", d.iter().max().unwrap_or(&0), share * 100.0)
                }
                _ => "could not read the buffers".into(),
            });
            tracing::info!("gpucheck: {}", self.check.as_deref().unwrap_or(""));
            self.free_buf(b);
        }
        self.prune();
        if self.partial && !infos.is_empty() {
            self.bufs[idx].drawn = Some((self.epoch, scene.clear, infos));
        }
        self.queued = Some(idx);
        self.render_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.render_avg_ms = if self.render_avg_ms == 0.0 { self.render_ms } else { self.render_avg_ms * 0.9 + self.render_ms * 0.1 };
        Ok(self.bufs[idx].fb)
    }

    /// The flip was refused: the buffer is free again.
    pub fn unqueue(&mut self) {
        self.queued = None;
    }

    /// Import/upload the images this scene samples.
    fn prepare(&mut self, shell: &Shell, scene: &Scene) {
        // a new buffer or commit of an app on screen: everything is drawn
        // again (an app's buffers are reused with new contents)
        let seen: Vec<_> = scene
            .apps()
            .into_iter()
            .filter_map(|u| {
                let s = shell.app_surface(u)?;
                let (b, c) = surface_buffer(&s)?;
                Some((u, b.id(), c))
            })
            .collect();
        if seen != self.seen {
            self.seen = seen;
            self.epoch += 1;
        }
        let before = self.images_stamp();
        for uid in scene.apps() {
            let Some(s) = shell.app_surface(uid) else { continue };
            let Some((buf, commit)) = surface_buffer(&s) else { continue };
            if let Ok(d) = smithay::wayland::dmabuf::get_dmabuf(&buf) {
                if let Some(e) = self.dmabufs.get_mut(&buf.id()) {
                    e.last = self.frame;
                    continue;
                }
                let size = d.size();
                let (Some(fd), Some(stride), Some(off)) = (d.handles().next(), d.strides().next(), d.offsets().next()) else { continue };
                if d.num_planes() != 1 {
                    continue;
                }
                match self.gl.import_dmabuf(fd.as_fd().as_raw_fd(), size.w, size.h, stride, off, d.format().code as u32) {
                    Ok(tex) => {
                        self.dmabufs.insert(buf.id(), Entry { tex, buffer: Some(buf.clone()), commit: None, last: self.frame });
                    }
                    Err(e) => tracing::debug!("client dma-buf import: {e}"),
                }
                continue;
            }
            let sid = s.id();
            if let Some(e) = self.shm.get_mut(&sid) {
                e.last = self.frame;
                if e.commit == Some(commit) {
                    continue;
                }
            }
            let old = self.shm.remove(&sid).map(|e| e.tex);
            let gl = &self.gl;
            let r = with_buffer_contents(&buf, |ptr, len, d| {
                if !matches!(d.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
                    return Err("format".to_string());
                }
                let (w, h, stride, off) = (d.width, d.height, d.stride as usize, d.offset as usize);
                if off + stride * h as usize > len {
                    return Err("short buffer".into());
                }
                // SAFETY: bounds checked above
                let px = unsafe { std::slice::from_raw_parts(ptr.add(off), stride * h as usize) };
                gl.upload(old, w, h, stride, px, true, true)
            });
            match r {
                Ok(Ok(tex)) => {
                    self.shm.insert(sid, Entry { tex, buffer: None, commit: Some(commit), last: self.frame });
                }
                Ok(Err(e)) => tracing::debug!("shm upload: {e}"),
                Err(e) => tracing::debug!("shm buffer: {e:?}"),
            }
        }
        self.prepare_blur(shell, scene);
        // pictures: uploaded once, dropped when unused
        let used = scene.pics();
        let gone: Vec<u32> = self.pics.keys().filter(|p| !used.contains(p)).copied().collect();
        for p in gone {
            if let Some(t) = self.pics.remove(&p) {
                self.gl.drop_tex(t);
            }
        }
        for p in used {
            if self.pics.contains_key(&p) {
                continue;
            }
            let Some(pm) = shell.cpu_imgs.pics.get(&p) else { continue };
            match self.gl.upload(None, pm.width() as i32, pm.height() as i32, pm.width() as usize * 4, pm.data(), false, false) {
                Ok(t) => {
                    self.pics.insert(p, t);
                }
                Err(e) => tracing::debug!("picture upload: {e}"),
            }
        }
        // CPU layers
        for (l, pm) in [(Layer::Osk, shell.cpu_imgs.osk.as_ref()), (Layer::Page, shell.cpu_imgs.page.as_ref())] {
            let Some(pm) = pm else {
                if let Some((_, t)) = self.layers.remove(&l) {
                    self.gl.drop_tex(t);
                }
                continue;
            };
            if self.layers.get(&l).is_some_and(|(g, _)| *g == shell.layer_gen) {
                continue;
            }
            let old = self.layers.remove(&l).map(|x| x.1);
            match self.gl.upload(old, pm.width() as i32, pm.height() as i32, pm.width() as usize * 4, pm.data(), false, false) {
                Ok(t) => {
                    self.layers.insert(l, (shell.layer_gen, t));
                }
                Err(e) => tracing::debug!("layer upload: {e}"),
            }
        }
        // a blur done, a picture or a layer come, gone or redrawn
        if self.images_stamp() != before {
            self.epoch += 1;
        }
    }

    /// Changes when the blurs, pictures or layers do.
    fn images_stamp(&self) -> (u64, usize, u64, usize) {
        (self.blur_count, self.pics.len(), self.layers.values().map(|(g, _)| *g).sum(), self.layers.len())
    }

    /// Blur the backdrop app again if its buffer changed (at most every
    /// BLUR_EVERY); free the FBOs when no MENU shows it.
    fn prepare_blur(&mut self, shell: &Shell, scene: &Scene) {
        let Some(uid) = scene.blurs().first().copied() else {
            if let Some(b) = self.blur.take() {
                self.gl.drop_fbo(b.a);
                self.gl.drop_fbo(b.b);
            }
            return;
        };
        let Some(s) = shell.app_surface(uid) else { return };
        let Some((buf, commit)) = surface_buffer(&s) else { return };
        let src = self.dmabufs.get(&buf.id()).or_else(|| self.shm.get(&s.id())).map(|e| (e.tex.tex, e.tex.swizzle, e.tex.opaque, e.tex.w, e.tex.h));
        let Some((tex, swz, opq, w, h)) = src else { return };
        let key = (tex, Some(commit));
        let (bw, bh) = ((w / 4).max(1), (h / 4).max(1));
        // a different app or size: new targets
        if self.blur.as_ref().is_some_and(|b| b.uid != uid || b.a.w != bw || b.a.h != bh)
            && let Some(b) = self.blur.take()
        {
            self.gl.drop_fbo(b.a);
            self.gl.drop_fbo(b.b);
        }
        if self.blur.is_none() {
            let a = match self.gl.fbo(bw, bh) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("blur: {e}");
                    return;
                }
            };
            let b = match self.gl.fbo(bw, bh) {
                Ok(b) => b,
                Err(e) => {
                    self.gl.drop_fbo(a);
                    tracing::warn!("blur: {e}");
                    return;
                }
            };
            self.blur = Some(Blur { uid, key: (0, None), at: Instant::now() - BLUR_EVERY, a, b });
        }
        let Some(bl) = self.blur.as_mut() else { return };
        if bl.key == key || bl.at.elapsed() < BLUR_EVERY {
            return;
        }
        let t0 = Instant::now();
        match self.gl.blur(tex, swz, opq, w, h, &bl.a, &bl.b) {
            Ok(()) => {
                bl.key = key;
                bl.at = Instant::now();
                self.blur_count += 1;
                // the blur's own GPU time (the frame is finished anyway
                // before its flip, so waiting here costs nothing extra)
                self.gl.finish();
                self.blur_ms = t0.elapsed().as_secs_f32() * 1000.0;
                self.mean = self.gl.mean_rgb(&bl.a);
            }
            Err(e) => tracing::warn!("blur: {e}"),
        }
    }

    /// A client buffer was destroyed: free its texture (every frame is
    /// finished before the next starts, so nothing on the GPU uses it).
    pub fn forget(&mut self, id: &ObjectId) {
        if let Some(e) = self.dmabufs.remove(id) {
            self.gl.drop_tex(e.tex);
        }
    }

    /// Free textures nobody draws (called after glFinish).
    fn prune(&mut self) {
        let f = self.frame;
        let dead_d: Vec<ObjectId> = self
            .dmabufs
            .iter()
            .filter(|(_, e)| !e.buffer.as_ref().is_some_and(|b| b.is_alive()) || f - e.last >= 120)
            .map(|(k, _)| k.clone())
            .collect();
        for k in dead_d {
            if let Some(e) = self.dmabufs.remove(&k) {
                self.gl.drop_tex(e.tex);
            }
        }
        let dead_s: Vec<ObjectId> = self.shm.iter().filter(|(_, e)| f - e.last >= 120).map(|(k, _)| k.clone()).collect();
        for k in dead_s {
            if let Some(e) = self.shm.remove(&k) {
                self.gl.drop_tex(e.tex);
            }
        }
    }

    /// Orderly teardown: finish, free everything, release EGL.
    pub fn destroy(&mut self) {
        self.gl.finish();
        for (_, e) in self.dmabufs.drain().chain(self.shm.drain()).collect::<Vec<_>>() {
            self.gl.drop_tex(e.tex);
        }
        for t in self.pics.drain().map(|e| e.1).collect::<Vec<_>>() {
            self.gl.drop_tex(t);
        }
        for (_, (_, t)) in self.layers.drain().collect::<Vec<_>>() {
            self.gl.drop_tex(t);
        }
        if let Some(b) = self.blur.take() {
            self.gl.drop_fbo(b.a);
            self.gl.drop_fbo(b.b);
        }
        for b in &mut self.bufs {
            if let Some(t) = b.target.take() {
                self.gl.drop_target(t);
            }
        }
        self.gl.teardown();
        for b in self.bufs.drain(..) {
            let _ = self.drm.destroy_framebuffer(b.fb);
            if let Some(db) = b.db {
                let _ = self.drm.destroy_dumb_buffer(db);
            }
        }
    }
}

