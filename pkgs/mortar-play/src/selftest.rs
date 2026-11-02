// SPDX-License-Identifier: GPL-2.0-or-later
//! Headless core test: run a game for N frames with scripted buttons, save
//! screenshots (full 1024x768 composition and native), capture the
//! resampled audio, and check save states (round trip determinism, slot
//! files + thumbnail) and SRAM. Prints a JSON report; used by the Nix
//! integration test and for measuring per-core speed on the device.

use crate::audio::{self, CaptureAudio, Mixer};
use crate::config::Config;
use crate::input::{Button, Held};
use crate::session::{Session, Slot};
use crate::video;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct Opts {
    pub frames: u64,
    pub shots: Vec<u64>,
    /// (start frame, button, frames held)
    pub press: Vec<(u64, String, u64)>,
    pub out: PathBuf,
    pub size: (u32, u32),
    /// Start from the game's resume state (states/<system>/<game>.state.auto).
    pub resume: bool,
    /// Run at the game's frame rate (sleep to each frame's deadline) instead
    /// of flat out, so threads and clocks behave as in play.
    pub paced: bool,
}

#[derive(Serialize, Default)]
pub struct ThreadCpu {
    pub name: String,
    /// CPU time per frame, ms (utime + stime; 10 ms tick resolution).
    pub ms_per_frame: f64,
    /// Share of one CPU core over the run's wall time.
    pub load: f64,
}

#[derive(Serialize, Default)]
pub struct Report {
    pub ok: bool,
    pub core: String,
    pub version: String,
    pub width: u32,
    pub height: u32,
    pub aspect: f32,
    pub fps: f64,
    pub core_rate: f64,
    pub frames: u64,
    /// Average wall time of retro_run, ms (no presentation): the core's cost.
    pub ms_per_frame: f64,
    /// Share of one CPU core at the game's frame rate.
    pub core_load: f64,
    /// Average cost of the CPU presenter's work per frame (convert done in
    /// the callback; this is the scale into a screen-sized buffer, as for
    /// wl_shm), ms.
    pub scale_ms: f64,
    /// Whole-process CPU time per frame (all threads: the core's GPU/SPU
    /// threads too), ms.
    pub cpu_ms_per_frame: f64,
    /// Per-thread CPU over the loop.
    pub threads: Vec<ThreadCpu>,
    /// Paced runs: frames whose work (run + scale) missed the frame's
    /// deadline, and the wall time of the loop.
    pub late_frames: u64,
    pub wall_s: f64,
    pub shots: Vec<String>,
    pub distinct_colors: usize,
    pub audio_rms: f64,
    pub audio_hz: f64,
    pub audio_seconds: f64,
    pub state_bytes: usize,
    pub state_deterministic: Option<bool>,
    pub state_slot_saved: bool,
    pub state_thumb_saved: bool,
    pub sram_bytes: usize,
    pub sram_saved: bool,
    pub errors: Vec<String>,
}

