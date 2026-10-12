use anyhow::{bail, Context, Result};
use hound::{SampleFormat, WavReader};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplerConfig {
    pub sample_path: PathBuf,
    pub root_note: u8,       // MIDI note where playback speed is 1.0 (default 60 = C4, or 36 = C2)
    pub loop_enabled: bool,
    pub loop_start_sample: usize,
    pub loop_end_sample: usize,
    pub choke_group: Option<u8>,
    pub gain: f32,           // 0.0 to 2.0
    pub pan: f32,            // -1.0 to 1.0
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            sample_path: PathBuf::new(),
            root_note: 60, // C4
            loop_enabled: false,
            loop_start_sample: 0,
            loop_end_sample: 0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            attack_ms: 1.0,
            decay_ms: 5000.0,
            sustain: 1.0,
            release_ms: 50.0,
        }
    }
}

pub struct SampleBuffer {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples_left: Vec<f32>,
    pub samples_right: Vec<f32>,
}

impl SampleBuffer {
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let mut reader = WavReader::open(path)
            .with_context(|| format!("Failed to open WAV file at {:?}", path))?;

        let spec = reader.spec();
        let channels = spec.channels;
        let sample_rate = spec.sample_rate;

        let mut samples_left = Vec::new();
        let mut samples_right = Vec::new();

        match (spec.sample_format, spec.bits_per_sample) {
            (SampleFormat::Int, 16) => {
                let mut channel_idx = 0;
                for sample in reader.samples::<i16>() {
                    let s = (sample? as f32) / 32768.0;
                    if channel_idx == 0 {
                        samples_left.push(s);
                        if channels == 1 {
                            samples_right.push(s);
                        }
                    } else {
                        samples_right.push(s);
                    }
                    channel_idx = (channel_idx + 1) % channels;
                }
            }
            (SampleFormat::Int, 24) => {
                let mut channel_idx = 0;
                for sample in reader.samples::<i32>() {
                    let s = (sample? as f32) / 8388608.0;
                    if channel_idx == 0 {
                        samples_left.push(s);
                        if channels == 1 {
                            samples_right.push(s);
                        }
                    } else {
                        samples_right.push(s);
                    }
                    channel_idx = (channel_idx + 1) % channels;
                }
            }
            (SampleFormat::Float, 32) => {
                let mut channel_idx = 0;
                for sample in reader.samples::<f32>() {
                    let s = sample?;
                    if channel_idx == 0 {
                        samples_left.push(s);
                        if channels == 1 {
                            samples_right.push(s);
                        }
                    } else {
                        samples_right.push(s);
                    }
                    channel_idx = (channel_idx + 1) % channels;
                }
            }
            _ => bail!(
                "Unsupported WAV format: {:?} with {} bits",
                spec.sample_format,
                spec.bits_per_sample
            ),
        }

        Ok(Self {
            channels,
            sample_rate,
            samples_left,
            samples_right,
        })
    }
}

