use crate::audio_clip::AudioClipTrack;
use crate::clap_host::ClapTrack;
use crate::lv2_host::Lv2Track;
use crate::sampler::SamplerInstrument;
use jamsynth::JamSynthEngine;

pub enum TrackInstrument {
    Synth(JamSynthEngine),
    Sampler(SamplerInstrument),
    AudioClips(AudioClipTrack),
    Clap(Box<ClapTrack>),
    Lv2(Box<Lv2Track>),
}

pub enum InsertEffect {
    Clap(Box<ClapTrack>),
    Lv2(Box<Lv2Track>),
}

pub struct Track {
    pub id: String,
    pub name: String,
    pub instrument: TrackInstrument,
    pub inserts: Vec<InsertEffect>,
    pub muted: bool,
    pub solo: bool,
    pub volume: f32, // Gain multiplier (0.0 to 2.0)
    pub pan: f32,    // -1.0 (hard left) to 1.0 (hard right)
    pub current_plugin: Option<String>,
    pub peak_level: f32,
}

impl Track {
    pub fn new_synth(id: &str, name: &str, engine: JamSynthEngine) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            instrument: TrackInstrument::Synth(engine),
            inserts: Vec::new(),
            muted: false,
            solo: false,
            volume: 1.0,
            pan: 0.0,
            current_plugin: Some("JamSynth (Built-in)".to_string()),
            peak_level: 0.0,
        }
    }

    pub fn new_clap(id: &str, name: &str, clap: ClapTrack, plugin_name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            instrument: TrackInstrument::Clap(Box::new(clap)),
            inserts: Vec::new(),
            muted: false,
            solo: false,
            volume: 1.0,
            pan: 0.0,
            current_plugin: Some(plugin_name.to_string()),
            peak_level: 0.0,
        }
    }

    pub fn new_lv2(id: &str, name: &str, lv2: Lv2Track, plugin_name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            instrument: TrackInstrument::Lv2(Box::new(lv2)),
            inserts: Vec::new(),
            muted: false,
            solo: false,
            volume: 1.0,
            pan: 0.0,
            current_plugin: Some(plugin_name.to_string()),
            peak_level: 0.0,
        }
    }

    pub fn new_sampler(id: &str, name: &str, sampler: SamplerInstrument) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            instrument: TrackInstrument::Sampler(sampler),
            inserts: Vec::new(),
            muted: false,
            solo: false,
            volume: 1.0,
            pan: 0.0,
            current_plugin: Some("Sampler".to_string()),
            peak_level: 0.0,
        }
    }

    pub fn new_audio_clips(id: &str, name: &str, clips: AudioClipTrack) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            instrument: TrackInstrument::AudioClips(clips),
            inserts: Vec::new(),
            muted: false,
            solo: false,
            volume: 1.0,
            pan: 0.0,
            current_plugin: None,
            peak_level: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, rate: f32) {
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => synth.set_sample_rate(rate),
            TrackInstrument::Sampler(sampler) => sampler.set_sample_rate(rate),
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(_) => {}
            TrackInstrument::Lv2(_) => {}
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        if self.muted {
            return;
        }
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => synth.note_on(note, velocity),
            TrackInstrument::Sampler(sampler) => sampler.note_on(note, velocity),
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(clap) => clap.note_on(note, velocity),
            TrackInstrument::Lv2(lv2) => lv2.note_on(note, velocity),
        }
    }

    pub fn note_off(&mut self, note: u8) {
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => synth.note_off(note),
            TrackInstrument::Sampler(sampler) => sampler.note_off(note),
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(clap) => clap.note_off(note),
            TrackInstrument::Lv2(lv2) => lv2.note_off(note),
        }
    }

    pub fn all_notes_off(&mut self) {
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => synth.all_notes_off(),
            TrackInstrument::Sampler(sampler) => sampler.all_notes_off(),
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(clap) => clap.all_notes_off(),
            TrackInstrument::Lv2(lv2) => lv2.all_notes_off(),
        }
    }

    pub fn handle_cc(&mut self, cc: u8, value: u8) {
        if let TrackInstrument::Synth(synth) = &mut self.instrument {
            synth.handle_cc(cc, value);
        }
    }

    /// Set a named DSP / engine parameter live
    pub fn set_parameter(&mut self, param_name: &str, value: f32) {
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => {
                match param_name {
                    "cutoff" => synth.patch.filter.cutoff_hz = value.clamp(20.0, 20000.0),
                    "resonance" => synth.patch.filter.resonance = value.clamp(0.0, 1.0),
                    "glide_ms" => synth.patch.glide_ms = value.clamp(0.0, 1000.0),
                    "drive" => synth.patch.drive.amount = value.clamp(0.0, 1.0),
                    "attack_ms" => synth.patch.amp_env.attack_ms = value.clamp(0.1, 5000.0),
                    "decay_ms" => synth.patch.amp_env.decay_ms = value.clamp(1.0, 5000.0),
                    "sustain" => synth.patch.amp_env.sustain = value.clamp(0.0, 1.0),
                    "release_ms" => synth.patch.amp_env.release_ms = value.clamp(1.0, 5000.0),
                    _ => {}
                }
            }
            TrackInstrument::Sampler(_) => {}
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(clap) => clap.set_parameter(param_name, value),
            TrackInstrument::Lv2(lv2) => lv2.set_parameter(param_name, value),
        }
    }

    /// Retrieve the current editable parameters for this track
    pub fn get_parameters(&self) -> Vec<(String, f32, f32, f32)> {
        match &self.instrument {
            TrackInstrument::Synth(synth) => {
                vec![
                    ("cutoff".to_string(), synth.patch.filter.cutoff_hz, 40.0, 12000.0),
                    ("resonance".to_string(), synth.patch.filter.resonance, 0.0, 0.95),
                    ("glide_ms".to_string(), synth.patch.glide_ms, 0.0, 300.0),
                    ("drive".to_string(), synth.patch.drive.amount, 0.0, 1.0),
                    ("attack_ms".to_string(), synth.patch.amp_env.attack_ms, 0.5, 1000.0),
                    ("decay_ms".to_string(), synth.patch.amp_env.decay_ms, 10.0, 3000.0),
                    ("sustain".to_string(), synth.patch.amp_env.sustain, 0.0, 1.0),
                    ("release_ms".to_string(), synth.patch.amp_env.release_ms, 10.0, 3000.0),
                ]
            }
            TrackInstrument::Sampler(_) => Vec::new(),
            TrackInstrument::AudioClips(_) => Vec::new(),
            TrackInstrument::Clap(clap) => clap.get_parameters(),
            TrackInstrument::Lv2(lv2) => lv2.get_parameters(),
        }
    }

    /// Export the synth patch of this track to a JSON preset file
    pub fn save_patch_to_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match &self.instrument {
            TrackInstrument::Clap(clap) => {
                let params = clap.get_parameters();
                let json = serde_json::to_string_pretty(&params)?;
                std::fs::write(path, json)?;
                Ok(())
            }
            TrackInstrument::Lv2(lv2) => {
                let params = lv2.get_parameters();
                let json = serde_json::to_string_pretty(&params)?;
                std::fs::write(path, json)?;
                Ok(())
            }
            TrackInstrument::Synth(synth) => {
                let json = synth.patch.to_json()?;
                std::fs::write(path, json)?;
                Ok(())
            }
            _ => anyhow::bail!("Track is not an instrument"),
        }
    }

    /// Load a synth patch from a JSON preset file into this track
    pub fn load_patch_from_file(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let content = std::fs::read_to_string(path)?;
        match &mut self.instrument {
            TrackInstrument::Clap(clap) => {
                // Try parsing as legacy or structured JamSynthPatch JSON
                if let Ok(patch) = jamsynth::JamSynthPatch::from_json(&content) {
                    clap.set_parameter("cutoff", patch.filter.cutoff_hz);
                    clap.set_parameter("resonance", patch.filter.resonance);
                    clap.set_parameter("glide", patch.glide_ms);
                    clap.set_parameter("drive", patch.drive.amount);
                    clap.set_parameter("attack", patch.amp_env.attack_ms);
                    clap.set_parameter("decay", patch.amp_env.decay_ms);
                    clap.set_parameter("sustain", patch.amp_env.sustain);
                    clap.set_parameter("release", patch.amp_env.release_ms);
                    clap.set_parameter("master_gain", patch.master_gain);
                    return Ok(());
                }
                // Try parsing as parameter array [(name, value, min, max)]
                if let Ok(params) = serde_json::from_str::<Vec<(String, f32, f32, f32)>>(&content) {
                    for (name, val, _, _) in params {
                        clap.set_parameter(&name, val);
                    }
                    return Ok(());
                }
                // Try parsing as key-value map
                if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, f32>>(&content) {
                    for (name, val) in map {
                        clap.set_parameter(&name, val);
                    }
                    return Ok(());
                }
                anyhow::bail!("Unrecognized preset format in {}", path.display());
            }
            TrackInstrument::Lv2(lv2) => {
                if let Ok(params) = serde_json::from_str::<Vec<(String, f32, f32, f32)>>(&content) {
                    for (name, val, _, _) in params {
                        lv2.set_parameter(&name, val);
                    }
                    return Ok(());
                }
                if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, f32>>(&content) {
                    for (name, val) in map {
                        lv2.set_parameter(&name, val);
                    }
                    return Ok(());
                }
                anyhow::bail!("Unrecognized preset format in {}", path.display());
            }
            TrackInstrument::Synth(synth) => {
                synth.load_patch_json(&content)?;
                Ok(())
            }
            _ => anyhow::bail!("Track is not an instrument"),
        }
    }

    /// Render track audio into temporary buffers and mix into the master bus.
    pub fn process_block(
        &mut self,
        current_timeline_sample: u64,
        master_left: &mut [f32],
        master_right: &mut [f32],
        scratch_left: &mut [f32],
        scratch_right: &mut [f32],
    ) {
        if self.muted {
            return;
        }

        let num_samples = master_left.len().min(master_right.len());
        scratch_left[..num_samples].fill(0.0);
        scratch_right[..num_samples].fill(0.0);

        match &mut self.instrument {
            TrackInstrument::Synth(synth) => {
                synth.process_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
            }
            TrackInstrument::Sampler(sampler) => {
                sampler.process_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
            }
            TrackInstrument::AudioClips(clips) => {
                clips.process_block(
                    current_timeline_sample,
                    &mut scratch_left[..num_samples],
                    &mut scratch_right[..num_samples],
                );
            }
            TrackInstrument::Clap(clap) => {
                clap.process_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
            }
            TrackInstrument::Lv2(lv2) => {
                lv2.process_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
            }
        }

        // Pass track signal through insert effects chain in series
        for fx in &mut self.inserts {
            match fx {
                InsertEffect::Clap(clap) => {
                    clap.process_effect_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
                }
                InsertEffect::Lv2(lv2) => {
                    lv2.process_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
                }
            }
        }

        let left_gain = self.volume * (1.0 - self.pan).clamp(0.0, 1.0);
        let right_gain = self.volume * (1.0 + self.pan).clamp(0.0, 1.0);

        let mut block_peak = 0.0f32;
        for i in 0..num_samples {
            let l = scratch_left[i] * left_gain;
            let r = scratch_right[i] * right_gain;
            master_left[i] += l;
            master_right[i] += r;

            let s_max = l.abs().max(r.abs());
            if s_max > block_peak {
                block_peak = s_max;
            }
        }

        // Instant attack, exponential decay (~0.85 per audio block)
        if block_peak >= self.peak_level {
            self.peak_level = block_peak;
        } else {
            self.peak_level = (self.peak_level * 0.85).max(0.0);
        }
    }
}
