// SPDX-License-Identifier: GPL-2.0-or-later
//! Direct scanout of client dma-bufs.
//!
//! Smithay's exporters only turn our own swapchain buffers into KMS
//! framebuffers for client buffers when they come through GBM with a
//! matching import node (and the dumb-buffer exporter never does). GPU apps
//! here (mortar-wlegl on the PowerVR) hand us linear XRGB8888 dma-bufs that
//! are sun4i dumb buffers (CMA, contiguous): PRIME-import them into the
//! card and AddFB2 them, and `DrmCompositor` can put a fullscreen app
//! straight on the primary plane. No copy, no CPU per frame.
//!
//! `ClientScanout` wraps the swapchain's exporter (GBM or dumb) and adds
//! that for `ExportBuffer::Wayland`. Anything it cannot take (shm, other
//! formats, several planes, tiled modifiers, a buffer the display can't
//! reach) is rejected and gets composited as before.

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer, Format, Fourcc, Modifier};
use smithay::backend::drm::exporter::{ExportBuffer, ExportFramebuffer};
use smithay::backend::drm::{DrmDeviceFd, Framebuffer};
use smithay::reexports::drm::buffer::{Handle as BufferHandle, PlanarBuffer};
use smithay::reexports::drm::control::{framebuffer, Device as ControlDevice, FbCmd2Flags};
use smithay::wayland::dmabuf::get_dmabuf;
use std::fmt;

/// Formats a plane on the A133 display engine takes, and what to use when
/// the plane wants it opaque.
fn opaque_of(code: Fourcc) -> Option<Fourcc> {
    Some(match code {
        Fourcc::Xrgb8888 | Fourcc::Argb8888 => Fourcc::Xrgb8888,
        Fourcc::Xbgr8888 | Fourcc::Abgr8888 => Fourcc::Xbgr8888,
        _ => return None,
    })
}

/// A client buffer we can try to put on a plane.
pub fn eligible(format: Format, planes: usize) -> bool {
    planes == 1 && opaque_of(format.code).is_some() && matches!(format.modifier, Modifier::Linear | Modifier::Invalid)
}

pub struct ClientScanout<E> {
    pub inner: E,
    /// Off: behave exactly like `inner` (MORTAR_SHELL_NO_SCANOUT).
    pub enabled: bool,
}

#[derive(Debug)]
pub enum ScanoutFb<F> {
    Swapchain(F),
    Client(ClientFb),
}

impl<F: AsRef<framebuffer::Handle>> AsRef<framebuffer::Handle> for ScanoutFb<F> {
    fn as_ref(&self) -> &framebuffer::Handle {
        match self {
            ScanoutFb::Swapchain(f) => f.as_ref(),
            ScanoutFb::Client(c) => &c.fb,
        }
    }
}

impl<F: Framebuffer> Framebuffer for ScanoutFb<F> {
    fn format(&self) -> Format {
        match self {
            ScanoutFb::Swapchain(f) => f.format(),
            ScanoutFb::Client(c) => c.format,
        }
    }
}

/// A KMS framebuffer on a client's dma-buf. It holds its own reference to
/// the GEM object, so the import handle is closed right away.
#[derive(Debug)]
pub struct ClientFb {
    fb: framebuffer::Handle,
    format: Format,
    drm: DrmDeviceFd,
}

impl Drop for ClientFb {
    fn drop(&mut self) {
        if let Err(e) = self.drm.destroy_framebuffer(self.fb) {
            tracing::warn!(fb = ?self.fb, "destroy client framebuffer: {e}");
        }
    }
}

#[derive(Debug)]
pub enum ScanoutError<E> {
    Swapchain(E),
    Unsupported,
    Import(std::io::Error),
}

impl<E: fmt::Display> fmt::Display for ScanoutError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanoutError::Swapchain(e) => write!(f, "{e}"),
            ScanoutError::Unsupported => write!(f, "client buffer can't be scanned out"),
            ScanoutError::Import(e) => write!(f, "client buffer import: {e}"),
        }
    }
}

impl<E: std::error::Error> std::error::Error for ScanoutError<E> {}

