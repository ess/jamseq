use crate::audio_clip::{AudioClip, AudioClipTrack};
use crate::recorder::{AudioRecorder, RecordedClipInfo};
use crate::timeline::{ScheduledAction, ScheduledEvent, Timeline, TimelineArrangement};
use crate::track::{AuxBus, MasterBus, Track, TrackInstrument};
use anyhow::{Context, Result};
use hound::{WavSpec, WavWriter};
use std::collections::HashMap;
use std::path::Path;

pub struct JamSeqHost {
    pub timeline: Timeline,
    pub tracks: Vec<Track>,
    pub track_map: HashMap<String, usize>,
    pub master_bus: MasterBus,
    pub aux_buses: Vec<AuxBus>,
    pub aux_map: HashMap<String, usize>,
    pub sample_rate: f32,
    pub master_gain: f32,
    pub master_peak: f32,
    pub recorder: AudioRecorder,
    pub project_path: Option<String>,
    pub available_plugins: Vec<crate::plugin_scanner::DiscoveredPlugin>,

    pub aux_left: Vec<Vec<f32>>,
    pub aux_right: Vec<Vec<f32>>,
    scratch_left: Vec<f32>,
    scratch_right: Vec<f32>,
}

impl JamSeqHost {
    pub fn new(bpm: f64, sample_rate: f32) -> Self {
        let sr = if sample_rate > 0.0 { sample_rate } else { 44100.0 };
        let available_plugins = crate::plugin_scanner::PluginScanner::scan_all();
        Self {
            timeline: Timeline::new(bpm, sr as f64),
            tracks: Vec::new(),
            track_map: HashMap::new(),
            master_bus: MasterBus::default(),
            aux_buses: Vec::new(),
            aux_map: HashMap::new(),
            sample_rate: sr,
            master_gain: 0.9,
            master_peak: 0.0,
            recorder: AudioRecorder::new(),
            project_path: None,
            available_plugins,
            aux_left: Vec::new(),
            aux_right: Vec::new(),
            scratch_left: vec![0.0; 2048],
            scratch_right: vec![0.0; 2048],
        }
    }

    pub fn rescan_plugins(&mut self) {
        self.available_plugins = crate::plugin_scanner::PluginScanner::scan_all();
    }

