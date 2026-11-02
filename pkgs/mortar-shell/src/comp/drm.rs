// SPDX-License-Identifier: GPL-2.0-or-later
//! The real thing: libseat session, DRM/KMS output, session-opened input.
//!
//! - Session: libseat talks to logind (a greetd session has one) or seatd.
//!   On VT switch / session pause the DRM device and every input fd are
//!   released; on resume they come back and the screen is redrawn.
//! - Card: $MORTAR_DRM_DEVICE or the first /dev/dri/card* with a connected
//!   connector (sun4i-drm's card0 on the Brick; the PowerVR has no KMS).
//! - Buffers: plain KMS dumb buffers for the swapchain (GBM, i.e. Mesa's
//!   kms_swrast handing out the same dumb buffers, when asked or as a
//!   fallback). Client dmabufs (GPU apps through mortar-wlegl) are
//!   PRIME-imported for direct scanout by `scanout::ClientScanout` with
//!   either allocator.
//! - The shell's UI layers: on the GPU (`gpu::GpuUi`, page-flipped on the
//!   compositor's own surface) when `ui.renderer` allows and the PowerVR
//!   initialises; else drawn on the CPU and composited by pixman. Any GPU
//!   error drops the GPU path for the session (orderly teardown) and the
//!   CPU renderer takes over.

use super::backend::{elements, CLEAR};
use super::gpu::GpuUi;
use super::input::Opener;
use super::scanout::ClientScanout;
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

type GbmComp = DrmCompositor<GbmAllocator<DrmDeviceFd>, ClientScanout<GbmFramebufferExporter<DrmDeviceFd>>, (), DrmDeviceFd>;
type DumbComp = DrmCompositor<DumbAllocator, ClientScanout<DrmDeviceFd>, (), DrmDeviceFd>;

/// Plain KMS dumb buffers (default) or GBM ones (MORTAR_SHELL_ALLOCATOR=gbm,
/// or when the dumb compositor does not start). Both scan out client
/// dmabufs directly.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Comp,
    Gpu,
}