struct Planar {
    size: (u32, u32),
    format: Fourcc,
    pitch: u32,
    offset: u32,
    handle: BufferHandle,
}

impl PlanarBuffer for Planar {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn format(&self) -> Fourcc {
        self.format
    }
    fn modifier(&self) -> Option<Modifier> {
        None // linear, implicit (sun4i takes no modifiers)
    }
    fn pitches(&self) -> [u32; 4] {
        [self.pitch, 0, 0, 0]
    }
    fn handles(&self) -> [Option<BufferHandle>; 4] {
        [Some(self.handle), None, None, None]
    }
    fn offsets(&self) -> [u32; 4] {
        [self.offset, 0, 0, 0]
    }
}

fn client_fb(drm: &DrmDeviceFd, buf: &Dmabuf, use_opaque: bool) -> Result<ClientFb, std::io::Error> {
    let format = buf.format();
    let code = if use_opaque { opaque_of(format.code).unwrap_or(format.code) } else { format.code };
    let fd = buf.handles().next().ok_or_else(|| std::io::Error::other("no plane"))?;
    let handle = drm.prime_fd_to_buffer(fd)?;
    let size = buf.size();
    let planar = Planar {
        size: (size.w as u32, size.h as u32),
        format: code,
        pitch: buf.strides().next().unwrap_or(0),
        offset: buf.offsets().next().unwrap_or(0),
        handle,
    };
    let fb = drm.add_planar_framebuffer(&planar, FbCmd2Flags::empty());
    // the framebuffer (if any) keeps the GEM object alive
    let _ = drm.close_buffer(handle);
    let fb = fb?;
    tracing::debug!(?fb, w = size.w, h = size.h, ?code, "client buffer framebuffer");
    // Added without modifiers (implicit layout, linear on sun4i). Smithay
    // matches this against the plane's formats, which always list
    // (code, Invalid) but list (code, Linear) only with IN_FORMATS.
    Ok(ClientFb { fb, format: Format { code, modifier: Modifier::Invalid }, drm: drm.clone() })
}

impl<E, B> ExportFramebuffer<B> for ClientScanout<E>
where
    B: Buffer,
    E: ExportFramebuffer<B>,
    E::Framebuffer: fmt::Debug,
{
    type Framebuffer = ScanoutFb<E::Framebuffer>;
    type Error = ScanoutError<E::Error>;

    fn add_framebuffer(&self, drm: &DrmDeviceFd, buffer: ExportBuffer<'_, B>, use_opaque: bool) -> Result<Option<Self::Framebuffer>, Self::Error> {
        match buffer {
            ExportBuffer::Wayland(wl) => {
                let buf = get_dmabuf(wl).map_err(|_| ScanoutError::Unsupported)?;
                if !self.enabled || !eligible(buf.format(), buf.num_planes()) {
                    return Err(ScanoutError::Unsupported);
                }
                client_fb(drm, buf, use_opaque).map(|f| Some(ScanoutFb::Client(f))).map_err(ScanoutError::Import)
            }
            other => self
                .inner
                .add_framebuffer(drm, other, use_opaque)
                .map(|f| f.map(ScanoutFb::Swapchain))
                .map_err(ScanoutError::Swapchain),
        }
    }

    fn can_add_framebuffer(&self, buffer: &ExportBuffer<'_, B>) -> bool {
        match buffer {
            ExportBuffer::Wayland(wl) => {
                self.enabled && get_dmabuf(wl).is_ok_and(|b| eligible(b.format(), b.num_planes()))
            }
            other => self.inner.can_add_framebuffer(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility() {
        let f = |code, modifier| Format { code, modifier };
        assert!(eligible(f(Fourcc::Xrgb8888, Modifier::Linear), 1));
        assert!(eligible(f(Fourcc::Argb8888, Modifier::Invalid), 1));
        assert!(!eligible(f(Fourcc::Xrgb8888, Modifier::Linear), 2));
        assert!(!eligible(f(Fourcc::Nv12, Modifier::Linear), 1));
        assert!(!eligible(f(Fourcc::Xrgb8888, Modifier::from(0x0100_0000_0000_0001u64)), 1));
    }
}
