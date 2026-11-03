// SPDX-License-Identifier: MIT
//! Sound: seed -> ps_sfx_generate (sfxr.js port, 22050 Hz mono i16) -> cached PCM, played on a
//! small pool of SDL3 audio streams bound to one playback device (SDL mixes bound streams and
//! resamples to the device rate).
use crate::ffi;
use sdl3::audio::{AudioDevice, AudioFormat, AudioSpec, AudioStreamOwner};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const VOICES: usize = 6;
const SFX_RATE: i32 = 22050;

pub struct Audio {
    out: Option<Output>,
    cache: HashMap<i64, Vec<i16>>,
    pending: VecDeque<i64>,
    pub muted: bool,
}

struct Output {
    _device: AudioDevice,
    voices: Vec<AudioStreamOwner>,
    voice_seed: Vec<i64>,
    next: usize,
}

impl Audio {
    pub fn new(sys: Option<&sdl3::AudioSubsystem>) -> Audio {
        let out = sys.and_then(|a| match Output::open(a) {
            Ok(o) => Some(o),
            Err(e) => {
                eprintln!("audio: {e} (continuing without sound)");
                None
            }
        });
        Audio { out, cache: HashMap::new(), pending: VecDeque::new(), muted: false }
    }

    /// New game: drop cached sounds and queue the game's seeds for pre-generation.
    pub fn set_game(&mut self, seeds: Vec<i64>) {
        self.cache.clear();
        self.pending = seeds.into();
        if let Some(o) = &mut self.out {
            for v in &o.voices {
                let _ = v.clear();
            }
        }
    }

    fn get(&mut self, seed: i64) -> Option<&Vec<i16>> {
        if !self.cache.contains_key(&seed) {
            let (pcm, rate) = ffi::sfx_generate(seed)?;
            debug_assert_eq!(rate, SFX_RATE);
            self.cache.insert(seed, pcm);
        }
        self.cache.get(&seed)
    }

    /// Generates queued sounds until `budget` has passed. Returns true if more remain.
    pub fn pregenerate(&mut self, budget: Duration) -> bool {
        let t0 = Instant::now();
        while let Some(seed) = self.pending.pop_front() {
            if !self.cache.contains_key(&seed) {
                self.get(seed);
                if t0.elapsed() >= budget {
                    break;
                }
            }
        }
        !self.pending.is_empty()
    }

    pub fn play(&mut self, seed: i64) {
        if self.out.is_none() || self.muted {
            return;
        }
        let pcm = match self.get(seed) {
            Some(p) => p.clone(),
            None => return,
        };
        let o = self.out.as_mut().unwrap();
        // the same sound again restarts it; otherwise a free voice, else the oldest one
        let idx = o
            .voice_seed
            .iter()
            .position(|&s| s == seed)
            .or_else(|| o.voices.iter().position(|v| v.queued_bytes().unwrap_or(0) == 0))
            .unwrap_or_else(|| {
                let i = o.next;
                o.next = (o.next + 1) % VOICES;
                i
            });
        let v = &o.voices[idx];
        let _ = v.clear();
        let _ = v.put_data_i16(&pcm);
        let _ = v.flush();
        o.voice_seed[idx] = seed;
    }
}

impl Output {
    fn open(a: &sdl3::AudioSubsystem) -> Result<Output, String> {
        let src = AudioSpec { freq: Some(SFX_RATE), channels: Some(1), format: Some(AudioFormat::s16_sys()) };
        let device = a.open_playback_device(&AudioSpec { freq: None, channels: None, format: None }).map_err(|e| e.to_string())?;
        let mut voices = Vec::new();
        for _ in 0..VOICES {
            let s = a.new_playback_stream(&src, None).map_err(|e| e.to_string())?;
            device.bind_stream(&s).map_err(|e| e.to_string())?;
            voices.push(s);
        }
        let _ = device.resume();
        Ok(Output { _device: device, voices, voice_seed: vec![-1; VOICES], next: 0 })
    }
}