pub fn run(cfg: &Config, sys_id: &str, rom: Option<&Path>, o: &Opts) -> Result<Report, String> {
    let sys = cfg.system(sys_id).ok_or_else(|| format!("unknown system '{sys_id}'"))?;
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let mut s = Session::start(cfg, sys, rom)?;
    let mut r = Report { core: s.core.info.name.clone(), version: s.core.info.version.clone(), fps: s.fps(), core_rate: s.sample_rate(), ..Default::default() };
    let cap = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut mixer = Mixer::new(Box::new(CaptureAudio { rate: 44100, samples: cap.clone() }), cfg.audio_latency_ms);
    mixer.set_core_rate(s.sample_rate());
    let mut held = Held::default();
    let presses: Vec<(u64, Button, u64)> = o.press.iter().filter_map(|(f, b, n)| Button::parse(b).map(|b| (*f, b, *n))).collect();
    let mut run_time = 0.0;
    let mut scale_time = 0.0;
    let mut scaler = video::Scaler::default();
    let mut screen = vec![0u32; (o.size.0 * o.size.1) as usize];
    let mut samples: Vec<i16> = Vec::new();
    if o.resume {
        s.load_state(Slot::Auto).map_err(|e| format!("resume: {e}"))?;
    }
    let ticks0 = task_ticks();
    let cpu0 = process_cpu();
    let start = Instant::now();
    let period = std::time::Duration::from_secs_f64(1.0 / s.fps().max(1.0));
    let mut deadline = start + period;
    for frame in 1..=o.frames {
        for &(f, b, n) in &presses {
            if frame == f {
                held.down(b);
            }
            if frame == f + n {
                held.up(b);
            }
        }
        let t = Instant::now();
        if s.run(&held, false, Some(50)) {
            mixer.set_core_rate(s.sample_rate());
        }
        run_time += t.elapsed().as_secs_f64();
        let t = Instant::now();
        {
            let f = s.display_frame();
            let dst = video::layout(sys.scaling, f.w, f.h, s.aspect(), o.size.0, o.size.1);
            scaler.scale(&f, dst, &mut screen, o.size.0 as usize);
        }
        scale_time += t.elapsed().as_secs_f64();
        if o.paced {
            let now = Instant::now();
            if now > deadline {
                r.late_frames += 1;
                deadline = now;
            } else {
                std::thread::sleep(deadline - now);
            }
            deadline += period;
        }
        samples.extend_from_slice(s.audio());
        mixer.push(s.audio());
        if o.shots.contains(&frame) {
            let f = s.display_frame().into_owned();
            let full = video::compose(&f, sys.scaling, s.aspect(), o.size.0, o.size.1);
            let p = o.out.join(format!("frame-{frame:05}.png"));
            full.save_png(&p)?;
            f.save_png(&o.out.join(format!("frame-{frame:05}-native.png")))?;
            r.shots.push(p.to_string_lossy().into_owned());
        }
    }
    r.wall_s = start.elapsed().as_secs_f64();
    r.cpu_ms_per_frame = (process_cpu() - cpu0) * 1000.0 / o.frames.max(1) as f64;
    let ticks1 = task_ticks();
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    for (tid, (name, t1)) in &ticks1 {
        let t0 = ticks0.get(tid).map_or(0, |v| v.1);
        let secs = (t1 - t0) as f64 / hz;
        if secs > 0.0 {
            r.threads.push(ThreadCpu { name: name.clone(), ms_per_frame: secs * 1000.0 / o.frames.max(1) as f64, load: secs / r.wall_s.max(1e-9) });
        }
    }
    r.threads.sort_by(|a, b| b.ms_per_frame.total_cmp(&a.ms_per_frame));
    r.frames = o.frames;
    r.ms_per_frame = run_time * 1000.0 / o.frames.max(1) as f64;
    r.core_load = r.ms_per_frame * s.fps() / 1000.0;
    r.scale_ms = scale_time * 1000.0 / o.frames.max(1) as f64;
    {
        let f = s.display_frame();
        r.width = f.w;
        r.height = f.h;
        let mut colors: Vec<u32> = f.px.iter().map(|p| p & 0xffffff).collect();
        colors.sort_unstable();
        colors.dedup();
        r.distinct_colors = colors.len();
    }
    r.aspect = s.aspect();

    // audio: what the device would get (resampled to 44.1 kHz)
    let captured = cap.borrow().clone();
    let (rms, hz) = audio::analyse(&captured, 44100);
    r.audio_rms = rms;
    r.audio_hz = hz;
    r.audio_seconds = captured.len() as f64 / 2.0 / 44100.0;
    audio::write_wav(&o.out.join("audio.wav"), 44100, &captured).map_err(|e| e.to_string())?;
    audio::write_wav(&o.out.join("audio-core.wav"), s.sample_rate().round() as u32, &samples).map_err(|e| e.to_string())?;

    // save states: determinism of serialize -> run -> unserialize -> run
    match s.core.serialize() {
        Some(state) => {
            r.state_bytes = state.len();
            let run_n = |s: &mut Session, held: &Held| {
                for _ in 0..30 {
                    s.run(held, false, None);
                }
                s.display_frame().into_owned()
            };
            let a = run_n(&mut s, &held);
            if s.core.unserialize(&state) {
                let b = run_n(&mut s, &held);
                r.state_deterministic = Some(a == b);
            } else {
                r.errors.push("unserialize failed".into());
                r.state_deterministic = Some(false);
            }
            if s.game.is_some() {
                match s.save_state(Slot::N(0)) {
                    Ok(()) => {
                        r.state_slot_saved = s.has_state(Slot::N(0));
                        r.state_thumb_saved = s.state_thumb(Slot::N(0)).is_some();
                        if let Err(e) = s.load_state(Slot::N(0)) {
                            r.errors.push(format!("load state: {e}"));
                        }
                    }
                    Err(e) => r.errors.push(format!("save state: {e}")),
                }
            }
        }
        None => r.errors.push("core has no save states".into()),
    }

    r.sram_bytes = s.core.memory(crate::lr::ffi::MEMORY_SAVE_RAM).map_or(0, |m| m.len());
    match s.save_sram() {
        Ok(_) => r.sram_saved = s.saves.as_ref().is_some_and(|p| p.srm.exists()),
        Err(e) => r.errors.push(format!("sram: {e}")),
    }
    r.ok = r.frames > 0 && r.distinct_colors > 1 && r.state_deterministic != Some(false) && r.errors.iter().all(|e| e.contains("no save states"));
    drop(s);
    let _ = std::fs::write(o.out.join("report.json"), serde_json::to_string_pretty(&r).unwrap_or_default());
    Ok(r)
}

