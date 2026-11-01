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
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{render_elements, AsRenderElements, Kind};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, Color32F, ImportAll, ImportDma, ImportMem, Offscreen, Renderer};
use smithay::reexports::pixman;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::utils::{CommitCounter, RendererSurfaceStateUserData};
use smithay::desktop::PopupManager;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Transform};
use smithay::wayland::compositor::{get_children, with_states};
use smithay::wayland::shm::with_buffer_contents;
use std::path::Path;

pub use super::drm::DrmBackend;

/// Behind every client (letterboxing).
pub const CLEAR: Color32F = Color32F::new(0.0, 0.0, 0.0, 1.0);

render_elements! {
    pub ShellElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Memory=MemoryRenderBufferRenderElement<R>,
    Solid=SolidColorRenderElement,
}

/// The visible app's buffer, copied into memory the renderer treats as
/// opaque (XRGB).
///
/// SDL apps on Mesa llvmpipe commit ARGB8888 wl_shm buffers whose alpha
/// is junk (often 0) while declaring the surface opaque. Composited as
/// ARGB that is wrong twice: the damage tracker skips clearing under the
/// "opaque" region, and pixman blends (OVER), so stale swapchain contents
/// show through (garbled app over the old home screen, different on each
/// buffer: flashing), and blending reads back the write-combined scanout
/// buffer (slow). A fullscreen app has nothing under it, so its pixels are
/// copied as XRGB and drawn with SRC: only damaged rectangles, and the
/// copy reads cached memory and only writes the scanout buffer.
pub struct OpaqueCopy {
    buf: MemoryRenderBuffer,
    size: (i32, i32),
    commit: Option<CommitCounter>,
    /// The colour of its bottom-left pixel (a terminal's background), for
    /// the strip below it while it is shorter than its area.
    edge: [f32; 4],
    fill: SolidColorBuffer,
}

fn opaque_copy<R>(shell: &mut Shell, r: &mut R, surface: &WlSurface, loc: Point<i32, Physical>) -> Option<MemoryRenderBufferRenderElement<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    if !get_children(surface).is_empty() {
        return None; // subsurfaces: let the generic path stack them
    }
    let id = surface.id();
    let last = shell.opaque.get(&id).and_then(|o| o.commit);
    let (buffer, commit, damage, view) = with_states(surface, |st| {
        let d = st.data_map.get::<RendererSurfaceStateUserData>()?;
        let d = d.lock().unwrap();
        let b: WlBuffer = (**d.buffer()?).clone();
        Some((b, d.current_commit(), d.damage_since(last).to_vec(), d.view()?))
    })?;
    if last == Some(commit) && shell.opaque.contains_key(&id) {
        let o = &shell.opaque[&id];
        return MemoryRenderBufferRenderElement::from_buffer(r, loc.to_f64(), &o.buf, None, Some(view.src), Some(view.dst), Kind::Unspecified).ok();
    }
    let entry = shell.opaque.entry(id).or_insert_with(|| OpaqueCopy {
        buf: xrgb_buffer(1, 1),
        size: (1, 1),
        commit: None,
        edge: [0.0, 0.0, 0.0, 1.0],
        fill: SolidColorBuffer::new((1, 1), [0.0, 0.0, 0.0, 1.0]),
    });
    let ok = with_buffer_contents(&buffer, |ptr, len, data| {
        if !matches!(data.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
            return false;
        }
        let (w, h, stride, off) = (data.width, data.height, data.stride as usize, data.offset as usize);
        if off + stride * h as usize > len || w <= 0 || h <= 0 {
            return false;
        }
        let full = entry.size != (w, h) || entry.commit.is_none();
        if entry.size != (w, h) {
            entry.buf = xrgb_buffer(w, h);
            entry.size = (w, h);
        }
        let full_rect = Rectangle::<i32, BufferCoords>::from_size((w, h).into());
        let rects: Vec<Rectangle<i32, BufferCoords>> = if full || damage.is_empty() {
            vec![full_rect]
        } else {
            damage.iter().filter_map(|d| d.intersection(full_rect)).collect()
        };
        // SAFETY: bounds checked above; the client may scribble concurrently
        // (its problem, as with any shm compositor)
        let src = unsafe { std::slice::from_raw_parts(ptr.add(off), stride * h as usize) };
        // XRGB little-endian: B, G, R, X
        let px = &src[(h as usize - 1) * stride..(h as usize - 1) * stride + 4];
        entry.edge = [px[2] as f32 / 255.0, px[1] as f32 / 255.0, px[0] as f32 / 255.0, 1.0];
        let mut ctx = entry.buf.render();
        let _ = ctx.draw(|mem: &mut [u8]| {
            let dst_stride = w as usize * 4;
            for rc in &rects {
                let (x, y, rw, rh) = (rc.loc.x as usize, rc.loc.y as usize, rc.size.w as usize, rc.size.h as usize);
                for row in y..y + rh {
                    let s = row * stride + x * 4;
                    let d = row * dst_stride + x * 4;
                    mem[d..d + rw * 4].copy_from_slice(&src[s..s + rw * 4]);
                }
            }
            Ok::<_, ()>(rects.clone())
        });
        true
    })
    .unwrap_or(false);
    if !ok {
        shell.opaque.remove(&surface.id());
        return None;
    }
    entry.commit = Some(commit);
    MemoryRenderBufferRenderElement::from_buffer(r, loc.to_f64(), &entry.buf, None, Some(view.src), Some(view.dst), Kind::Unspecified).ok()
}