pub struct DrmBackend {
    pub session: LibSeatSession,
    drm: DrmDevice,
    comp: Comp,
    pub renderer: PixmanRenderer,
    /// The GPU UI renderer (None: off, unavailable or failed).
    pub gpu: Option<GpuUi>,
    /// The last frame on screen came from the GPU UI.
    gpu_shown: bool,
    gpu_errors: u32,
    pub gpu_frames: u64,
    /// A frame is queued; wait for its vblank before the next.
    pending: Option<Pending>,
    active: bool,
    pub frames: u64,
    pub scanout_frames: u64,
    /// The last frame put a client buffer on the primary plane.
    direct: bool,
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
pub fn init(handle: &LoopHandle<'static, Shell>, ui: &crate::config::UiConfig) -> Result<(DrmBackend, Output, (i32, i32)), String> {
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
    // Plain KMS dumb buffers by default. GBM here is Mesa's kms_swrast,
    // which hands out the same dumb buffers but first loads libgallium,
    // LLVM and the Vulkan drivers (2.4 s at boot from the card, ~40 MB
    // resident). GBM when asked (MORTAR_SHELL_ALLOCATOR=gbm) or when the
    // dumb compositor does not start.
    let want_gbm = std::env::var("MORTAR_SHELL_ALLOCATOR").is_ok_and(|v| v == "gbm");
    let scanout = std::env::var_os("MORTAR_SHELL_NO_SCANOUT").is_none();
    let mut surface = Some(surface);
    let mut dumb = None;
    if !want_gbm && let Some(s) = surface.take() {
        let allocator = DumbAllocator::new(fd.clone());
        match DrmCompositor::new(
            &output,
            s,
            None,
            allocator,
            ClientScanout { inner: fd.clone(), enabled: scanout },
            formats,
            renderer.dmabuf_formats(),
            drm.cursor_size(),
            None::<GbmDevice<DrmDeviceFd>>,
        ) {
            Ok(c) => dumb = Some(Comp::Dumb(Box::new(c))),
            Err(e) => tracing::warn!("drm compositor (dumb): {e:?}; trying GBM"),
        }
    }
    let comp = match dumb {
        Some(c) => c,
        None => {
            let s = match surface.take() {
                Some(s) => s,
                None => drm.create_surface(crtc, mode, &[conn.handle()]).map_err(|e| format!("drm surface: {e:?}"))?,
            };
            let gbm = GbmDevice::new(fd.clone()).map_err(|e| format!("gbm: {e}"))?;
            let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
            let exporter = ClientScanout { inner: GbmFramebufferExporter::new(gbm.clone(), None), enabled: scanout };
            DrmCompositor::new(&output, s, None, allocator, exporter, formats, renderer.dmabuf_formats(), drm.cursor_size(), Some(gbm))
                .map(|c| Comp::Gbm(Box::new(c)))
                .map_err(|e| format!("drm compositor (gbm): {e:?}"))?
        }
    };
    tracing::info!(allocator = if matches!(comp, Comp::Gbm(_)) { "gbm" } else { "dumb" }, "scanout buffers");

    let renderer_choice = match std::env::var("MORTAR_SHELL_RENDERER").as_deref() {
        Ok("gpu") => crate::config::Renderer::Gpu,
        Ok("cpu") => crate::config::Renderer::Cpu,
        _ => ui.renderer,
    };
    let msaa = std::env::var("MORTAR_SHELL_GL_MSAA").ok().and_then(|v| v.parse().ok()).unwrap_or(ui.msaa);
    let gpu = match (renderer_choice, &ui.gl_lib_dir) {
        (crate::config::Renderer::Cpu, _) => {
            tracing::info!("UI renderer: CPU (config)");
            None
        }
        (_, None) => {
            tracing::info!("UI renderer: CPU (no ui.glLibDir)");
            None
        }
        (_, Some(dir)) => match GpuUi::new(&fd, (w, h), dir, msaa) {
            Ok(g) => {
                tracing::info!("UI renderer: GPU");
                Some(g)
            }
            Err(e) => {
                tracing::warn!("UI renderer: CPU (GPU unavailable: {e})");
                None
            }
        },
    };

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
                    d.pending = None;
                    if let Some(g) = &mut d.gpu {
                        g.released();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session active");
                if let super::backend::Backend::Drm(d) = &mut shell.backend {
                    if let Err(e) = d.drm.activate(false) {
                        tracing::warn!("drm activate: {e:?}");
                    }
                    let _ = with_comp!(&mut d.comp, c => c.reset_state());
                    d.gpu_shown = false;
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
        DrmBackend {
            session,
            drm,
            comp,
            renderer,
            gpu,
            gpu_shown: false,
            gpu_errors: 0,
            gpu_frames: 0,
            pending: None,
            active: true,
            frames: 0,
            scanout_frames: 0,
            direct: false,
        },
        output,
        (w, h),
    ))
}

fn on_vblank(shell: &mut Shell) {
    if let super::backend::Backend::Drm(d) = &mut shell.backend {
        match d.pending.take() {
            Some(Pending::Comp) => {
                with_comp!(&mut d.comp, c => {
                    if let Err(e) = c.frame_submitted() {
                        tracing::warn!("frame_submitted: {e:?}");
                    }
                });
                // our UI buffers are off screen now
                if !d.gpu_shown && let Some(g) = &mut d.gpu {
                    g.released();
                }
            }
            Some(Pending::Gpu) => {
                if let Some(g) = &mut d.gpu {
                    g.flipped();
                }
            }
            None => {}
        }
    }
    shell.frame_done();
    if shell.needs_redraw {
        super::backend::render(shell);
    }
}

impl DrmBackend {
    /// Free the GPU path in order (exit, or after an error).
    pub fn drop_gpu(&mut self, why: &str) {
        if let Some(mut g) = self.gpu.take() {
            tracing::warn!("GPU UI off: {why}; drawing the UI on the CPU");
            g.destroy();
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(mut g) = self.gpu.take() {
            g.destroy();
        }
    }

    /// Draw the UI on the GPU and flip to it. False: not done (use the CPU).
    fn render_gpu(&mut self, shell: &mut Shell) -> bool {
        let Some(g) = self.gpu.as_mut() else { return false };
        let scene = shell.build_scene();
        let fb = match g.render(shell, &scene) {
            Ok(fb) => fb,
            Err(e) => {
                self.drop_gpu(&e);
                shell.gpu_ui = false;
                return false;
            }
        };
        let (w, h) = shell.size;
        let surface = match &self.comp {
            Comp::Gbm(c) => c.surface(),
            Comp::Dumb(c) => c.surface(),
        };
        use smithay::backend::drm::{PlaneConfig, PlaneState};
        use smithay::utils::{Rectangle, Transform};
        let mut planes = vec![PlaneState {
            handle: surface.plane(),
            config: Some(PlaneConfig {
                src: Rectangle::from_size((w as f64, h as f64).into()),
                dst: Rectangle::from_size((w, h).into()),
                transform: Transform::Normal,
                alpha: 1.0,
                damage_clips: None,
                fb,
                fence: None,
            }),
        }];
        if !self.gpu_shown {
            // the compositor may have used other planes: clear them
            let p = surface.planes();
            for pl in p.overlay.iter().chain(p.cursor.iter()) {
                planes.push(PlaneState { handle: pl.handle, config: None });
            }
        }
        // the first frame after start (boot screen) sets the mode: the CRTC
        // was switched off when the device was opened
        let r = if surface.commit_pending() { surface.commit(planes, true) } else { surface.page_flip(planes, true) };
        match r {
            Ok(()) => {
                self.pending = Some(Pending::Gpu);
                self.gpu_shown = true;
                self.gpu_errors = 0;
                self.gpu_frames += 1;
                self.frames += 1;
                shell.needs_redraw = false;
                true
            }
            Err(e) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.unqueue();
                }
                self.gpu_errors += 1;
                tracing::warn!("UI page flip: {e:?}");
                if self.gpu_errors >= 3 {
                    self.drop_gpu("page flips keep failing");
                    shell.gpu_ui = false;
                }
                false
            }
        }
    }

