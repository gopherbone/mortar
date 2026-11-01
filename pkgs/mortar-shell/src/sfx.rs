// SPDX-License-Identifier: GPL-2.0-or-later
//! UI sounds: a tiny synthesizer (square/triangle/sine oscillators with an
//! exponential pitch sweep, band-passed noise with a swept centre, attack +
//! exponential decay envelopes) mirroring the prototype's WebAudio SFX
//! table, and a player thread that mixes voices into ALSA ("default": the
//! codec's dmix in modules/mortar-apps.nix, 44.1 kHz).
//!
//! The player opens the PCM on the first sound and closes it after a few
//! seconds of silence, so an idle shell holds no audio device. Writes block
//! only the player thread; the compositor just sends events down a channel.
//!
//!   sound     voices (delay s: what)
//!   tick      square 1320 Hz 35 ms
//!   move      triangle 880 -> 1040 Hz 50 ms
//!   tab       triangle 660 -> 990 Hz 60 ms
//!   open      noise band 300 -> 2600 Hz 320 ms (whoosh) + triangle 392 -> 784 Hz at 40 ms
//!   close     noise band 2400 -> 260 Hz 260 ms + triangle 784 -> 392 Hz
//!   drawer    triangle 523 Hz, then 784 Hz at 60 ms
//!   drawerUp  triangle 784 Hz, then 523 Hz at 50 ms
//!   chime     sines 523, 659, 784, 1047 Hz at 0, 120, 240, 380 ms
//!   shutter   noise 3 kHz 60 ms, noise 1500 -> 900 Hz at 70 ms
//!   unlock    squares 523, 659, 784, 1047, 1319 Hz every 70 ms
//!   deny      square 220 -> 160 Hz 120 ms
//!   vol(n)    sine 400 + 6n Hz 50 ms

pub const RATE: u32 = 44_100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sfx {
    Tick,
    Move,
    Tab,
    Open,
    Close,
    Drawer,
    DrawerUp,
    Chime,
    Shutter,
    Unlock,
    Deny,
    /// Volume feedback at level 0..100.
    Vol(u8),
}

impl Sfx {
    pub fn parse(s: &str) -> Option<Sfx> {
        Some(match s.to_ascii_lowercase().as_str() {
            "tick" => Sfx::Tick,
            "move" => Sfx::Move,
            "tab" => Sfx::Tab,
            "open" => Sfx::Open,
            "close" => Sfx::Close,
            "drawer" => Sfx::Drawer,
            "drawerup" | "drawer-up" => Sfx::DrawerUp,
            "chime" => Sfx::Chime,
            "shutter" => Sfx::Shutter,
            "unlock" => Sfx::Unlock,
            "deny" => Sfx::Deny,
            "vol" => Sfx::Vol(70),
            "none" => return None,
            _ => return None,
        })
    }
    pub const NAMES: &'static [&'static str] = &["tick", "move", "tab", "open", "close", "drawer", "drawerup", "chime", "shutter", "unlock", "deny", "vol"];
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Wave {
    Square,
    Triangle,
    Sine,
}

#[derive(Clone, Copy, Debug)]
enum Voice {
    Tone { f: f32, f2: Option<f32>, d: f32, wave: Wave, g: f32, delay: f32 },
    Noise { d: f32, f0: f32, f1: f32, g: f32, delay: f32 },
}

fn tone(f: f32, d: f32, wave: Wave, g: f32, f2: Option<f32>, delay: f32) -> Voice {
    Voice::Tone { f, f2, d, wave, g, delay }
}
fn noise(d: f32, f0: f32, f1: f32, g: f32, delay: f32) -> Voice {
    Voice::Noise { d, f0, f1, g, delay }
}

