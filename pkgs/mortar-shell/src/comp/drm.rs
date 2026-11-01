// SPDX-License-Identifier: GPL-2.0-or-later
//! The real thing: libseat session, DRM/KMS output, session-opened input.
//!
//! - Session: libseat talks to logind (a greetd session has one) or seatd.
//!   On VT switch / session pause the DRM device and every input fd are
//!   released; on resume they come back and the screen is redrawn.
//! - Card: $MORTAR_DRM_DEVICE or the first /dev/dri/card* with a connected
//!   connector (sun4i-drm's card0 on the Brick; the PowerVR has no KMS).
//! - Buffers: GBM (Mesa kms_swrast = dumb buffers) for the swapchain, so the
//!   GBM framebuffer exporter can also import client dmabufs for direct
//!   scanout.

use super::backend::{elements, CLEAR};
use super::input::Opener;
use super::state::Shell;
use smithay::backend::allocator::dumb::DumbAllocator;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::drm::control::{connector, Device as ControlDevice, ModeTypeFlags};
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

type GbmComp = DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type DumbComp = DrmCompositor<DumbAllocator, DrmDeviceFd, (), DrmDeviceFd>;

/// GBM buffers (default; allows direct scanout of client dmabufs) or plain
/// KMS dumb buffers (fallback when libgbm/Mesa is unusable, or
/// MORTAR_SHELL_ALLOCATOR=dumb).
enum Comp {
    Gbm(Box<GbmComp>),
    Dumb(Box<DumbComp>),
}

macro_rules! with_comp {
    ($comp:expr, $c:ident => $body:expr) => {
        match $comp {
            Comp::Gbm($c) => $body,
            Comp::Dumb($c) => $body,
        }
    };
}

pub struct DrmBackend {
    pub session: LibSeatSession,
    drm: DrmDevice,
    comp: Comp,
    pub renderer: PixmanRenderer,
    /// A frame is queued; wait for its vblank before the next.
    pending: bool,
    active: bool,
    pub frames: u64,
    pub scanout_frames: u64,
}

/// Session-backed input opener (fds are revoked on pause).
pub struct SessionOpener(pub LibSeatSession);
impl Opener for SessionOpener {
    fn open(&mut self, path: &Path) -> io::Result<OwnedFd> {
        self.0
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)
            .map_err(|e| io::Error::other(format!("{e:?}")))
    }
    fn close(&mut self, fd: OwnedFd) {
        let _ = self.0.close(fd);
    }
}

fn cards() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("MORTAR_DRM_DEVICE") {
        return vec![p.into()];
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("card")));
    v.sort();
    v
}