    /// Load a project arrangement JSON from disk
    pub fn load_project_from_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path_ref = path.as_ref();
        let content = std::fs::read_to_string(path_ref)
            .with_context(|| format!("Failed to read project file at {:?}", path_ref))?;
        let arrangement: TimelineArrangement = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse project JSON at {:?}", path_ref))?;
        self.load_arrangement(arrangement);
        self.project_path = Some(path_ref.to_string_lossy().to_string());
        Ok(())
    }

    /// Save the current live arrangement to a project JSON on disk
    pub fn save_project_to_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path_ref = path.as_ref();
        let arrangement = self.timeline.to_arrangement();
        let json_data = serde_json::to_string_pretty(&arrangement)
            .context("Failed to serialize arrangement to JSON")?;
        if let Some(parent) = path_ref.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path_ref, json_data)
            .with_context(|| format!("Failed to write project file to {:?}", path_ref))?;
        self.project_path = Some(path_ref.to_string_lossy().to_string());
        Ok(())
    }

    /// Reset and start a fresh empty project with default sections and tempo
    pub fn new_project(&mut self, title: &str, bpm: f64) {
        let mut arrangement = TimelineArrangement::default();
        arrangement.title = title.to_string();
        arrangement.bpm = if bpm > 10.0 { bpm } else { 120.0 };
        arrangement.total_bars = 8;
        arrangement.sections = vec![
            crate::timeline::SongSection::new("intro", "Intro", 1, 4, 2),
            crate::timeline::SongSection::new("verse_1", "Verse 1", 5, 4, 3),
        ];
        arrangement.events = Vec::new();
        self.load_arrangement(arrangement);
        self.project_path = None;
    }

    pub fn set_sample_rate(&mut self, rate: f32) {
        if rate > 0.0 {
            self.sample_rate = rate;
            self.timeline.sample_rate = rate as f64;
            for t in &mut self.tracks {
                t.set_sample_rate(rate);
            }
        }
    }

    pub fn add_track(&mut self, mut track: Track) {
        track.set_sample_rate(self.sample_rate);
        let idx = self.tracks.len();
        self.track_map.insert(track.id.clone(), idx);
        self.tracks.push(track);
    }

    /// Automatically attaches a newly recorded WAV clip as an audio track and adds timeline event representation
    pub fn attach_recorded_clip(&mut self, clip_info: &RecordedClipInfo) -> String {
        let clip_count = self.tracks.iter().filter(|t| t.id.starts_with("mic_take_") || t.id.starts_with("audio_")).count() + 1;
        let track_id = format!("mic_take_{}", clip_count);
        let track_name = format!("Mic Take {}", clip_count);

        let mut clip_track = AudioClipTrack::new();
        clip_track.add_clip(AudioClip::new(&clip_info.path, clip_info.start_sample));

        let track = Track::new_audio_clips(&track_id, &track_name, clip_track);
        self.add_track(track);

        // Add a visual block event on the timeline spanning the recorded take
        let start_tick = self.timeline.samples_to_ticks(clip_info.start_sample);
        let duration_ticks = self.timeline.samples_to_ticks(clip_info.total_samples).max(240);
        let event_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.timeline.events.push(ScheduledEvent::new(
            event_id,
            start_tick,
            &track_id,
            ScheduledAction::NoteOn { note: 60, velocity: 1.0 },
        ));
        self.timeline.events.push(ScheduledEvent::new(
            event_id + 1,
            start_tick + duration_ticks,
            &track_id,
            ScheduledAction::NoteOff { note: 60 },
        ));
        self.timeline.events.sort_by_key(|e| e.tick);
        self.timeline.seek_tick(self.timeline.current_tick);

        track_id
    }

    /// Manually add a track of the requested type
    /// Manually add a track of the requested type
    pub fn add_custom_track(&mut self, id: &str, name: &str, track_type: &str) {
        let sr = self.sample_rate;
        let track = match track_type {
            "audio" => {
                Track::new_audio_clips(id, name, AudioClipTrack::new())
            }
            _ => {
                if let Some(jamsynth_path) = crate::plugin_scanner::find_jamsynth_plugin() {
                    match crate::clap_host::ClapTrack::load(&jamsynth_path.to_string_lossy(), sr) {
                        Ok(clap_inst) => {
                            apply_synth_preset(&clap_inst, track_type);
                            let display = match track_type {
                                "synth:reese" => "JamSynth (Dark Reese)",
                                "synth:pad" => "JamSynth (Cold Pad)",
                                _ => "JamSynth (Sub 808)",
                            };
                            Track::new_clap(id, name, clap_inst, display)
                        }
                        Err(e) => {
                            eprintln!("[jamseq] Failed to load JamSynth CLAP: {}", e);
                            let mut engine = jamsynth::JamSynthEngine::new(sr);
                            if track_type == "synth:reese" {
                                engine.load_patch(jamsynth::JamSynthPatch::preset_reese_bass());
                            } else if track_type == "synth:pad" {
                                engine.load_patch(jamsynth::JamSynthPatch::preset_cold_pad());
                            } else {
                                engine.load_patch(jamsynth::JamSynthPatch::preset_808_sub());
                            }
                            Track::new_synth(id, name, engine)
                        }
                    }
                } else {
                    let mut engine = jamsynth::JamSynthEngine::new(sr);
                    if track_type == "synth:reese" {
                        engine.load_patch(jamsynth::JamSynthPatch::preset_reese_bass());
                    } else if track_type == "synth:pad" {
                        engine.load_patch(jamsynth::JamSynthPatch::preset_cold_pad());
                    } else {
                        engine.load_patch(jamsynth::JamSynthPatch::preset_808_sub());
                    }
                    Track::new_synth(id, name, engine)
                }
            }
        };
        self.add_track(track);
    }

    /// Rename an existing track
    pub fn rename_track(&mut self, track_id: &str, new_name: &str) {
        if let Some(&idx) = self.track_map.get(track_id) {
            self.tracks[idx].name = new_name.to_string();
        }
    }

    /// Delete a track and clean up track mapping and associated timeline events
    pub fn delete_track(&mut self, track_id: &str) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            self.tracks.remove(idx);
            // Rebuild track_map indices
            self.track_map.clear();
            for (i, t) in self.tracks.iter().enumerate() {
                self.track_map.insert(t.id.clone(), i);
            }
            // Retain timeline events for remaining tracks
            self.timeline.events.retain(|e| e.track_id != track_id);
            self.timeline.seek_tick(self.timeline.current_tick);
            true
        } else {
            false
        }
    }

    /// Reorder a track from index `from` to `to`
    pub fn reorder_tracks(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tracks.len() || to >= self.tracks.len() {
            return false;
        }
        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
        // Rebuild track_map indices
        self.track_map.clear();
        for (i, t) in self.tracks.iter().enumerate() {
            self.track_map.insert(t.id.clone(), i);
        }
        true
    }

    /// Dynamically switch a track's instrument to an internal synth patch or plugin
    pub fn switch_track_instrument(&mut self, track_id: &str, uri_or_type: &str, display_name: &str) {
        if let Some(&idx) = self.track_map.get(track_id) {
            let sr = self.sample_rate;
            let track = &mut self.tracks[idx];

            track.current_plugin = Some(display_name.to_string());

            if uri_or_type.starts_with("synth:") || uri_or_type == "jamsynth" {
                if let Some(jamsynth_path) = crate::plugin_scanner::find_jamsynth_plugin() {
                    match crate::clap_host::ClapTrack::load(&jamsynth_path.to_string_lossy(), sr) {
                        Ok(clap_inst) => {
                            apply_synth_preset(&clap_inst, uri_or_type);
                            println!("[jamseq] ✓ Switched track '{}' to JamSynth CLAP: {}", track_id, display_name);
                            track.instrument = TrackInstrument::Clap(Box::new(clap_inst));
                            return;
                        }
                        Err(e) => {
                            eprintln!("[jamseq] Failed to load JamSynth CLAP: {}", e);
                        }
                    }
                }

                // Fallback to internal synth engine if CLAP binary unavailable
                let mut engine = jamsynth::JamSynthEngine::new(sr);
                if uri_or_type.starts_with("synth:reese") {
                    engine.load_patch(jamsynth::JamSynthPatch::preset_reese_bass());
                } else if uri_or_type.starts_with("synth:pad") {
                    engine.load_patch(jamsynth::JamSynthPatch::preset_cold_pad());
                } else {
                    engine.load_patch(jamsynth::JamSynthPatch::preset_808_sub());
                }
                track.instrument = TrackInstrument::Synth(engine);
            } else if uri_or_type.ends_with(".clap") || uri_or_type.ends_with(".so") {
                match crate::clap_host::ClapTrack::load(uri_or_type, sr) {
                    Ok(clap_inst) => {
                        println!("[jamseq] ✓ Switched track '{}' to CLAP plugin: {}", track_id, display_name);
                        track.instrument = TrackInstrument::Clap(Box::new(clap_inst));
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load CLAP plugin '{}': {}", uri_or_type, e);
                    }
                }
            } else if uri_or_type.starts_with("http://") || uri_or_type.starts_with("https://") || uri_or_type.starts_with("urn:") {
                match crate::lv2_host::Lv2Track::load(uri_or_type, sr) {
                    Ok(lv2_inst) => {
                        println!("[jamseq] ✓ Switched track '{}' to LV2 plugin: {}", track_id, display_name);
                        track.instrument = TrackInstrument::Lv2(Box::new(lv2_inst));
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load LV2 plugin '{}': {}", uri_or_type, e);
                    }
                }
            } else {
                println!("[jamseq] Switched track '{}' to unknown plugin type: {} ({})", track_id, display_name, uri_or_type);
            }
        }
    }

    /// Add an insert effect plugin to a track's signal chain
    pub fn add_track_insert(&mut self, track_id: &str, uri_or_path: &str, display_name: &str) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            let sr = self.sample_rate;
            let track = &mut self.tracks[idx];

            if uri_or_path.ends_with(".clap") || uri_or_path.ends_with(".so") {
                match crate::clap_host::ClapTrack::load(uri_or_path, sr) {
                    Ok(clap_inst) => {
                        println!("[jamseq] ✓ Added CLAP insert effect '{}' to track '{}'", display_name, track_id);
                        track.inserts.push(crate::track::InsertEffect::Clap {
                            name: display_name.to_string(),
                            uri: uri_or_path.to_string(),
                            track: Box::new(clap_inst),
                        });
                        return true;
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load CLAP insert effect '{}': {}", uri_or_path, e);
                        return false;
                    }
                }
            } else if uri_or_path.starts_with("http://") || uri_or_path.starts_with("https://") || uri_or_path.starts_with("urn:") {
                match crate::lv2_host::Lv2Track::load(uri_or_path, sr) {
                    Ok(lv2_inst) => {
                        println!("[jamseq] ✓ Added LV2 insert effect '{}' to track '{}'", display_name, track_id);
                        track.inserts.push(crate::track::InsertEffect::Lv2 {
                            name: display_name.to_string(),
                            uri: uri_or_path.to_string(),
                            track: Box::new(lv2_inst),
                        });
                        return true;
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load LV2 insert effect '{}': {}", uri_or_path, e);
                        return false;
                    }
                }
            }
        }
        false
    }

    /// Remove an insert effect from a track by index in chain
    pub fn remove_track_insert(&mut self, track_id: &str, insert_index: usize) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            let track = &mut self.tracks[idx];
            if insert_index < track.inserts.len() {
                track.inserts.remove(insert_index);
                return true;
            }
        }
        false
    }

    /// Add an insert effect to the Master Bus
    pub fn add_master_insert(&mut self, uri_or_path: &str, display_name: &str) -> bool {
        let sr = self.sample_rate;
        if uri_or_path.ends_with(".clap") || uri_or_path.ends_with(".so") {
            match crate::clap_host::ClapTrack::load(uri_or_path, sr) {
                Ok(clap_inst) => {
                    println!("[jamseq] ✓ Added CLAP Master insert '{}'", display_name);
                    self.master_bus.inserts.push(crate::track::InsertEffect::Clap {
                        name: display_name.to_string(),
                        uri: uri_or_path.to_string(),
                        track: Box::new(clap_inst),
                    });
                    return true;
                }
                Err(e) => {
                    eprintln!("[jamseq] Failed to load CLAP master insert '{}': {}", uri_or_path, e);
                    return false;
                }
            }
        } else if uri_or_path.starts_with("http://") || uri_or_path.starts_with("https://") || uri_or_path.starts_with("urn:") {
            match crate::lv2_host::Lv2Track::load(uri_or_path, sr) {
                Ok(lv2_inst) => {
                    println!("[jamseq] ✓ Added LV2 Master insert '{}'", display_name);
                    self.master_bus.inserts.push(crate::track::InsertEffect::Lv2 {
                        name: display_name.to_string(),
                        uri: uri_or_path.to_string(),
                        track: Box::new(lv2_inst),
                    });
                    return true;
                }
                Err(e) => {
                    eprintln!("[jamseq] Failed to load LV2 master insert '{}': {}", uri_or_path, e);
                    return false;
                }
            }
        }
        false
    }

    /// Remove an insert effect from the Master Bus
    pub fn remove_master_insert(&mut self, insert_index: usize) -> bool {
        if insert_index < self.master_bus.inserts.len() {
            self.master_bus.inserts.remove(insert_index);
            return true;
        }
        false
    }

    /// Add an Aux Bus
    pub fn add_aux_bus(&mut self, id: &str, name: &str) {
        if !self.aux_map.contains_key(id) {
            let idx = self.aux_buses.len();
            self.aux_buses.push(crate::track::AuxBus::new(id, name));
            self.aux_map.insert(id.to_string(), idx);
            self.aux_left.push(vec![0.0; 2048]);
            self.aux_right.push(vec![0.0; 2048]);
            println!("[jamseq] ✓ Created Aux Bus '{}' ({})", name, id);
        }
    }

    /// Remove an Aux Bus
    pub fn remove_aux_bus(&mut self, id: &str) {
        if let Some(idx) = self.aux_map.remove(id) {
            self.aux_buses.remove(idx);
            self.aux_left.remove(idx);
            self.aux_right.remove(idx);
            self.aux_map.clear();
            for (i, b) in self.aux_buses.iter().enumerate() {
                self.aux_map.insert(b.id.clone(), i);
            }
            for t in &mut self.tracks {
                t.remove_aux_send(id);
            }
            println!("[jamseq] Removed Aux Bus '{}'", id);
        }
    }

    /// Add an insert effect to an Aux Bus
    pub fn add_aux_insert(&mut self, bus_id: &str, uri_or_path: &str, display_name: &str) -> bool {
        if let Some(&idx) = self.aux_map.get(bus_id) {
            let sr = self.sample_rate;
            let bus = &mut self.aux_buses[idx];
            if uri_or_path.ends_with(".clap") || uri_or_path.ends_with(".so") {
                match crate::clap_host::ClapTrack::load(uri_or_path, sr) {
                    Ok(clap_inst) => {
                        println!("[jamseq] ✓ Added CLAP insert '{}' to Aux Bus '{}'", display_name, bus_id);
                        bus.inserts.push(crate::track::InsertEffect::Clap {
                            name: display_name.to_string(),
                            uri: uri_or_path.to_string(),
                            track: Box::new(clap_inst),
                        });
                        return true;
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load CLAP aux insert '{}': {}", uri_or_path, e);
                        return false;
                    }
                }
            } else if uri_or_path.starts_with("http://") || uri_or_path.starts_with("https://") || uri_or_path.starts_with("urn:") {
                match crate::lv2_host::Lv2Track::load(uri_or_path, sr) {
                    Ok(lv2_inst) => {
                        println!("[jamseq] ✓ Added LV2 insert '{}' to Aux Bus '{}'", display_name, bus_id);
                        bus.inserts.push(crate::track::InsertEffect::Lv2 {
                            name: display_name.to_string(),
                            uri: uri_or_path.to_string(),
                            track: Box::new(lv2_inst),
                        });
                        return true;
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load LV2 aux insert '{}': {}", uri_or_path, e);
                        return false;
                    }
                }
            }
        }
        false
    }

    /// Remove an insert effect from an Aux Bus
    pub fn remove_aux_insert(&mut self, bus_id: &str, insert_index: usize) -> bool {
        if let Some(&idx) = self.aux_map.get(bus_id) {
            let bus = &mut self.aux_buses[idx];
            if insert_index < bus.inserts.len() {
                bus.inserts.remove(insert_index);
                return true;
            }
        }
        false
    }

    /// Configure a track's aux send level
    pub fn set_track_aux_send(&mut self, track_id: &str, bus_id: &str, gain: f32, is_pre_fader: bool) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            self.tracks[idx].set_aux_send(bus_id, gain, is_pre_fader);
            return true;
        }
        false
    }

    /// Remove a track's aux send
    pub fn remove_track_aux_send(&mut self, track_id: &str, bus_id: &str) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            self.tracks[idx].remove_aux_send(bus_id);
            return true;
        }
        false
    }

    pub fn load_arrangement(&mut self, arrangement: TimelineArrangement) {
        self.timeline.load_arrangement(arrangement);
    }

    pub fn play(&mut self) {
        self.timeline.play();
    }

    pub fn pause(&mut self) {
        self.timeline.pause();
        for t in &mut self.tracks {
            t.all_notes_off();
        }
    }

    /// Render a block of stereo audio into master_left and master_right.
    pub fn process_block(&mut self, master_left: &mut [f32], master_right: &mut [f32]) {
        let num_samples = master_left.len().min(master_right.len());
        master_left[..num_samples].fill(0.0);
        master_right[..num_samples].fill(0.0);

        if self.scratch_left.len() < num_samples {
            self.scratch_left.resize(num_samples, 0.0);
            self.scratch_right.resize(num_samples, 0.0);
        }

        // 1. Advance timeline and fetch due events
        let due_events = self.timeline.advance_samples(num_samples);

        // 2. Dispatch events to matching tracks
        for ev in due_events {
            if let Some(&track_idx) = self.track_map.get(&ev.track_id) {
                let track = &mut self.tracks[track_idx];
                match ev.action {
                    ScheduledAction::NoteOn { note, velocity } => {
                        track.note_on(note, velocity);
                    }
                    ScheduledAction::NoteOff { note } => {
                        track.note_off(note);
                    }
                    ScheduledAction::ControlChange { cc, value } => {
                        track.handle_cc(cc, value);
                    }
                    ScheduledAction::SetParam { param_name, value } => {
                        track.set_parameter(&param_name, value);
                    }
                    ScheduledAction::LoadPatchJson { json } => {
                        match &mut track.instrument {
                            crate::track::TrackInstrument::Clap(clap) => {
                                if let Ok(patch) = jamsynth::JamSynthPatch::from_json(&json) {
                                    clap.set_parameter("cutoff", patch.filter.cutoff_hz);
                                    clap.set_parameter("resonance", patch.filter.resonance);
                                    clap.set_parameter("glide", patch.glide_ms);
                                    clap.set_parameter("drive", patch.drive.amount);
                                    clap.set_parameter("attack", patch.amp_env.attack_ms);
                                    clap.set_parameter("decay", patch.amp_env.decay_ms);
                                    clap.set_parameter("sustain", patch.amp_env.sustain);
                                    clap.set_parameter("release", patch.amp_env.release_ms);
                                    clap.set_parameter("master_gain", patch.master_gain);
                                }
                            }
                            crate::track::TrackInstrument::Synth(synth) => {
                                let _ = synth.load_patch_json(&json);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let current_sample = self.timeline.current_sample;
        let any_solo = self.tracks.iter().any(|t| t.solo);

        // 3. Prepare aux buffers
        for l in &mut self.aux_left {
            if l.len() < num_samples {
                l.resize(num_samples, 0.0);
            }
            l[..num_samples].fill(0.0);
        }
        for r in &mut self.aux_right {
            if r.len() < num_samples {
                r.resize(num_samples, 0.0);
            }
            r[..num_samples].fill(0.0);
        }

        // Render all tracks into master bus and tap aux sends
        for track in &mut self.tracks {
            if any_solo && !track.solo {
                continue;
            }
            track.process_block(
                current_sample,
                &mut master_left[..num_samples],
                &mut master_right[..num_samples],
                &mut self.scratch_left[..num_samples],
                &mut self.scratch_right[..num_samples],
            );

            // Tap aux sends
            if !track.muted && !track.sends.is_empty() {
                let left_gain = track.volume * (1.0 - track.pan).clamp(0.0, 1.0);
                let right_gain = track.volume * (1.0 + track.pan).clamp(0.0, 1.0);
                for send in &track.sends {
                    if !send.is_muted && send.gain > 0.0 {
                        if let Some(&bus_idx) = self.aux_map.get(&send.target_bus_id) {
                            let bus_l = &mut self.aux_left[bus_idx][..num_samples];
                            let bus_r = &mut self.aux_right[bus_idx][..num_samples];
                            if send.is_pre_fader {
                                for i in 0..num_samples {
                                    bus_l[i] += self.scratch_left[i] * send.gain;
                                    bus_r[i] += self.scratch_right[i] * send.gain;
                                }
                            } else {
                                for i in 0..num_samples {
                                    bus_l[i] += self.scratch_left[i] * left_gain * send.gain;
                                    bus_r[i] += self.scratch_right[i] * right_gain * send.gain;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Process Aux Return Buses through their inserts and sum into Master
        for (idx, bus) in self.aux_buses.iter_mut().enumerate() {
            if bus.muted {
                continue;
            }
            let bus_l = &mut self.aux_left[idx][..num_samples];
            let bus_r = &mut self.aux_right[idx][..num_samples];

            // Pass through Aux Bus inserts in series
            for fx in &mut bus.inserts {
                match fx {
                    crate::track::InsertEffect::Clap { track: clap, .. } => {
                        clap.process_effect_block(bus_l, bus_r);
                    }
                    crate::track::InsertEffect::Lv2 { track: lv2, .. } => {
                        lv2.process_block(bus_l, bus_r);
                    }
                }
            }

            let left_gain = bus.volume * (1.0 - bus.pan).clamp(0.0, 1.0);
            let right_gain = bus.volume * (1.0 + bus.pan).clamp(0.0, 1.0);

            let mut bus_peak = 0.0f32;
            for i in 0..num_samples {
                let l = bus_l[i] * left_gain;
                let r = bus_r[i] * right_gain;
                master_left[i] += l;
                master_right[i] += r;

                let s = l.abs().max(r.abs());
                if s > bus_peak {
                    bus_peak = s;
                }
            }

            if bus_peak >= bus.peak_level {
                bus.peak_level = bus_peak;
            } else {
                bus.peak_level = (bus.peak_level * 0.85).max(0.0);
            }
        }

        // 4. Process Master Bus inserts in series
        if !self.master_bus.muted {
            for fx in &mut self.master_bus.inserts {
                match fx {
                    crate::track::InsertEffect::Clap { track: clap, .. } => {
                        clap.process_effect_block(&mut master_left[..num_samples], &mut master_right[..num_samples]);
                    }
                    crate::track::InsertEffect::Lv2 { track: lv2, .. } => {
                        lv2.process_block(&mut master_left[..num_samples], &mut master_right[..num_samples]);
                    }
                }
            }
        }

        // 5. Master volume, pan, and soft limiter to prevent digital clipping
        let master_l_gain = self.master_gain * self.master_bus.volume * (1.0 - self.master_bus.pan).clamp(0.0, 1.0);
        let master_r_gain = self.master_gain * self.master_bus.volume * (1.0 + self.master_bus.pan).clamp(0.0, 1.0);

        let mut block_peak = 0.0f32;
        for i in 0..num_samples {
            let l = if self.master_bus.muted { 0.0 } else { (master_left[i] * master_l_gain).tanh() };
            let r = if self.master_bus.muted { 0.0 } else { (master_right[i] * master_r_gain).tanh() };
            master_left[i] = l;
            master_right[i] = r;

            let s_max = l.abs().max(r.abs());
            if s_max > block_peak {
                block_peak = s_max;
            }
        }

        if block_peak >= self.master_peak {
            self.master_peak = block_peak;
        } else {
            self.master_peak = (self.master_peak * 0.85).max(0.0);
        }
        self.master_bus.peak_level = self.master_peak;
    }

    /// Offline render the entire song timeline directly to a WAV file.
    pub fn render_to_wav(&mut self, out_path: &Path, num_bars: u32) -> Result<()> {
        let spec = WavSpec {
            channels: 2,
            sample_rate: self.sample_rate as u32,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = WavWriter::create(out_path, spec)
            .with_context(|| format!("Failed to create output WAV at {:?}", out_path))?;

        let total_ticks = num_bars as u64 * 4 * crate::timeline::PPQ;
        let total_samples = self.timeline.ticks_to_samples(total_ticks);

        self.timeline.seek_tick(0);
        self.timeline.loop_enabled = false;
        self.timeline.play();

        let block_size = 512;
        let mut left = vec![0.0f32; block_size];
        let mut right = vec![0.0f32; block_size];
        let mut rendered_samples = 0;

        while rendered_samples < total_samples {
            let current_block = ((total_samples - rendered_samples) as usize).min(block_size);
            self.process_block(&mut left[..current_block], &mut right[..current_block]);

            for i in 0..current_block {
                let sample_i24_l = (left[i] * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
                let sample_i24_r = (right[i] * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
                writer.write_sample(sample_i24_l)?;
                writer.write_sample(sample_i24_r)?;
            }

            rendered_samples += current_block as u64;
        }

        writer.finalize()?;
        self.timeline.pause();
        Ok(())
    }
}

fn apply_synth_preset(clap: &crate::clap_host::ClapTrack, preset: &str) {
    if preset.starts_with("synth:reese") {
        clap.set_parameter("cutoff", 480.0);
        clap.set_parameter("resonance", 0.3);
        clap.set_parameter("glide", 60.0);
        clap.set_parameter("drive", 0.45);
        clap.set_parameter("attack", 15.0);
        clap.set_parameter("decay", 2000.0);
        clap.set_parameter("sustain", 0.8);
        clap.set_parameter("release", 200.0);
        clap.set_parameter("master_gain", 0.85);
    } else if preset.starts_with("synth:pad") {
        clap.set_parameter("cutoff", 1200.0);
        clap.set_parameter("resonance", 0.15);
        clap.set_parameter("glide", 0.0);
        clap.set_parameter("drive", 0.15);
        clap.set_parameter("attack", 350.0);
        clap.set_parameter("decay", 3000.0);
        clap.set_parameter("sustain", 0.7);
        clap.set_parameter("release", 1200.0);
        clap.set_parameter("master_gain", 0.8);
    } else {
        // Default 808 sub
        clap.set_parameter("cutoff", 350.0);
        clap.set_parameter("resonance", 0.2);
        clap.set_parameter("glide", 45.0);
        clap.set_parameter("drive", 0.35);
        clap.set_parameter("attack", 1.0);
        clap.set_parameter("decay", 1400.0);
        clap.set_parameter("sustain", 0.0);
        clap.set_parameter("release", 100.0);
        clap.set_parameter("master_gain", 0.9);
    }
}
