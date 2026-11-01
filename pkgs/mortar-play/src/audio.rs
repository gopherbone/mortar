// SPDX-License-Identifier: GPL-2.0-or-later
//! Audio: the core's samples (its own rate, e.g. 32040.5 Hz for the SNES)
//! resampled to the device rate (44.1 kHz on the Brick: dmix on the codec)
//! with dynamic rate control.
//!
//! Video is paced by the frame clock (or the display), audio by the sound
//! card's clock; the two drift apart by a few hundred ppm, and a game's
//! frame rate rarely matches the display exactly (60.0988 Hz SNES on a
//! 60 Hz panel). Dynamic rate control (as in RetroArch) nudges the
//! resampling ratio by at most ±0.5% from the buffer fill level, so the
//! queue stays half full: no underrun crackle, no growing latency, and the
//! pitch change is inaudible.

/// Cubic (Hermite) resampler for interleaved stereo i16.
pub struct Resampler {
    /// Last three input frames, the history the next block interpolates from.
    hist: [[f32; 2]; 3],
    /// Read position in the [hist ++ input] buffer, in frames.
    pos: f64,
    buf: Vec<[f32; 2]>,
}

impl Default for Resampler {
    fn default() -> Self {
        Resampler { hist: [[0.0; 2]; 3], pos: 1.0, buf: Vec::new() }
    }
}

#[inline]
fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c0 = x1;
    let c1 = 0.5 * (x2 - x0);
    let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
    let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
    ((c3 * t + c2) * t + c1) * t + c0
}

impl Resampler {
    /// Resample `input` (stereo frames) by `ratio` = output rate / input
    /// rate, appending to `out`.
    pub fn process(&mut self, input: &[i16], ratio: f64, out: &mut Vec<i16>) {
        let frames = input.len() / 2;
        if frames == 0 || ratio <= 0.0 {
            return;
        }
        self.buf.clear();
        self.buf.extend_from_slice(&self.hist);
        self.buf.extend(input.as_chunks::<2>().0.iter().map(|c| [c[0] as f32, c[1] as f32]));
        let n = self.buf.len();
        let step = 1.0 / ratio;
        let mut pos = self.pos;
        out.reserve((frames as f64 * ratio) as usize * 2 + 4);
        while (pos as usize) + 2 < n {
            let i = pos as usize;
            let t = (pos - i as f64) as f32;
            for ch in 0..2 {
                let v = hermite(self.buf[i - 1][ch], self.buf[i][ch], self.buf[i + 1][ch], self.buf[i + 2][ch], t);
                out.push(v.round().clamp(-32768.0, 32767.0) as i16);
            }
            pos += step;
        }
        self.hist.copy_from_slice(&self.buf[n - 3..]);
        self.pos = pos - (n - 3) as f64;
    }

    pub fn reset(&mut self) {
        *self = Resampler::default();
    }
}

/// Dynamic rate control: the resampling ratio for a buffer fill level.
#[derive(Clone, Copy, Debug)]
pub struct Drc {
    /// Nominal output/input rate ratio.
    pub base: f64,
    /// Largest relative adjustment (0.005 = ±0.5%).
    pub max_dev: f64,
}

impl Drc {
    pub fn new(core_rate: f64, device_rate: f64) -> Drc {
        Drc { base: device_rate / core_rate.max(1.0), max_dev: 0.005 }
    }
    /// `fill`: queued / (2 x target), 0.5 when at the target latency.
    pub fn ratio(&self, fill: f64) -> f64 {
        let f = fill.clamp(0.0, 1.0);
        self.base * (1.0 + self.max_dev * (1.0 - 2.0 * f))
    }
}

/// Where resampled audio goes.
pub trait AudioOut {
    /// Device sample rate (stereo i16).
    fn rate(&self) -> u32;
    /// Frames queued and not yet played.
    fn queued(&self) -> usize;
    fn push(&mut self, s: &[i16]);
    fn set_paused(&mut self, paused: bool);
    fn clear(&mut self);
}

/// No sound device: everything is dropped, the queue always looks on target.
pub struct NullAudio {
    pub rate: u32,
}

impl AudioOut for NullAudio {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn queued(&self) -> usize {
        usize::MAX
    }
    fn push(&mut self, _: &[i16]) {}
    fn set_paused(&mut self, _: bool) {}
    fn clear(&mut self) {}
}

/// Headless: keep everything (tests write it to a WAV).
pub struct CaptureAudio {
    pub rate: u32,
    pub samples: std::rc::Rc<std::cell::RefCell<Vec<i16>>>,
}

impl AudioOut for CaptureAudio {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn queued(&self) -> usize {
        usize::MAX
    }
    fn push(&mut self, s: &[i16]) {
        self.samples.borrow_mut().extend_from_slice(s);
    }
    fn set_paused(&mut self, _: bool) {}
    fn clear(&mut self) {}
}

/// The core-to-device path: resampler, DRC and latency target.
pub struct Mixer {
    pub out: Box<dyn AudioOut>,
    rs: Resampler,
    drc: Drc,
    /// Target queue length in frames.
    pub target: usize,
    tmp: Vec<i16>,
    pub muted: bool,
}