/// Open the session and the panel. Returns the backend, the output and its
/// size in pixels.
pub fn init(handle: &LoopHandle<'static, Shell>) -> Result<(DrmBackend, Output, (i32, i32)), String> {
    let (mut session, notifier) = LibSeatSession::new().map_err(|e| format!("libseat session: {e:?}"))?;
    tracing::info!(seat = %session.seat(), "session");

    let mut chosen = None;
    for path in cards() {
        let fd = match session.open(&path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK) {
            Ok(fd) => fd,
            Err(e) => {
                tracing::warn!(path = %path.display(), "open: {e:?}");
                continue;
            }
        };
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));
        let Ok(res) = fd.resource_handles() else { continue };
        let conn = res.connectors().iter().find_map(|c| {
            let info = fd.get_connector(*c, false).ok()?;
            (info.state() == connector::State::Connected && !info.modes().is_empty()).then_some(info)
        });
        if let Some(info) = conn {
            tracing::info!(path = %path.display(), connector = ?info.interface(), "using card");
            chosen = Some((fd, res, info));
            break;
        }
    }
    let (fd, res, conn) = chosen.ok_or("no DRM card with a connected display")?;

    let mode = conn
        .modes()
        .iter()
        .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .copied()
        .unwrap_or(conn.modes()[0]);
    let crtc = conn
        .encoders()
        .iter()
        .filter_map(|e| fd.get_encoder(*e).ok())
        .flat_map(|enc| res.filter_crtcs(enc.possible_crtcs()))
        .next()
        .ok_or("no CRTC for the connector")?;

    let (mut drm, drm_notifier) = DrmDevice::new(fd.clone(), true).map_err(|e| format!("drm: {e:?}"))?;
    let surface = drm.create_surface(crtc, mode, &[conn.handle()]).map_err(|e| format!("drm surface: {e:?}"))?;

    let (w, h) = (mode.size().0 as i32, mode.size().1 as i32);
    let (mm_w, mm_h) = conn.size().unwrap_or((0, 0));
    let output = Output::new(
        "panel".into(),
        PhysicalProperties { size: (mm_w as i32, mm_h as i32).into(), subpixel: Subpixel::Unknown, make: "TrimUI".into(), model: "Brick".into() },
    );
    let omode = Mode::from(mode);
    output.change_current_state(Some(omode), Some(smithay::utils::Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(omode);

    let renderer = PixmanRenderer::new().map_err(|e| format!("pixman: {e:?}"))?;
    let formats = [Fourcc::Xrgb8888, Fourcc::Argb8888];
    let want_dumb = std::env::var("MORTAR_SHELL_ALLOCATOR").is_ok_and(|v| v == "dumb");
    let gbm = if want_dumb {
        None
    } else {
        GbmDevice::new(fd.clone()).map_err(|e| tracing::warn!("gbm: {e}; using dumb buffers")).ok()
    };
    let comp = match gbm {
        Some(gbm) => {
            let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
            let exporter = GbmFramebufferExporter::new(gbm.clone(), None);
            DrmCompositor::new(&output, surface, None, allocator, exporter, formats, renderer.dmabuf_formats(), drm.cursor_size(), Some(gbm))
                .map(|c| Comp::Gbm(Box::new(c)))
                .map_err(|e| format!("drm compositor (gbm): {e:?}"))?
        }
        None => {
            let allocator = DumbAllocator::new(fd.clone());
            DrmCompositor::new(
                &output,
                surface,
                None,
                allocator,
                fd.clone(),
                formats,
                renderer.dmabuf_formats(),
                drm.cursor_size(),
                None::<GbmDevice<DrmDeviceFd>>,
            )
            .map(|c| Comp::Dumb(Box::new(c)))
            .map_err(|e| format!("drm compositor (dumb): {e:?}"))?
        }
    };
    tracing::info!(allocator = if matches!(comp, Comp::Gbm(_)) { "gbm" } else { "dumb" }, "scanout buffers");

    handle
        .insert_source(drm_notifier, |ev, _, shell: &mut Shell| match ev {
            DrmEvent::VBlank(_) => on_vblank(shell),
            DrmEvent::Error(e) => tracing::warn!("drm: {e:?}"),
        })
        .map_err(|e| e.to_string())?;
    handle
        .insert_source(notifier, |ev, _, shell: &mut Shell| match ev {
            SessionEvent::PauseSession => {
                tracing::info!("session paused");
                super::input::close_all(shell);
                shell.inputs.enabled = false;
                if let super::backend::Backend::Drm(d) = &mut shell.backend {
                    d.drm.pause();
                    d.active = false;
                    d.pending = false;
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session active");
                if let super::backend::Backend::Drm(d) = &mut shell.backend {
                    if let Err(e) = d.drm.activate(false) {
                        tracing::warn!("drm activate: {e:?}");
                    }
                    let _ = with_comp!(&mut d.comp, c => c.reset_state());
                    d.active = true;
                }
                shell.inputs.enabled = true;
                super::input::scan(shell);
                shell.needs_redraw = true;
                shell.ui_dirty = true;
            }
        })
        .map_err(|e| e.to_string())?;

    Ok((
        DrmBackend { session, drm, comp, renderer, pending: false, active: true, frames: 0, scanout_frames: 0 },
        output,
        (w, h),
    ))
}

fn on_vblank(shell: &mut Shell) {
    if let super::backend::Backend::Drm(d) = &mut shell.backend {
        with_comp!(&mut d.comp, c => {
            if let Err(e) = c.frame_submitted() {
                tracing::warn!("frame_submitted: {e:?}");
            }
        });
        d.pending = false;
    }
    shell.frame_done();
    if shell.needs_redraw {
        super::backend::render(shell);
    }
}

impl DrmBackend {
    pub fn render(&mut self, shell: &mut Shell) {
        if !self.active || self.pending {
            return; // stays dirty; rendered at the next vblank / resume
        }
        let els = elements(shell, &mut self.renderer);
        let flags = if std::env::var_os("MORTAR_SHELL_NO_SCANOUT").is_some() { FrameFlags::empty() } else { FrameFlags::DEFAULT };
        let renderer = &mut self.renderer;
        let r: Result<(bool, bool), String> = with_comp!(&mut self.comp, c => {
            c.render_frame(renderer, &els, CLEAR, flags)
                .map(|res| {
                    let direct = matches!(res.primary_element, smithay::backend::drm::compositor::PrimaryPlaneElement::Element(_));
                    (res.is_empty, direct)
                })
                .map_err(|e| format!("{e:?}"))
        });
        match r {
            Ok((empty, direct)) => {
                shell.needs_redraw = false;
                if direct {
                    self.scanout_frames += 1; // a client buffer went straight to the panel
                }
                if empty {
                    shell.frame_done();
                    return;
                }
                let q = with_comp!(&mut self.comp, c => c.queue_frame(()).map_err(|e| format!("{e:?}")));
                match q {
                    Ok(()) => {
                        self.pending = true;
                        self.frames += 1;
                    }
                    Err(e) => {
                        tracing::warn!("queue_frame: {e}");
                        shell.frame_done();
                    }
                }
            }
            Err(e) => tracing::warn!("render_frame: {e}"),
        }
    }
}
