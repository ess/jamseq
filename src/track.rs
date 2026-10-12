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
    Clap { name: String, uri: String, track: Box<ClapTrack> },
    Lv2 { name: String, uri: String, track: Box<Lv2Track> },
}

impl InsertEffect {
    pub fn name(&self) -> &str {
        match self {
            InsertEffect::Clap { name, .. } => name,
            InsertEffect::Lv2 { name, .. } => name,
        }
    }

    pub fn uri(&self) -> &str {
        match self {
            InsertEffect::Clap { uri, .. } => uri,
            InsertEffect::Lv2 { uri, .. } => uri,
        }
    }

    pub fn get_parameters(&self) -> Vec<(String, f32, f32, f32)> {
        match self {
            InsertEffect::Clap { track, .. } => track.get_parameters(),
            InsertEffect::Lv2 { track, .. } => track.get_parameters(),
        }
    }

    pub fn set_parameter(&mut self, param_name: &str, value: f32) {
        match self {
            InsertEffect::Clap { track, .. } => track.set_parameter(param_name, value),
            InsertEffect::Lv2 { track, .. } => track.set_parameter(param_name, value),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SidechainSend {
    pub target_track_id: String,
    pub target_insert_index: usize,
    pub gain: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuxSend {
    pub target_bus_id: String,
    pub gain: f32,          // Send gain (0.0 to 1.0+)
    pub is_pre_fader: bool, // false = post-fader (default)
    pub is_muted: bool,
}

pub struct AuxBus {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub inserts: Vec<InsertEffect>,
    pub peak_level: f32,
}

impl AuxBus {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            volume: 1.0,
            pan: 0.0,
            muted: false,
            solo: false,
            inserts: Vec::new(),
            peak_level: 0.0,
        }
    }
}

pub struct MasterBus {
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub inserts: Vec<InsertEffect>,
    pub peak_level: f32,
}

impl Default for MasterBus {
    fn default() -> Self {
        Self {
            volume: 0.9,
            pan: 0.0,
            muted: false,
            inserts: Vec::new(),
            peak_level: 0.0,
        }
    }
}

pub struct Track {
    pub id: String,
    pub name: String,
    pub instrument: TrackInstrument,
    pub inserts: Vec<InsertEffect>,
    pub sends: Vec<AuxSend>,
    pub sidechains: Vec<SidechainSend>,
    pub muted: bool,
    pub solo: bool,
    pub is_frozen: bool,
    pub cached_stem: Option<crate::audio_clip::AudioClip>,
    pub stem_dirty: bool,
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
            sends: Vec::new(),
            sidechains: Vec::new(),
            muted: false,
            solo: false,
            is_frozen: false,
            cached_stem: None,
            stem_dirty: true,
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
            sends: Vec::new(),
            sidechains: Vec::new(),
            muted: false,
            solo: false,
            is_frozen: false,
            cached_stem: None,
            stem_dirty: true,
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
            sends: Vec::new(),
            sidechains: Vec::new(),
            muted: false,
            solo: false,
            is_frozen: false,
            cached_stem: None,
            stem_dirty: true,
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
            sends: Vec::new(),
            sidechains: Vec::new(),
            muted: false,
            solo: false,
            is_frozen: false,
            cached_stem: None,
            stem_dirty: true,
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
            sends: Vec::new(),
            sidechains: Vec::new(),
            muted: false,
            solo: false,
            is_frozen: false,
            cached_stem: None,
            stem_dirty: false,
            volume: 1.0,
            pan: 0.0,
            current_plugin: None,
            peak_level: 0.0,
        }
    }

    pub fn set_aux_send(&mut self, bus_id: &str, gain: f32, is_pre_fader: bool) {
        if let Some(send) = self.sends.iter_mut().find(|s| s.target_bus_id == bus_id) {
            send.gain = gain;
            send.is_pre_fader = is_pre_fader;
        } else {
            self.sends.push(AuxSend {
                target_bus_id: bus_id.to_string(),
                gain,
                is_pre_fader,
                is_muted: false,
            });
        }
    }

    pub fn remove_aux_send(&mut self, bus_id: &str) {
        self.sends.retain(|s| s.target_bus_id != bus_id);
    }

    pub fn set_sidechain(&mut self, target_track_id: &str, target_insert_index: usize, gain: f32) {
        if let Some(sc) = self.sidechains.iter_mut().find(|s| s.target_track_id == target_track_id && s.target_insert_index == target_insert_index) {
            sc.gain = gain;
        } else {
            self.sidechains.push(SidechainSend {
                target_track_id: target_track_id.to_string(),
                target_insert_index,
                gain,
            });
        }
    }

    pub fn remove_sidechain(&mut self, target_track_id: &str, target_insert_index: usize) {
        self.sidechains.retain(|s| !(s.target_track_id == target_track_id && s.target_insert_index == target_insert_index));
    }

