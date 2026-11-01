// SPDX-License-Identifier: GPL-2.0-or-later
//! Output backends and composition.
//!
//! Composition is generic over the Smithay renderer (`elements`), so a GLES
//! renderer for the PowerVR can replace pixman later without touching the
//! shell logic (see DESIGN.md, "Rendering"). Today both backends use
//! Smithay's pixman (CPU) renderer:
//!
//! - `Headless`: renders into an offscreen image at most once per frame
//!   request; used by tests and `--backend headless`.
//! - `Drm`: libseat session (logind or seatd) + DRM/KMS on the panel through
//!   Smithay's DrmCompositor with GBM-allocated scanout buffers (Mesa's
//!   kms_swrast on sun4i). When the only element is a client dmabuf that
//!   covers the screen, DrmCompositor scans it out directly (no copy);
//!   shm clients (Mesa llvmpipe on Wayland) cost one copy per frame.

use super::state::Shell;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{render_elements, AsRenderElements, Kind};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, Color32F, ImportAll, ImportDma, ImportMem, Offscreen, Renderer};
use smithay::reexports::pixman;
use smithay::utils::{Physical, Point, Scale, Transform};
use std::path::Path;

pub use super::drm::DrmBackend;

/// Behind every client (letterboxing).
pub const CLEAR: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);

render_elements! {
    pub ShellElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Memory=MemoryRenderBufferRenderElement<R>,
}

/// What is on screen, front to back.
pub fn elements<R>(shell: &Shell, r: &mut R) -> Vec<ShellElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut out: Vec<ShellElement<R>> = Vec::new();
    let mem = |r: &mut R, loc: (f64, f64), buf| {
        MemoryRenderBufferRenderElement::from_buffer(r, loc, buf, None, None, None, Kind::Unspecified).ok()
    };
    if shell.shell_screen() {
        if let Some(e) = mem(r, (0.0, 0.0), &shell.overlay_buf) {
            out.push(ShellElement::Memory(e));
        }
        return out;
    }
    if let Some(b) = &shell.osd_buf {
        let x = (shell.size.0 - shell.osd_w - 16) as f64;
        if let Some(e) = mem(r, (x, 16.0), b) {
            out.push(ShellElement::Memory(e));
        }
    }
    let area = shell.app_area();
    if shell.osk.visible
        && let Some(e) = mem(r, (0.0, (shell.size.1 - shell.osk_height()) as f64), &shell.osk_buf)
    {
        out.push(ShellElement::Memory(e));
    }
    if let Some(w) = shell.visible_window() {
        // centre clients smaller than the area; offset client-side shadows
        let geo = w.geometry();
        let x = ((area.w - geo.size.w) / 2).max(0) - geo.loc.x;
        let y = ((area.h - geo.size.h) / 2).max(0) - geo.loc.y;
        let loc: Point<i32, Physical> = (x, y).into();
        let els: Vec<WaylandSurfaceRenderElement<R>> = w.render_elements(r, loc, Scale::from(1.0), 1.0);
        out.extend(els.into_iter().map(ShellElement::Surface));
    }
    out
}

pub struct Headless {
    pub renderer: PixmanRenderer,
    image: pixman::Image<'static, 'static>,
    damage: OutputDamageTracker,
    pub frames: u64,
}

impl Headless {
    pub fn new(size: (i32, i32)) -> Result<Headless, String> {
        let mut renderer = PixmanRenderer::new().map_err(|e| format!("pixman: {e:?}"))?;
        let image = renderer.create_buffer(Fourcc::Xrgb8888, size.into()).map_err(|e| format!("pixman: {e:?}"))?;
        Ok(Headless { renderer, image, damage: OutputDamageTracker::new(size, 1.0, Transform::Normal), frames: 0 })
    }

    fn render(&mut self, shell: &mut Shell) {
        let els = elements(shell, &mut self.renderer);
        let r = match self.renderer.bind(&mut self.image) {
            Ok(mut fb) => self.damage.render_output(&mut self.renderer, &mut fb, 1, &els, CLEAR).map(|_| ()).map_err(|e| format!("{e:?}")),
            Err(e) => Err(format!("{e:?}")),
        };
        if let Err(e) = r {
            tracing::warn!("headless render: {e}");
        }
        self.frames += 1;
        shell.needs_redraw = false;
        shell.frame_done();
    }
}

pub enum Backend {
    None,
    Headless(Box<Headless>),
    Drm(Box<DrmBackend>),
}

impl Backend {
    pub fn dmabuf_formats(&self) -> Option<FormatSet> {
        match self {
            Backend::Headless(h) => Some(h.renderer.dmabuf_formats()),
            Backend::Drm(d) => Some(d.renderer.dmabuf_formats()),
            Backend::None => None,
        }
    }

    pub fn import_dmabuf(&mut self, buf: &Dmabuf) -> bool {
        let r = match self {
            Backend::Headless(h) => &mut h.renderer,
            Backend::Drm(d) => &mut d.renderer,
            Backend::None => return false,
        };
        r.import_dmabuf(buf, None).is_ok()
    }

    /// Frames where a client buffer was scanned out directly (DRM only).
    pub fn scanout_frames(&self) -> u64 {
        match self {
            Backend::Drm(d) => d.scanout_frames,
            _ => 0,
        }
    }

    pub fn frames(&self) -> u64 {
        match self {
            Backend::Headless(h) => h.frames,
            Backend::Drm(d) => d.frames,
            Backend::None => 0,
        }
    }
}

/// Render a frame if the backend can take one now.
pub fn render(shell: &mut Shell) {
    let mut b = std::mem::replace(&mut shell.backend, Backend::None);
    match &mut b {
        Backend::Headless(h) => h.render(shell),
        Backend::Drm(d) => d.render(shell),
        Backend::None => {}
    }
    shell.backend = b;
}

/// Compose the current screen offscreen (independent of the backend) and
/// write it as a PNG.
pub fn screenshot(shell: &mut Shell, path: &Path) -> Result<(), String> {
    let (w, h) = shell.size;
    let mut renderer = PixmanRenderer::new().map_err(|e| format!("pixman: {e:?}"))?;
    let mut image: pixman::Image<'static, 'static> =
        renderer.create_buffer(Fourcc::Xrgb8888, (w, h).into()).map_err(|e| format!("{e:?}"))?;
    let els = elements(shell, &mut renderer);
    {
        let mut fb = renderer.bind(&mut image).map_err(|e| format!("{e:?}"))?;
        let mut dt = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
        dt.render_output(&mut renderer, &mut fb, 0, &els, CLEAR).map_err(|e| format!("{e:?}"))?;
    }
    let stride = image.stride() / 4;
    // SAFETY: the image owns w*h pixels of `stride`; nothing else uses it now
    let px = unsafe { std::slice::from_raw_parts(image.data() as *const u32, stride * h as usize) };
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let p = px[y * stride + x];
            rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut wr| wr.write_image_data(&rgb)).map_err(|e| e.to_string())?;
    Ok(())
}