fn voices(s: Sfx) -> Vec<Voice> {
    use Wave::*;
    match s {
        Sfx::Tick => vec![tone(1320.0, 0.035, Square, 0.05, None, 0.0)],
        Sfx::Move => vec![tone(880.0, 0.05, Triangle, 0.09, Some(1040.0), 0.0)],
        Sfx::Tab => vec![tone(660.0, 0.06, Triangle, 0.08, Some(990.0), 0.0)],
        Sfx::Open => vec![noise(0.32, 300.0, 2600.0, 0.22, 0.0), tone(392.0, 0.18, Triangle, 0.08, Some(784.0), 0.04)],
        Sfx::Close => vec![noise(0.26, 2400.0, 260.0, 0.18, 0.0), tone(784.0, 0.16, Triangle, 0.07, Some(392.0), 0.0)],
        Sfx::Drawer => vec![tone(523.0, 0.07, Triangle, 0.08, None, 0.0), tone(784.0, 0.1, Triangle, 0.08, None, 0.06)],
        Sfx::DrawerUp => vec![tone(784.0, 0.07, Triangle, 0.07, None, 0.0), tone(523.0, 0.1, Triangle, 0.07, None, 0.05)],
        Sfx::Chime => vec![
            tone(523.0, 0.5, Sine, 0.13, None, 0.0),
            tone(659.0, 0.5, Sine, 0.11, None, 0.12),
            tone(784.0, 0.7, Sine, 0.11, None, 0.24),
            tone(1047.0, 0.9, Sine, 0.08, None, 0.38),
        ],
        Sfx::Shutter => vec![noise(0.06, 3000.0, 3000.0, 0.4, 0.0), noise(0.09, 1500.0, 900.0, 0.3, 0.07)],
        Sfx::Unlock => [523.0, 659.0, 784.0, 1047.0, 1319.0].iter().enumerate().map(|(i, f)| tone(*f, 0.16, Square, 0.06, None, i as f32 * 0.07)).collect(),
        Sfx::Deny => vec![tone(220.0, 0.12, Square, 0.07, Some(160.0), 0.0)],
        Sfx::Vol(n) => vec![tone(400.0 + n.min(100) as f32 * 6.0, 0.05, Sine, 0.1, None, 0.0)],
    }
}

/// WebAudio's envelope: 0 -> g linearly by `attack`, then an exponential
/// ramp to 0.0001 at `d`.
fn env(t: f32, attack: f32, d: f32, g: f32) -> f32 {
    if t < 0.0 || t > d {
        0.0
    } else if t < attack {
        g * t / attack
    } else {
        let k = (t - attack) / (d - attack).max(1e-4);
        g * (0.0001f32 / g).powf(k)
    }
}

/// Band-limited step correction (PolyBLEP) for the square wave.
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Render a sound as mono samples at `RATE`, `gain` = master volume.
pub fn render(s: Sfx, gain: f32) -> Vec<f32> {
    let vs = voices(s);
    let end = vs
        .iter()
        .map(|v| match v {
            Voice::Tone { d, delay, .. } | Voice::Noise { d, delay, .. } => d + delay + 0.02,
        })
        .fold(0.0, f32::max);
    let n = (end * RATE as f32).ceil() as usize;
    let mut out = vec![0.0f32; n];
    let sr = RATE as f32;
    let mut rng = Rng(0x9e37_79b9);
    for v in vs {
        match v {
            Voice::Tone { f, f2, d, wave, g, delay } => {
                let start = (delay * sr) as usize;
                let mut phase = 0.0f32;
                for (i, o) in out.iter_mut().enumerate().skip(start) {
                    let t = (i - start) as f32 / sr;
                    if t > d + 0.02 {
                        break;
                    }
                    let freq = match f2 {
                        Some(f2) => f * (f2 / f).powf((t / d).min(1.0)),
                        None => f,
                    };
                    let dt = freq / sr;
                    let x = match wave {
                        Wave::Sine => (phase * std::f32::consts::TAU).sin(),
                        Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
                        Wave::Square => {
                            let mut s = if phase < 0.5 { 1.0 } else { -1.0 };
                            s += blep(phase, dt);
                            s -= blep((phase + 0.5) % 1.0, dt);
                            s
                        }
                    };
                    *o += x * env(t, 0.004, d, g);
                    phase = (phase + dt) % 1.0;
                }
            }
            Voice::Noise { d, f0, f1, g, delay } => {
                let start = (delay * sr) as usize;
                let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                let mut co = [0.0f32; 5];
                for (i, o) in out.iter_mut().enumerate().skip(start) {
                    let t = (i - start) as f32 / sr;
                    if t > d + 0.02 {
                        break;
                    }
                    if (i - start) % 32 == 0 {
                        // RBJ band-pass (0 dB peak), Q 1.2, centre swept exponentially
                        let fc = f0 * (f1 / f0).powf((t / d).min(1.0));
                        let w0 = std::f32::consts::TAU * fc / sr;
                        let alpha = w0.sin() / (2.0 * 1.2);
                        let a0 = 1.0 + alpha;
                        co = [alpha / a0, 0.0, -alpha / a0, -2.0 * w0.cos() / a0, (1.0 - alpha) / a0];
                    }
                    let x = if t <= d { rng.next() } else { 0.0 };
                    let y = co[0] * x + co[1] * x1 + co[2] * x2 - co[3] * y1 - co[4] * y2;
                    x2 = x1;
                    x1 = x;
                    y2 = y1;
                    y1 = y;
                    *o += y * env(t, 0.3 * d, d, g);
                }
            }
        }
    }
    for o in &mut out {
        *o = (*o * gain).clamp(-1.0, 1.0);
    }
    out
}