fn xrgb_buffer(w: i32, h: i32) -> MemoryRenderBuffer {
    let full = Rectangle::<i32, BufferCoords>::from_size((w, h).into());
    MemoryRenderBuffer::new(Fourcc::Xrgb8888, (w, h), 1, Transform::Normal, Some(vec![full]))
}

/// What is on screen, front to back.
pub fn elements<R>(shell: &mut Shell, r: &mut R) -> Vec<ShellElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut out: Vec<ShellElement<R>> = Vec::new();
    let mem = |r: &mut R, loc: (f64, f64), buf| {
        MemoryRenderBufferRenderElement::from_buffer(r, loc, buf, None, None, None, Kind::Unspecified).ok()
    };
    if shell.ui_layer() {
        // the shell UI, drawn by the CPU renderer (cpu_ui_frame)
        if let Some(e) = mem(r, (0.0, 0.0), &shell.overlay_buf) {
            out.push(ShellElement::Memory(e));
        }
        return out;
    }
    let area = shell.app_area();
    // the keyboard, sliding (osk_slide.rs): its top shows first
    if shell.osk_shown()
        && let Some(e) = mem(r, (0.0, (shell.size.1 - shell.osk_shown_h()) as f64), &shell.osk_buf)
    {
        out.push(ShellElement::Memory(e));
    }
    if let Some(w) = shell.visible_window().cloned() {
        // centre clients smaller than the area; offset client-side shadows
        let geo = w.geometry();
        let x = ((area.w - geo.size.w) / 2).max(0) - geo.loc.x;
        let y = ((area.h - geo.size.h) / 2).max(0) - geo.loc.y;
        let loc: Point<i32, Physical> = (x, y).into();
        let popups = w.toplevel().map(|t| PopupManager::popups_for_surface(t.wl_surface()).next().is_some()).unwrap_or(true);
        let fast = if popups || std::env::var_os("MORTAR_SHELL_NO_OPAQUE_COPY").is_some() {
            None
        } else {
            w.toplevel().map(|t| t.wl_surface().clone()).and_then(|s| opaque_copy(shell, r, &s, loc))
        };
        match fast {
            Some(e) => {
                out.push(ShellElement::Memory(e));
                // Shorter than the screen (the keyboard just closed and the
                // app has not drawn at full height yet, or the keyboard is
                // still sliding over the gap): fill below it with its own
                // edge colour, not black.
                let bottom = (loc.y + geo.loc.y + geo.size.h).max(0);
                if bottom < shell.size.1 && loc.y + geo.loc.y == 0 && shell.keyboard_app()
                    && let Some(o) = w.toplevel().and_then(|t| shell.opaque.get_mut(&t.wl_surface().id()))
                {
                    o.fill.update((shell.size.0, shell.size.1 - bottom), o.edge);
                    out.push(ShellElement::Solid(SolidColorRenderElement::from_buffer(&o.fill, (0, bottom), 1.0, 1.0, Kind::Unspecified)));
                }
            }
            None => {
                let els: Vec<WaylandSurfaceRenderElement<R>> = w.render_elements(r, loc, Scale::from(1.0), 1.0);
                out.extend(els.into_iter().map(ShellElement::Surface));
            }
        }
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
        if shell.ui_layer() {
            shell.cpu_ui_frame();
        }
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
    /// The shell UI renders on the GPU.
    pub fn has_gpu_ui(&self) -> bool {
        matches!(self, Backend::Drm(d) if d.gpu.is_some())
    }

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

    /// Frames the GPU UI renderer drew (DRM only).
    pub fn gpu_frames(&self) -> u64 {
        match self {
            Backend::Drm(d) => d.gpu_frames,
            _ => 0,
        }
    }

    /// GPU UI timings: last and smoothed frame ms, last blur ms, blurs.
    pub fn gpu_stats(&self) -> Option<(f32, f32, f32, u64)> {
        match self {
            Backend::Drm(d) => d.gpu.as_ref().map(|g| (g.render_ms, g.render_avg_ms, g.blur_ms, g.blur_count)),
            _ => None,
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

/// The current screen (UI and apps, without the shutter flash) as a PNG.
pub fn screenshot(shell: &mut Shell, path: &Path) -> Result<(), String> {
    let pm = shell.capture().ok_or("capture failed")?;
    super::ui::save_rgb_png(&pm, path)
}