/// Process CPU time (user + system, all threads), seconds.
fn process_cpu() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

/// Per-thread CPU ticks (Linux /proc/self/task; empty elsewhere): tid ->
/// (name, utime + stime).
fn task_ticks() -> std::collections::BTreeMap<u32, (String, u64)> {
    let mut m = std::collections::BTreeMap::new();
    let Ok(rd) = std::fs::read_dir("/proc/self/task") else { return m };
    for e in rd.flatten() {
        let Some(tid) = e.file_name().to_str().and_then(|t| t.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // "tid (comm) state ppid ..." - comm may contain spaces; fields after ')'
        let Some((head, rest)) = stat.rsplit_once(')') else { continue };
        let name = head.split_once('(').map_or("", |x| x.1).to_string();
        let f: Vec<&str> = rest.split_whitespace().collect();
        // rest[0] is field 3 (state): utime is field 14, stime 15
        let t = f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        m.insert(tid, (name, t));
    }
    m
}

/// Load every system's core (no game) and check the option presets against
/// what the core declares. Returns the exit status.
pub fn check(cfg: &Config) -> i32 {
    use crate::lr::core::{Core, Host};
    use crate::lr::options::Options;
    let mut bad = 0;
    let tmp = std::env::temp_dir();
    for sys in &cfg.systems {
        let presets = sys.options.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let core = crate::config::expand(&sys.core);
        match Core::open(&core, Host::new(Options::new(presets), &tmp, &tmp, &core)) {
            Ok(c) => {
                // some cores (fceumm, fbneo) declare their options only
                // once a game is loaded: nothing to check without one
                let deferred = c.host_ref().options.defs.is_empty();
                let problems = if deferred { Vec::new() } else { c.host_ref().options.preset_problems() };
                println!(
                    "{:10} {} {} [{}] {} options{}",
                    sys.id,
                    c.info.name,
                    c.info.version,
                    c.info.extensions.join("|"),
                    c.host_ref().options.defs.len(),
                    if deferred && !sys.options.is_empty() {
                        " (declared with the game: presets not checked)"
                    } else if problems.is_empty() {
                        ", presets ok"
                    } else {
                        ""
                    }
                );
                for (k, v, p) in problems {
                    println!("           preset {k}={v}: {p}");
                    bad += 1;
                }
            }
            Err(e) => {
                println!("{:10} FAILED: {e}", sys.id);
                bad += 1;
            }
        }
    }
    if bad > 0 { 1 } else { 0 }
}