pub struct SamplerVoice {
    pub note: u8,
    pub velocity: f32,
    pub pos: f64,
    pub rate: f64,
    pub env_val: f32,
    pub env_state: VoiceState,
    pub is_active: bool,
    pub choke_group: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoiceState {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

impl SamplerVoice {
    pub fn new() -> Self {
        Self {
            note: 0,
            velocity: 0.0,
            pos: 0.0,
            rate: 1.0,
            env_val: 0.0,
            env_state: VoiceState::Idle,
            is_active: false,
            choke_group: None,
        }
    }
}

pub struct SamplerInstrument {
    pub config: SamplerConfig,
    pub buffer: Option<SampleBuffer>,
    voices: Vec<SamplerVoice>,
    sample_rate: f32,
}

impl SamplerInstrument {
    pub fn new(config: SamplerConfig, sample_rate: f32) -> Self {
        let mut voices = Vec::with_capacity(16);
        for _ in 0..16 {
            voices.push(SamplerVoice::new());
        }

        let buffer = if config.sample_path.as_os_str().is_empty() {
            None
        } else {
            SampleBuffer::load_from_file(&config.sample_path).ok()
        };

        Self {
            config,
            buffer,
            voices,
            sample_rate: if sample_rate > 0.0 { sample_rate } else { 44100.0 },
        }
    }

    pub fn set_sample_rate(&mut self, rate: f32) {
        if rate > 0.0 {
            self.sample_rate = rate;
        }
    }

    pub fn load_sample(&mut self, path: &Path) -> Result<()> {
        let buf = SampleBuffer::load_from_file(path)?;
        self.config.sample_path = path.to_path_buf();
        self.buffer = Some(buf);
        Ok(())
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        let buf = match &self.buffer {
            Some(b) => b,
            None => return,
        };

        if buf.samples_left.is_empty() {
            return;
        }

        // Choke group handling: choke any currently playing voices in the same choke group
        if let Some(choke) = self.config.choke_group {
            for v in &mut self.voices {
                if v.is_active && v.choke_group == Some(choke) {
                    v.env_state = VoiceState::Release;
                }
            }
        }

        // Calculate pitch ratio relative to root note
        // semitones = note - root_note
        // speed = 2^(semitones / 12) * (file_sample_rate / engine_sample_rate)
        let semitones = note as f64 - self.config.root_note as f64;
        let pitch_mult = (2.0f64).powf(semitones / 12.0);
        let rate = pitch_mult * (buf.sample_rate as f64 / self.sample_rate as f64);

        // Find available voice or steal oldest
        let mut target_idx = None;
        for (i, v) in self.voices.iter().enumerate() {
            if !v.is_active {
                target_idx = Some(i);
                break;
            }
        }

        let idx = target_idx.unwrap_or(0);
        let v = &mut self.voices[idx];
        v.note = note;
        v.velocity = velocity.clamp(0.0, 1.0);
        v.pos = 0.0;
        v.rate = rate;
        v.env_val = 0.0;
        v.env_state = VoiceState::Attack;
        v.is_active = true;
        v.choke_group = self.config.choke_group;
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.is_active && v.note == note {
                v.env_state = VoiceState::Release;
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            v.is_active = false;
            v.env_state = VoiceState::Idle;
        }
    }

    /// Render a block of stereo audio
    pub fn process_block(&mut self, left_out: &mut [f32], right_out: &mut [f32]) {
        let buf = match &self.buffer {
            Some(b) => b,
            None => return,
        };

        let len = buf.samples_left.len();
        if len == 0 {
            return;
        }

        let num_samples = left_out.len().min(right_out.len());
        let attack_step = 1.0 / (self.config.attack_ms.max(0.5) * 0.001 * self.sample_rate).max(1.0);
        let decay_mult = (-3.0 / (self.config.decay_ms.max(1.0) * 0.001 * self.sample_rate).max(1.0)).exp();
        let release_mult = (-4.0 / (self.config.release_ms.max(1.0) * 0.001 * self.sample_rate).max(1.0)).exp();

        let pan_left = (1.0 - self.config.pan).clamp(0.0, 1.0);
        let pan_right = (1.0 + self.config.pan).clamp(0.0, 1.0);

        for i in 0..num_samples {
            let mut sample_l = 0.0f32;
            let mut sample_r = 0.0f32;

            for v in &mut self.voices {
                if !v.is_active {
                    continue;
                }

                // Envelope processing
                match v.env_state {
                    VoiceState::Idle => {
                        v.is_active = false;
                        continue;
                    }
                    VoiceState::Attack => {
                        v.env_val += attack_step;
                        if v.env_val >= 1.0 {
                            v.env_val = 1.0;
                            v.env_state = VoiceState::Decay;
                        }
                    }
                    VoiceState::Decay => {
                        let sus = self.config.sustain;
                        v.env_val = sus + (v.env_val - sus) * decay_mult;
                        if (v.env_val - sus).abs() < 0.001 {
                            v.env_val = sus;
                            v.env_state = VoiceState::Sustain;
                        }
                    }
                    VoiceState::Sustain => {
                        v.env_val = self.config.sustain;
                    }
                    VoiceState::Release => {
                        v.env_val *= release_mult;
                        if v.env_val < 0.0001 {
                            v.env_val = 0.0;
                            v.env_state = VoiceState::Idle;
                            v.is_active = false;
                            continue;
                        }
                    }
                }

                let idx0 = v.pos.floor() as usize;
                let idx1 = (idx0 + 1).min(len - 1);
                let frac = (v.pos - idx0 as f64) as f32;

                if idx0 >= len {
                    if self.config.loop_enabled && self.config.loop_end_sample > self.config.loop_start_sample {
                        v.pos = self.config.loop_start_sample as f64;
                    } else {
                        v.is_active = false;
                        continue;
                    }
                }

                // Linear interpolation
                let s_l = buf.samples_left[idx0] * (1.0 - frac) + buf.samples_left[idx1] * frac;
                let s_r = buf.samples_right[idx0] * (1.0 - frac) + buf.samples_right[idx1] * frac;

                let amp = v.env_val * v.velocity * self.config.gain;
                sample_l += s_l * amp * pan_left;
                sample_r += s_r * amp * pan_right;

                v.pos += v.rate;
            }

            left_out[i] += sample_l;
            right_out[i] += sample_r;
        }
    }
}