/// 16-bit mono WAV (for `mortar-shell sfx NAME --wav FILE`).
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| ((s * 32767.0) as i16).to_le_bytes()).collect();
    let mut v = Vec::with_capacity(44 + data.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&RATE.to_le_bytes());
    v.extend_from_slice(&(RATE * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// The player: a thread that mixes rendered sounds into the PCM.
pub struct Player {
    tx: Option<std::sync::mpsc::Sender<Vec<f32>>>,
}

impl Player {
    /// `device`: an ALSA PCM name ("default"); None: silent.
    pub fn new(device: Option<String>) -> Player {
        let Some(dev) = device else { return Player { tx: None } };
        let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
        let r = std::thread::Builder::new().name("mortar-sfx".into()).spawn(move || alsa::run(&dev, rx));
        match r {
            Ok(_) => Player { tx: Some(tx) },
            Err(e) => {
                tracing::warn!("sound thread: {e}");
                Player { tx: None }
            }
        }
    }
    pub fn play(&self, s: Sfx, volume: u8) {
        if volume == 0 {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(render(s, volume as f32 / 100.0));
        }
    }
}

#[cfg(target_os = "linux")]
mod alsa {
    use super::RATE;
    use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr, CString};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant};

    #[allow(non_camel_case_types)]
    type snd_pcm_t = c_void;
    #[link(name = "asound")]
    unsafe extern "C" {
        fn snd_pcm_open(pcm: *mut *mut snd_pcm_t, name: *const c_char, stream: c_int, mode: c_int) -> c_int;
        fn snd_pcm_set_params(pcm: *mut snd_pcm_t, format: c_int, access: c_int, channels: c_uint, rate: c_uint, soft_resample: c_int, latency: c_uint) -> c_int;
        fn snd_pcm_writei(pcm: *mut snd_pcm_t, buf: *const c_void, frames: c_long) -> c_long;
        fn snd_pcm_recover(pcm: *mut snd_pcm_t, err: c_int, silent: c_int) -> c_int;
        fn snd_pcm_drain(pcm: *mut snd_pcm_t) -> c_int;
        fn snd_pcm_close(pcm: *mut snd_pcm_t) -> c_int;
        fn snd_strerror(err: c_int) -> *const c_char;
    }
    const SND_PCM_STREAM_PLAYBACK: c_int = 0;
    const SND_PCM_FORMAT_S16_LE: c_int = 2;
    const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
    const PERIOD: usize = 256;

    fn err(e: c_int) -> String {
        // SAFETY: snd_strerror returns a static string
        unsafe { CStr::from_ptr(snd_strerror(e)).to_string_lossy().into_owned() }
    }

    struct Pcm(*mut snd_pcm_t);
    impl Pcm {
        fn open(dev: &str) -> Result<Pcm, String> {
            let name = CString::new(dev).map_err(|e| e.to_string())?;
            let mut p = std::ptr::null_mut();
            // SAFETY: valid out pointer and C string
            let r = unsafe { snd_pcm_open(&mut p, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
            if r < 0 {
                return Err(format!("open {dev}: {}", err(r)));
            }
            let pcm = Pcm(p);
            // 30 ms of buffering: low latency, still safe on a busy A53
            // SAFETY: open handle
            let r = unsafe { snd_pcm_set_params(pcm.0, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, 2, RATE, 1, 30_000) };
            if r < 0 {
                return Err(format!("set params: {}", err(r)));
            }
            Ok(pcm)
        }
        fn write(&self, frames: &[i16]) -> Result<(), String> {
            let mut off = 0;
            let n = frames.len() / 2;
            while off < n {
                // SAFETY: frames holds n stereo frames
                let r = unsafe { snd_pcm_writei(self.0, frames[off * 2..].as_ptr() as *const c_void, (n - off) as c_long) };
                if r < 0 {
                    // SAFETY: open handle
                    let rr = unsafe { snd_pcm_recover(self.0, r as c_int, 1) };
                    if rr < 0 {
                        return Err(format!("write: {}", err(r as c_int)));
                    }
                } else {
                    off += r as usize;
                }
            }
            Ok(())
        }
    }
    impl Drop for Pcm {
        fn drop(&mut self) {
            // SAFETY: open handle, closed once
            unsafe {
                snd_pcm_drain(self.0);
                snd_pcm_close(self.0);
            }
        }
    }

    pub fn run(dev: &str, rx: Receiver<Vec<f32>>) {
        let mut voices: Vec<(Vec<f32>, usize)> = Vec::new();
        let mut pcm: Option<Pcm> = None;
        let mut last_sound = Instant::now();
        let mut retry_at: Option<Instant> = None;
        let mut buf = vec![0i16; PERIOD * 2];
        loop {
            if voices.is_empty() {
                let wait = if pcm.is_some() { Duration::from_millis(500) } else { Duration::from_secs(3600) };
                match rx.recv_timeout(wait) {
                    Ok(v) => voices.push((v, 0)),
                    Err(RecvTimeoutError::Timeout) => {
                        if pcm.is_some() && last_sound.elapsed() > Duration::from_secs(4) {
                            pcm = None; // drain and close: no device held while idle
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            while let Ok(v) = rx.try_recv() {
                voices.push((v, 0));
            }
            if pcm.is_none() {
                if retry_at.is_some_and(|t| Instant::now() < t) {
                    voices.clear();
                    continue;
                }
                match Pcm::open(dev) {
                    Ok(p) => pcm = Some(p),
                    Err(e) => {
                        tracing::warn!("UI sounds off for a while: {e}");
                        retry_at = Some(Instant::now() + Duration::from_secs(30));
                        voices.clear();
                        continue;
                    }
                }
            }
            for f in 0..PERIOD {
                let mut s = 0.0f32;
                for (v, pos) in voices.iter_mut() {
                    if let Some(x) = v.get(*pos + f) {
                        s += x;
                    }
                }
                let s = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                buf[f * 2] = s;
                buf[f * 2 + 1] = s;
            }
            for (_, pos) in voices.iter_mut() {
                *pos += PERIOD;
            }
            voices.retain(|(v, pos)| *pos < v.len());
            last_sound = Instant::now();
            if let Some(p) = &pcm
                && let Err(e) = p.write(&buf)
            {
                tracing::warn!("UI sounds: {e}");
                pcm = None;
                voices.clear();
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod alsa {
    pub fn run(_dev: &str, rx: std::sync::mpsc::Receiver<Vec<f32>>) {
        while rx.recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_render_with_expected_lengths() {
        let tick = render(Sfx::Tick, 1.0);
        assert!((tick.len() as f32 / RATE as f32 - 0.055).abs() < 0.002);
        let chime = render(Sfx::Chime, 1.0);
        assert!(chime.len() as f32 / RATE as f32 > 1.2);
        for s in Sfx::NAMES {
            let v = render(Sfx::parse(s).unwrap(), 0.6);
            let peak = v.iter().fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(peak > 0.005 && peak <= 0.6, "{s}: peak {peak}");
        }
        // the whoosh sweeps: more energy late than early in a high band?
        // (just check it is not silent in either half)
        let open = render(Sfx::Open, 1.0);
        let half = open.len() / 2;
        let e1: f32 = open[..half].iter().map(|x| x * x).sum();
        let e2: f32 = open[half..].iter().map(|x| x * x).sum();
        assert!(e1 > 0.0 && e2 > 0.0);
    }

    #[test]
    fn wav_header() {
        let w = wav(&[0.0; 10]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 20);
    }
}
