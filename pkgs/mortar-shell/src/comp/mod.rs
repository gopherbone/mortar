// SPDX-License-Identifier: GPL-2.0-or-later
//! The compositor (Linux only).

pub mod backend;
pub mod bg;
pub mod ctl;
pub mod drm;
pub mod gl;
pub mod gpu;
pub mod input;
pub mod osk_slide;
pub mod pads;
pub mod scanout;
pub mod state;
pub mod suggest;
pub mod ui;

use backend::{Backend, Headless};
use input::{DirectOpener, Inputs};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as CMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;
use state::{ClientState, Opts, Shell};
use std::sync::Arc;
use std::time::Duration;

pub fn run(opts: Opts) -> Result<(), String> {
    if let Some(t) = opts.config.theme.as_deref().and_then(mortar_ui::theme::Theme::by_name) {
        mortar_ui::theme::set(t);
    }
    let mut event_loop: EventLoop<'static, Shell> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let handle = event_loop.handle();
    let display: Display<Shell> = Display::new().map_err(|e| e.to_string())?;
    let dh = display.handle();

    let socket = match &opts.socket {
        Some(n) => ListeningSocketSource::with_name(n),
        None => ListeningSocketSource::new_auto(),
    }
    .map_err(|e| format!("wayland socket: {e}"))?;
    let socket_name = socket.socket_name().to_string_lossy().into_owned();
    handle
        .insert_source(socket, |stream, _, shell: &mut Shell| {
            if let Err(e) = shell.dh.insert_client(stream, Arc::new(ClientState::default())) {
                tracing::warn!("client: {e}");
            }
        })
        .map_err(|e| e.to_string())?;
    handle
        .insert_source(Generic::new(display, Interest::READ, CMode::Level), |_, display, shell: &mut Shell| {
            // SAFETY: the display is not dropped while dispatching
            unsafe {
                display.get_mut().dispatch_clients(shell).ok();
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.to_string())?;

    let (backend, output, size, inputs) = match opts.backend.as_str() {
        "headless" => {
            let output = Output::new(
                "headless".into(),
                PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Mortar".into(), model: "headless".into() },
            );
            let mode = Mode { size: opts.size.into(), refresh: 60_000 };
            output.change_current_state(Some(mode), Some(smithay::utils::Transform::Normal), None, Some((0, 0).into()));
            output.set_preferred(mode);
            let h = Headless::new(opts.size)?;
            (Backend::Headless(Box::new(h)), output, opts.size, Inputs::new(Box::new(DirectOpener), opts.direct_input))
        }
        "drm" | "auto" => {
            let (d, output, size) = drm::init(&handle, &opts.config.ui)?;
            let opener = drm::SessionOpener(d.session.clone());
            (Backend::Drm(Box::new(d)), output, size, Inputs::new(Box::new(opener), true))
        }
        other => return Err(format!("unknown backend '{other}' (drm, headless)")),
    };

    let mut shell = Shell::new(dh, handle.clone(), event_loop.get_signal(), socket_name.clone(), output, size, backend, opts.config, inputs)?;
    // SAFETY: single-threaded; children inherit it
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name) };
    tracing::info!(socket = %socket_name, size = ?size, "mortar-shell running");

    let ctl = opts.ctl.clone().unwrap_or_else(ctl::default_path);
    ctl::listen(&handle, &ctl)?;
    let osk_sock = suggest::default_path();
    if let Err(e) = suggest::listen(&handle, &osk_sock) {
        tracing::warn!("osk suggestions: {e}");
    }
    input::scan(&mut shell);
    handle
        .insert_source(smithay::reexports::calloop::timer::Timer::from_duration(Duration::from_secs(2)), |_, _, shell: &mut Shell| {
            input::scan(shell);
            smithay::reexports::calloop::timer::TimeoutAction::ToDuration(Duration::from_secs(2))
        })
        .map_err(|e| e.to_string())?;

    // autostart: launch in reverse so the first ends up visible
    let auto: Vec<String> = shell.cfg.autostart.clone();
    for id in auto.iter().rev() {
        if let Err(e) = shell.launch(id) {
            tracing::warn!("autostart {id}: {e}");
        }
    }
    if let Some(first) = auto.first().and_then(|id| shell.apps.by_entry(id)).map(|a| a.uid) {
        shell.pending_focus = Some(first);
    }

    while shell.running {
        let t = shell.timeout();
        event_loop.dispatch(Some(t), &mut shell).map_err(|e| e.to_string())?;
        shell.tick();
        let _ = shell.dh.flush_clients();
    }
    tracing::info!("exiting; closing apps");
    if let backend::Backend::Drm(d) = &mut shell.backend {
        d.shutdown();
    }
    shell.apps.terminate_all();
    let _ = std::fs::remove_file(&ctl);
    let _ = std::fs::remove_file(&osk_sock);
    Ok(())
}