    pub fn render(&mut self, shell: &mut Shell) {
        if !self.active || self.pending.is_some() {
            return; // stays dirty; rendered at the next vblank / resume
        }
        if shell.ui_layer() {
            if self.gpu.is_some() && self.render_gpu(shell) {
                return;
            }
            shell.cpu_ui_frame();
        }
        if self.gpu_shown {
            // back from the GPU UI: the compositor redraws all and recommits
            let _ = with_comp!(&mut self.comp, c => c.reset_state());
            self.gpu_shown = false;
        }
        let els = elements(shell, &mut self.renderer);
        // _ANY: a client buffer may go on the primary plane even if its
        // format/modifier differs from the swapchain's (dumb slots say
        // Linear, GBM ones Invalid, client framebuffers Invalid); the
        // atomic test commit decides.
        let flags = if std::env::var_os("MORTAR_SHELL_NO_SCANOUT").is_some() {
            FrameFlags::empty()
        } else {
            FrameFlags::DEFAULT | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
        };
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
                if direct != self.direct && !empty {
                    tracing::info!("{}", if direct { "direct scanout: client buffer on the primary plane" } else { "composited" });
                    self.direct = direct;
                }
                if empty {
                    shell.frame_done();
                    return;
                }
                let q = with_comp!(&mut self.comp, c => c.queue_frame(()).map_err(|e| format!("{e:?}")));
                match q {
                    Ok(()) => {
                        self.pending = Some(Pending::Comp);
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