impl Mixer {
    pub fn new(out: Box<dyn AudioOut>, latency_ms: u32) -> Mixer {
        let target = (out.rate() as usize * latency_ms as usize / 1000).max(256);
        let drc = Drc::new(out.rate() as f64, out.rate() as f64);
        Mixer { out, rs: Resampler::default(), drc, target, tmp: Vec::new(), muted: false }
    }

    pub fn set_core_rate(&mut self, core_rate: f64) {
        self.drc = Drc::new(core_rate, self.out.rate() as f64);
        self.rs.reset();
    }

    /// Fill level as DRC sees it (0.5 = on target).
    pub fn fill(&self) -> f64 {
        let q = self.out.queued();
        if q == usize::MAX { 0.5 } else { q as f64 / (2 * self.target) as f64 }
    }

    /// Buffer occupancy in percent of 2 x target (for the core's
    /// audio-buffer-status callback / frameskip).
    pub fn occupancy(&self) -> u32 {
        (self.fill() * 100.0).round().clamp(0.0, 100.0) as u32
    }

    pub fn push(&mut self, core: &[i16]) {
        if self.muted || core.is_empty() {
            return;
        }
        let ratio = self.drc.ratio(self.fill());
        self.tmp.clear();
        self.rs.process(core, ratio, &mut self.tmp);
        // a hard cap: never queue more than 4x target (e.g. after a stall)
        let q = self.out.queued();
        if q != usize::MAX && q > 4 * self.target {
            return;
        }
        self.out.push(&self.tmp);
    }
}

/// Write interleaved stereo i16 as a WAV file.
pub fn write_wav(path: &std::path::Path, rate: u32, s: &[i16]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data = (s.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for v in s {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

/// Simple measurements for tests: RMS and the dominant frequency estimated
/// from zero crossings of the left channel.
pub fn analyse(s: &[i16], rate: u32) -> (f64, f64) {
    let left: Vec<f64> = s.iter().step_by(2).map(|&v| v as f64).collect();
    if left.is_empty() {
        return (0.0, 0.0);
    }
    let mean = left.iter().sum::<f64>() / left.len() as f64;
    let rms = (left.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / left.len() as f64).sqrt();
    // frequency from the span between the first and last rising zero
    // crossing, so leading/trailing silence doesn't skew it
    let ups: Vec<usize> = left.windows(2).enumerate().filter(|(_, w)| (w[0] - mean) < 0.0 && (w[1] - mean) >= 0.0).map(|(i, _)| i).collect();
    let hz = match (ups.first(), ups.last()) {
        (Some(&a), Some(&b)) if b > a => (ups.len() - 1) as f64 * rate as f64 / (b - a) as f64,
        _ => 0.0,
    };
    (rms, hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: f64, hz: f64, frames: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|i| {
                let v = ((i as f64 * hz * 2.0 * std::f64::consts::PI / rate).sin() * 12000.0) as i16;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn resample_length_and_pitch() {
        let mut rs = Resampler::default();
        let input = sine(32040.5, 440.0, 32040);
        let mut out = Vec::new();
        // feed in frame-sized blocks like a core does
        for block in input.chunks(534 * 2) {
            rs.process(block, 44100.0 / 32040.5, &mut out);
        }
        let frames = out.len() / 2;
        assert!((frames as i64 - 44100).abs() < 50, "{frames}");
        let (rms, hz) = analyse(&out, 44100);
        assert!(rms > 7000.0, "rms {rms}");
        assert!((hz - 440.0).abs() < 3.0, "freq {hz}");
    }

    #[test]
    fn identity_ratio_is_transparent() {
        let mut rs = Resampler::default();
        let input: Vec<i16> = (0..200).flat_map(|i| [i as i16 * 10, -(i as i16) * 10]).collect();
        let mut out = Vec::new();
        rs.process(&input, 1.0, &mut out);
        // output lags by two frames of history, otherwise identical
        assert_eq!(&out[4..20], &input[0..16]);
    }

    #[test]
    fn drc_direction() {
        let d = Drc::new(44100.0, 44100.0);
        assert!((d.ratio(0.5) - 1.0).abs() < 1e-9);
        assert!(d.ratio(0.9) < 1.0, "fuller buffer -> fewer samples");
        assert!(d.ratio(0.1) > 1.0);
        assert!((d.ratio(5.0) - 0.995).abs() < 1e-9);
    }

    /// Simulate a device that plays 0.2% slower than nominal: with DRC the
    /// queue converges instead of growing.
    #[test]
    fn drc_tracks_clock_drift() {
        struct Sim {
            q: f64,
        }
        let mut q = Sim { q: 0.0 };
        let target = 2205.0;
        let drc = Drc::new(32040.5, 44100.0);
        let mut rs = Resampler::default();
        let block = sine(32040.5, 300.0, 534);
        let mut out = Vec::new();
        for _ in 0..3000 {
            out.clear();
            let fill = q.q / (2.0 * target);
            rs.process(&block, drc.ratio(fill), &mut out);
            q.q += (out.len() / 2) as f64;
            // the device consumes one 60.0988 Hz frame worth at 0.998 x 44100
            q.q = (q.q - 44100.0 * 0.998 / 60.0988).max(0.0);
        }
        // proportional control settles where the ratio matches the drift
        // (here 0.36% slow: fill ~0.86) instead of overflowing
        let fill = q.q / (2.0 * target);
        assert!(fill > 0.5 && fill < 0.95, "fill {fill}");
    }
}