    pub fn mark_stem_dirty(&mut self) {
        self.cached_stem = None;
        self.stem_dirty = true;
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
        if self.muted || self.is_frozen {
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
        if self.is_frozen {
            return;
        }
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => synth.note_off(note),
            TrackInstrument::Sampler(sampler) => sampler.note_off(note),
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(clap) => clap.note_off(note),
            TrackInstrument::Lv2(lv2) => lv2.note_off(note),
        }
    }

    pub fn all_notes_off(&mut self) {
        if self.is_frozen {
            return;
        }
        match &mut self.instrument {
            TrackInstrument::Synth(synth) => synth.all_notes_off(),
            TrackInstrument::Sampler(sampler) => sampler.all_notes_off(),
            TrackInstrument::AudioClips(_) => {}
            TrackInstrument::Clap(clap) => clap.all_notes_off(),
            TrackInstrument::Lv2(lv2) => lv2.all_notes_off(),
        }
    }

    pub fn handle_cc(&mut self, cc: u8, value: u8) {
        if self.is_frozen {
            return;
        }
        if let TrackInstrument::Synth(synth) = &mut self.instrument {
            synth.handle_cc(cc, value);
        }
    }

    /// Set a named DSP / engine parameter live
    pub fn set_parameter(&mut self, param_name: &str, value: f32) {
        self.mark_stem_dirty();
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
        self.mark_stem_dirty();
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
        sidechain_buffers: Option<&std::collections::HashMap<(String, usize), (Vec<f32>, Vec<f32>)>>,
        bypass_cached_stem: bool,
    ) {
        if self.muted {
            return;
        }

        let num_samples = master_left.len().min(master_right.len());
        scratch_left[..num_samples].fill(0.0);
        scratch_right[..num_samples].fill(0.0);

        // If a pre-rendered stem is available and not bypassed, stream directly from the cached stem!
        let mut played_from_stem = false;
        if !bypass_cached_stem && !self.is_frozen {
            if let Some(ref stem) = self.cached_stem {
                if let Some(ref buf) = stem.buffer {
                    let clip_len = buf.samples_left.len() as u64;
                    let clip_end = stem.start_sample + clip_len;
                    let block_end = current_timeline_sample + num_samples as u64;

                    if block_end > stem.start_sample && current_timeline_sample < clip_end {
                        for i in 0..num_samples {
                            let sample_pos = current_timeline_sample + i as u64;
                            if sample_pos >= stem.start_sample && sample_pos < clip_end {
                                let offset = (sample_pos - stem.start_sample) as usize;
                                if offset < buf.samples_left.len() {
                                    scratch_left[i] += buf.samples_left[offset];
                                    scratch_right[i] += buf.samples_right[offset];
                                }
                            }
                        }
                    }
                    played_from_stem = true;
                }
            }
        }

        if !played_from_stem {
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
            for (ins_idx, fx) in self.inserts.iter_mut().enumerate() {
                match fx {
                    InsertEffect::Clap { track: clap, .. } => {
                        clap.process_effect_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
                    }
                    InsertEffect::Lv2 { track: lv2, .. } => {
                        lv2.process_block(&mut scratch_left[..num_samples], &mut scratch_right[..num_samples]);
                    }
                }

                // Apply dynamic sidechain ducking if this insert has an incoming sidechain feed
                if let Some(sc_map) = sidechain_buffers {
                    if let Some((sc_l, sc_r)) = sc_map.get(&(self.id.clone(), ins_idx)) {
                        let mut env = 0.0f32;
                        for i in 0..num_samples {
                            let sc_mag = sc_l[i].abs().max(sc_r[i].abs());
                            if sc_mag > env {
                                env = sc_mag;
                            } else {
                                env *= 0.999;
                            }
                            let duck = (1.0 - (env * 0.85)).clamp(0.05, 1.0);
                            scratch_left[i] *= duck;
                            scratch_right[i] *= duck;
                        }
                    }
                }
            }

            // Native track ducking on slot 0 if track has no external inserts
            if self.inserts.is_empty() {
                if let Some(sc_map) = sidechain_buffers {
                    if let Some((sc_l, sc_r)) = sc_map.get(&(self.id.clone(), 0)) {
                        let mut env = 0.0f32;
                        for i in 0..num_samples {
                            let sc_mag = sc_l[i].abs().max(sc_r[i].abs());
                            if sc_mag > env {
                                env = sc_mag;
                            } else {
                                env *= 0.999;
                            }
                            let duck = (1.0 - (env * 0.85)).clamp(0.05, 1.0);
                            scratch_left[i] *= duck;
                            scratch_right[i] *= duck;
                        }
                    }
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
