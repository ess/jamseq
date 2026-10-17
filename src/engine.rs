use crate::audio_clip::{AudioClip, AudioClipTrack};
use crate::recorder::{AudioRecorder, RecordedClipInfo};
use crate::timeline::{ScheduledAction, ScheduledEvent, Timeline};
use crate::track::{AuxBus, MasterBus, Track, TrackInstrument};
use anyhow::{Context, Result};
use hound::{WavSpec, WavWriter};
use std::collections::HashMap;
use std::path::Path;

pub struct FrozenTrackState {
    pub instrument: TrackInstrument,
    pub inserts: Vec<crate::track::InsertEffect>,
    pub wav_path: String,
}

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
    pub midi_input: crate::midi_input::MidiInputManager,
    pub armed_track_id: Option<String>,
    pub project_path: Option<String>,
    pub available_plugins: Vec<crate::plugin_scanner::DiscoveredPlugin>,
    pub frozen_tracks: HashMap<String, FrozenTrackState>,
    pub sidechain_buffers: HashMap<(String, usize), (Vec<f32>, Vec<f32>)>,

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
            midi_input: crate::midi_input::MidiInputManager::new(),
            armed_track_id: None,
            project_path: None,
            available_plugins,
            frozen_tracks: HashMap::new(),
            sidechain_buffers: HashMap::new(),
            aux_left: Vec::new(),
            aux_right: Vec::new(),
            scratch_left: vec![0.0; 2048],
            scratch_right: vec![0.0; 2048],
        }
    }

    pub fn rescan_plugins(&mut self) {
        self.available_plugins = crate::plugin_scanner::PluginScanner::scan_all();
    }

    pub fn add_section(&mut self, section: crate::timeline::SongSection) {
        let block_refs: Vec<(String, String)> = section.track_blocks.iter()
            .map(|(t_id, b)| (t_id.clone(), b.id.clone()))
            .collect();
        self.timeline.add_section(section);
        for (track_id, block_id) in block_refs {
            self.rebake_block(&track_id, &block_id);
        }
    }

    pub fn remove_section(&mut self, section_id: &str) -> bool {
        let block_ids: Vec<(String, String)> = self.timeline.sections.iter()
            .find(|s| s.id == section_id)
            .map(|s| s.track_blocks.iter().map(|(t_id, b)| (t_id.clone(), b.id.clone())).collect())
            .unwrap_or_default();
        let res = self.timeline.remove_section(section_id);
        if res {
            for (track_id, block_id) in block_ids {
                if let Some(&idx) = self.track_map.get(&track_id) {
                    self.tracks[idx].active_block_stems.retain(|bs| bs.block_id != block_id);
                }
            }
        }
        res
    }

    pub fn update_section(&mut self, section: crate::timeline::SongSection) -> bool {
        let block_refs: Vec<(String, String)> = section.track_blocks.iter()
            .map(|(t_id, b)| (t_id.clone(), b.id.clone()))
            .collect();
        let res = self.timeline.update_section(section);
        if res {
            for (track_id, block_id) in block_refs {
                self.rebake_block(&track_id, &block_id);
            }
        }
        res
    }

    pub fn add_block(
        &mut self,
        track_id: &str,
        suggested_block_id: Option<&str>,
        section_id: Option<&str>,
        start_tick: Option<u64>,
        length_ticks: Option<u64>,
    ) -> Option<crate::timeline::TrackBlock> {
        let (block, block_id) = if let Some(sec_id) = section_id {
            let sec_len = self.timeline.sections.iter()
                .find(|s| s.id == sec_id)
                .map(|s| s.length_ticks)
                .unwrap_or(3840);
            let len = length_ticks.unwrap_or(sec_len);
            let block_id = suggested_block_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{}_{}", sec_id, track_id));
            let mut block = crate::timeline::TrackBlock::new(&block_id, track_id, len);
            block.section_id = Some(sec_id.to_string());
            if let Some(sec) = self.timeline.sections.iter_mut().find(|s| s.id == sec_id) {
                sec.track_blocks.insert(track_id.to_string(), block.clone());
            } else {
                return None;
            }
            (block, block_id)
        } else {
            let start = start_tick.unwrap_or(0);
            let len = length_ticks.unwrap_or(3840);
            let block_id = suggested_block_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    format!("{}_clip_{}_{}", track_id, start, now % 100000)
                });
            let block = crate::timeline::TrackBlock::new_timeline(&block_id, track_id, start, len);
            self.timeline.timeline_blocks.push(block.clone());
            (block, block_id)
        };

        self.timeline.rebuild_active_events();
        self.rebake_block(track_id, &block_id);
        Some(block)
    }

    pub fn update_block(&mut self, mut block: crate::timeline::TrackBlock) -> bool {
        let track_id = block.track_id.clone();
        let block_id = block.id.clone();

        let mut updated = false;
        if let Some(sec_id) = &block.section_id {
            if let Some(sec) = self.timeline.sections.iter_mut().find(|s| &s.id == sec_id) {
                sec.track_blocks.insert(track_id.clone(), block.clone());
                updated = true;
            }
        }

        if !updated {
            // Check if it's in timeline_blocks
            if let Some(pos) = self.timeline.timeline_blocks.iter().position(|b| b.id == block_id && b.track_id == track_id) {
                block.section_id = None;
                self.timeline.timeline_blocks[pos] = block.clone();
                updated = true;
            }
        }

        if !updated {
            // Fallback: search sections by block_id
            for sec in &mut self.timeline.sections {
                if let Some(b) = sec.track_blocks.get_mut(&track_id) {
                    if b.id == block_id {
                        block.section_id = Some(sec.id.clone());
                        *b = block.clone();
                        updated = true;
                        break;
                    }
                }
            }
        }

        if updated {
            self.timeline.rebuild_active_events();
            self.rebake_block(&track_id, &block_id);
        }
        updated
    }

    pub fn remove_block(&mut self, track_id: &str, block_id: &str, section_id: Option<&str>) -> bool {
        let mut removed = false;
        if let Some(sec_id) = section_id {
            if let Some(sec) = self.timeline.sections.iter_mut().find(|s| s.id == sec_id) {
                removed = sec.track_blocks.remove(track_id).is_some();
            }
        } else {
            let orig_len = self.timeline.timeline_blocks.len();
            self.timeline.timeline_blocks.retain(|b| !(b.id == block_id && b.track_id == track_id));
            removed = self.timeline.timeline_blocks.len() < orig_len;
            if !removed {
                // Also check sections if section_id was not explicitly provided
                for sec in &mut self.timeline.sections {
                    if let Some(b) = sec.track_blocks.get(track_id) {
                        if b.id == block_id {
                            sec.track_blocks.remove(track_id);
                            removed = true;
                            break;
                        }
                    }
                }
            }
        }

        if removed {
            if let Some(&idx) = self.track_map.get(track_id) {
                self.tracks[idx].active_block_stems.retain(|bs| bs.block_id != block_id);
            }
            self.timeline.rebuild_active_events();
        }
        removed
    }

    pub fn set_track_block(&mut self, section_id: &str, mut block: crate::timeline::TrackBlock) -> bool {
        block.section_id = Some(section_id.to_string());
        let track_id = block.track_id.clone();
        let block_id = block.id.clone();
        let res = self.timeline.set_track_block(section_id, block);
        if res {
            self.rebake_block(&track_id, &block_id);
        }
        res
    }

    pub fn remove_track_block(&mut self, section_id: &str, track_id: &str) -> bool {
        let maybe_block_id = self.timeline.sections.iter()
            .find(|s| s.id == section_id)
            .and_then(|s| s.track_blocks.get(track_id).map(|b| b.id.clone()))
            .unwrap_or_default();
        self.remove_block(track_id, &maybe_block_id, Some(section_id))
    }

    pub fn set_timeline_block(&mut self, mut block: crate::timeline::TrackBlock) -> bool {
        block.section_id = None;
        let track_id = block.track_id.clone();
        let block_id = block.id.clone();
        let res = self.timeline.set_timeline_block(block);
        if res {
            self.rebake_block(&track_id, &block_id);
        }
        res
    }

    pub fn remove_timeline_block(&mut self, block_id: &str) -> bool {
        let maybe_track_id = self.timeline.get_timeline_block(block_id).map(|b| b.track_id.clone()).unwrap_or_default();
        self.remove_block(&maybe_track_id, block_id, None)
    }

    pub fn set_track_arranger_mode(&mut self, track_id: &str, mode: crate::timeline::TrackArrangerMode) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            self.tracks[idx].arranger_mode = mode;
            true
        } else {
            false
        }
    }

    pub fn set_section_tempo(&mut self, section_id: &str, tempo: Option<crate::timeline::TempoOverride>) -> bool {
        self.timeline.set_section_tempo(section_id, tempo)
    }

    pub fn set_block_tempo(&mut self, section_id: &str, track_id: &str, tempo: Option<crate::timeline::TempoOverride>) -> bool {
        let maybe_block_id = self.timeline.sections.iter()
            .find(|s| s.id == section_id)
            .and_then(|s| s.track_blocks.get(track_id).map(|b| b.id.clone()));
        let res = self.timeline.set_block_tempo(section_id, track_id, tempo);
        if res {
            if let Some(block_id) = maybe_block_id {
                self.rebake_block(track_id, &block_id);
            }
        }
        res
    }

    pub fn set_project_time_signature(&mut self, time_signature: crate::timeline::TimeSignature) {
        self.timeline.set_project_time_signature(time_signature);
    }

    pub fn set_section_time_signature(&mut self, section_id: &str, time_signature: Option<crate::timeline::TimeSignature>) -> bool {
        self.timeline.set_section_time_signature(section_id, time_signature)
    }

    pub fn set_block_time_signature(&mut self, section_id: &str, track_id: &str, time_signature: Option<crate::timeline::TimeSignature>) -> bool {
        let maybe_block_id = self.timeline.sections.iter()
            .find(|s| s.id == section_id)
            .and_then(|s| s.track_blocks.get(track_id).map(|b| b.id.clone()));
        let res = self.timeline.set_block_time_signature(section_id, track_id, time_signature);
        if res {
            if let Some(block_id) = maybe_block_id {
                self.rebake_block(track_id, &block_id);
            }
        }
        res
    }

    pub fn set_sample_rate(&mut self, rate: f32) {
        if rate > 0.0 && (rate - self.sample_rate).abs() > 0.01 {
            self.sample_rate = rate;
            self.timeline.sample_rate = rate as f64;
            for t in &mut self.tracks {
                t.set_sample_rate(rate);
            }
            self.rebake_all_blocks();
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
            "sampler" => {
                Track::new_sampler(id, name, crate::sampler::SamplerInstrument::new(crate::sampler::SamplerConfig::default(), sr))
            }
            _ if track_type.starts_with("http://") || track_type.starts_with("https://") || track_type.starts_with("urn:") => {
                match crate::lv2_host::Lv2Track::load(track_type, sr) {
                    Ok(lv2_inst) => Track::new_lv2(id, name, lv2_inst, name),
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load LV2 track '{}': {}", track_type, e);
                        Track::new_synth(id, name, jamsynth::JamSynthEngine::new(sr))
                    }
                }
            }
            _ if track_type.ends_with(".vst3") || track_type.starts_with("vst3:") => {
                let path = track_type.strip_prefix("vst3:").unwrap_or(track_type);
                match crate::vst3_host::Vst3Track::load(path, sr) {
                    Ok(vst3_inst) => Track::new_vst3(id, name, vst3_inst, name),
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load VST3 track '{}': {}", path, e);
                        Track::new_synth(id, name, jamsynth::JamSynthEngine::new(sr))
                    }
                }
            }
            _ if (track_type.ends_with(".clap") || track_type.ends_with(".so"))
                && !track_type.contains("libjamsynth")
                && !track_type.contains("JamSynth") =>
            {
                match crate::clap_host::ClapTrack::load(track_type, sr) {
                    Ok(clap_inst) => Track::new_clap(id, name, clap_inst, name),
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load CLAP track '{}': {}", track_type, e);
                        Track::new_synth(id, name, jamsynth::JamSynthEngine::new(sr))
                    }
                }
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
            // Clean up sidechains targeting this track
            for t in &mut self.tracks {
                t.sidechains.retain(|s| s.target_track_id != track_id);
            }
            // Clean up frozen track cache
            self.frozen_tracks.remove(track_id);
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

            if uri_or_type.starts_with("synth:") || uri_or_type == "jamsynth" || uri_or_type == "synth" {
                if let Some(jamsynth_path) = crate::plugin_scanner::find_jamsynth_plugin() {
                    match crate::clap_host::ClapTrack::load(&jamsynth_path.to_string_lossy(), sr) {
                        Ok(clap_inst) => {
                            apply_synth_preset(&clap_inst, uri_or_type);
                            println!("[jamseq] ✓ Switched track '{}' to JamSynth CLAP: {}", track_id, display_name);
                            track.instrument = TrackInstrument::Clap(Box::new(clap_inst));
                            track.current_plugin = Some(display_name.to_string());
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
                track.current_plugin = Some(display_name.to_string());
            } else if uri_or_type.ends_with(".vst3") || uri_or_type.starts_with("vst3:") {
                let path = uri_or_type.strip_prefix("vst3:").unwrap_or(uri_or_type);
                match crate::vst3_host::Vst3Track::load(path, sr) {
                    Ok(vst3_inst) => {
                        println!("[jamseq] ✓ Switched track '{}' to VST3 plugin: {}", track_id, display_name);
                        track.instrument = TrackInstrument::Vst3(Box::new(vst3_inst));
                        track.current_plugin = Some(display_name.to_string());
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load VST3 plugin '{}': {}", uri_or_type, e);
                    }
                }
            } else if uri_or_type.ends_with(".clap") || uri_or_type.ends_with(".so") {
                match crate::clap_host::ClapTrack::load(uri_or_type, sr) {
                    Ok(clap_inst) => {
                        println!("[jamseq] ✓ Switched track '{}' to CLAP plugin: {}", track_id, display_name);
                        track.instrument = TrackInstrument::Clap(Box::new(clap_inst));
                        track.current_plugin = Some(display_name.to_string());
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
                        track.current_plugin = Some(display_name.to_string());
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

            if uri_or_path.ends_with(".vst3") || uri_or_path.starts_with("vst3:") {
                let path = uri_or_path.strip_prefix("vst3:").unwrap_or(uri_or_path);
                match crate::vst3_host::Vst3Track::load(path, sr) {
                    Ok(vst3_inst) => {
                        println!("[jamseq] ✓ Added VST3 insert effect '{}' to track '{}'", display_name, track_id);
                        track.inserts.push(crate::track::InsertEffect::Vst3 {
                            name: display_name.to_string(),
                            uri: uri_or_path.to_string(),
                            track: Box::new(vst3_inst),
                        });
                        return true;
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load VST3 insert effect '{}': {}", uri_or_path, e);
                        return false;
                    }
                }
            } else if uri_or_path.ends_with(".clap") || uri_or_path.ends_with(".so") {
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

    /// Set a parameter on a track's insert effect
    pub fn set_track_insert_param(&mut self, track_id: &str, insert_index: usize, param_name: &str, value: f32) -> bool {
        if let Some(&idx) = self.track_map.get(track_id) {
            let track = &mut self.tracks[idx];
            if insert_index < track.inserts.len() {
                track.inserts[insert_index].set_parameter(param_name, value);
                return true;
            }
        }
        false
    }


    /// Add an insert effect to the Master Bus
    pub fn add_master_insert(&mut self, uri_or_path: &str, display_name: &str) -> bool {
        let sr = self.sample_rate;
        if uri_or_path.ends_with(".vst3") || uri_or_path.starts_with("vst3:") {
            let path = uri_or_path.strip_prefix("vst3:").unwrap_or(uri_or_path);
            match crate::vst3_host::Vst3Track::load(path, sr) {
                Ok(vst3_inst) => {
                    println!("[jamseq] ✓ Added VST3 Master insert '{}'", display_name);
                    self.master_bus.inserts.push(crate::track::InsertEffect::Vst3 {
                        name: display_name.to_string(),
                        uri: uri_or_path.to_string(),
                        track: Box::new(vst3_inst),
                    });
                    return true;
                }
                Err(e) => {
                    eprintln!("[jamseq] Failed to load VST3 master insert '{}': {}", uri_or_path, e);
                    return false;
                }
            }
        } else if uri_or_path.ends_with(".clap") || uri_or_path.ends_with(".so") {
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
            if uri_or_path.ends_with(".vst3") || uri_or_path.starts_with("vst3:") {
                let path = uri_or_path.strip_prefix("vst3:").unwrap_or(uri_or_path);
                match crate::vst3_host::Vst3Track::load(path, sr) {
                    Ok(vst3_inst) => {
                        println!("[jamseq] ✓ Added VST3 insert '{}' to Aux Bus '{}'", display_name, bus_id);
                        bus.inserts.push(crate::track::InsertEffect::Vst3 {
                            name: display_name.to_string(),
                            uri: uri_or_path.to_string(),
                            track: Box::new(vst3_inst),
                        });
                        return true;
                    }
                    Err(e) => {
                        eprintln!("[jamseq] Failed to load VST3 aux insert '{}': {}", uri_or_path, e);
                        return false;
                    }
                }
            } else if uri_or_path.ends_with(".clap") || uri_or_path.ends_with(".so") {
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

    /// Configure sidechain audio routing from source_track into (target_track, target_insert_index)
    pub fn set_insert_sidechain(&mut self, target_track_id: &str, target_insert_index: usize, source_track_id: &str, gain: f32) -> bool {
        // Clear existing sidechains targeting this specific insert
        self.remove_insert_sidechain(target_track_id, target_insert_index);

        if let Some(&src_idx) = self.track_map.get(source_track_id) {
            if self.track_map.contains_key(target_track_id) {
                self.tracks[src_idx].set_sidechain(target_track_id, target_insert_index, gain);
                println!("[jamseq] ✓ Configured sidechain: '{}' -> ('{}', insert {}) gain: {:.2}",
                    source_track_id, target_track_id, target_insert_index, gain);
                return true;
            }
        }
        false
    }

    /// Remove sidechain audio routing targeting (target_track, target_insert_index)
    pub fn remove_insert_sidechain(&mut self, target_track_id: &str, target_insert_index: usize) -> bool {
        let mut removed = false;
        for track in &mut self.tracks {
            let before = track.sidechains.len();
            track.remove_sidechain(target_track_id, target_insert_index);
            if track.sidechains.len() < before {
                removed = true;
            }
        }
        removed
    }

    /// Compute dependency-ordered track execution sequence so sidechain sources process before target tracks
    pub fn get_track_processing_order(&self) -> Vec<usize> {
        let n = self.tracks.len();
        if n <= 1 {
            return (0..n).collect();
        }

        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut in_degree = vec![0usize; n];

        for (u, track) in self.tracks.iter().enumerate() {
            for sc in &track.sidechains {
                if let Some(&v) = self.track_map.get(&sc.target_track_id) {
                    if u != v {
                        adj[u].push(v);
                        in_degree[v] += 1;
                    }
                }
            }
        }

        let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);

        while let Some(u) = queue.pop_front() {
            order.push(u);
            for &v in &adj[u] {
                in_degree[v] -= 1;
                if in_degree[v] == 0 {
                    queue.push_back(v);
                }
            }
        }

        if order.len() < n {
            for i in 0..n {
                if !order.contains(&i) {
                    order.push(i);
                }
            }
        }

        order
    }

    pub fn seek_tick(&mut self, tick: u64) {
        self.timeline.seek_tick(tick);
    }

    pub fn set_armed_track(&mut self, track_id: Option<String>) {
        println!("[jamseq] Armed track for MIDI input: {:?}", track_id);
        self.armed_track_id = track_id;
    }

    pub fn list_midi_devices(&self) -> Vec<crate::midi_input::MidiDeviceStatus> {
        self.midi_input.list_devices()
    }

    pub fn connect_midi_device(&mut self, index: usize) -> Result<String> {
        self.midi_input.connect_device(index)
    }

    pub fn disconnect_midi_device(&mut self) {
        self.midi_input.disconnect();
    }

    pub fn play(&mut self) {
        if !self.timeline.is_playing {
            self.timeline.play();
        }
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

        // 0. Drain live hardware MIDI inputs and dispatch to armed track
        while let Ok(ev) = self.midi_input.receiver.try_recv() {
            if let Some(ref track_id) = self.armed_track_id {
                if let Some(&track_idx) = self.track_map.get(track_id) {
                    let track = &mut self.tracks[track_idx];
                    let status_type = ev.status & 0xF0;
                    if status_type == 0x90 && ev.data2 > 0 {
                        track.note_on(ev.data1, ev.data2 as f32 / 127.0);
                    } else if status_type == 0x80 || (status_type == 0x90 && ev.data2 == 0) {
                        track.note_off(ev.data1);
                    } else if status_type == 0xB0 {
                        track.handle_cc(ev.data1, ev.data2);
                    }
                }
            }
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

        self.sidechain_buffers.clear();
        let track_order = self.get_track_processing_order();

        let current_tick = self.timeline.current_tick;

        // Render all tracks into master bus and tap aux sends & sidechains
        for &track_idx in &track_order {
            let track = &mut self.tracks[track_idx];
            if any_solo && !track.solo {
                continue;
            }
            let effective_bpm = self.timeline.get_track_effective_bpm(&track.id, current_tick);
            track.set_tempo(effective_bpm);
            let is_armed = self.armed_track_id.as_deref() == Some(&track.id);
            track.process_block(
                current_sample,
                &mut master_left[..num_samples],
                &mut master_right[..num_samples],
                &mut self.scratch_left[..num_samples],
                &mut self.scratch_right[..num_samples],
                Some(&self.sidechain_buffers),
                is_armed,
                self.timeline.is_playing,
            );

            // Tap sidechains: record pre-fader/post-synth audio into sidechain buffers
            if !track.muted && !track.sidechains.is_empty() {
                for sc in &track.sidechains {
                    let entry = self.sidechain_buffers
                        .entry((sc.target_track_id.clone(), sc.target_insert_index))
                        .or_insert_with(|| (vec![0.0; num_samples], vec![0.0; num_samples]));
                    if entry.0.len() < num_samples {
                        entry.0.resize(num_samples, 0.0);
                        entry.1.resize(num_samples, 0.0);
                    }
                    for i in 0..num_samples {
                        entry.0[i] += self.scratch_left[i] * sc.gain;
                        entry.1[i] += self.scratch_right[i] * sc.gain;
                    }
                }
            }

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
                        lv2.process_effect_block(bus_l, bus_r);
                    }
                    crate::track::InsertEffect::Vst3 { track: vst3, .. } => {
                        vst3.process_effect_block(bus_l, bus_r);
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
                        lv2.process_effect_block(&mut master_left[..num_samples], &mut master_right[..num_samples]);
                    }
                    crate::track::InsertEffect::Vst3 { track: vst3, .. } => {
                        vst3.process_effect_block(&mut master_left[..num_samples], &mut master_right[..num_samples]);
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

    /// Offline render a single track directly to a stereo 24-bit WAV file.
    pub fn render_track_to_wav(&mut self, track_id: &str, out_path: &Path, num_bars: u32) -> Result<()> {
        let spec = WavSpec {
            channels: 2,
            sample_rate: self.sample_rate as u32,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };

        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut writer = WavWriter::create(out_path, spec)
            .with_context(|| format!("Failed to create output WAV at {:?}", out_path))?;

        let total_ticks = num_bars as u64 * 4 * crate::timeline::PPQ;
        let total_samples = self.timeline.ticks_to_samples(total_ticks);

        let target_idx = *self.track_map.get(track_id).ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;
        let orig_active_stems = std::mem::take(&mut self.tracks[target_idx].active_block_stems);

        let orig_states: Vec<(bool, bool)> = self.tracks.iter().map(|t| (t.muted, t.solo)).collect();
        let orig_aux_mutes: Vec<bool> = self.aux_buses.iter().map(|b| b.muted).collect();
        let orig_pos = self.timeline.current_tick;
        let orig_loop = self.timeline.loop_enabled;
        let orig_master_inserts = std::mem::take(&mut self.master_bus.inserts);

        // Solo only target track and mute aux return buses to capture isolated track stem
        for t in &mut self.tracks {
            t.solo = t.id == track_id;
            t.muted = false;
        }
        for b in &mut self.aux_buses {
            b.muted = true;
        }

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
        for t in &mut self.tracks {
            t.all_notes_off();
        }

        // Restore target track stems and solo/mute states
        self.tracks[target_idx].active_block_stems = orig_active_stems;
        for (i, t) in self.tracks.iter_mut().enumerate() {
            if i < orig_states.len() {
                t.muted = orig_states[i].0;
                t.solo = orig_states[i].1;
            }
        }
        for (i, b) in self.aux_buses.iter_mut().enumerate() {
            if i < orig_aux_mutes.len() {
                b.muted = orig_aux_mutes[i];
            }
        }
        self.master_bus.inserts = orig_master_inserts;
        self.timeline.loop_enabled = orig_loop;
        self.timeline.seek_tick(orig_pos);

        Ok(())
    }

    /// Offline render a single track directly to an in-memory SampleBuffer without disk I/O.
    pub fn render_track_to_buffer(&mut self, track_id: &str, num_bars: u32) -> Result<crate::sampler::SampleBuffer> {
        let target_idx = *self.track_map.get(track_id).ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;
        let orig_active_stems = std::mem::take(&mut self.tracks[target_idx].active_block_stems);

        let total_ticks = num_bars as u64 * 4 * crate::timeline::PPQ;
        let total_samples = self.timeline.ticks_to_samples(total_ticks) as usize;

        let orig_states: Vec<(bool, bool)> = self.tracks.iter().map(|t| (t.muted, t.solo)).collect();
        let orig_aux_mutes: Vec<bool> = self.aux_buses.iter().map(|b| b.muted).collect();
        let orig_pos = self.timeline.current_tick;
        let orig_loop = self.timeline.loop_enabled;
        let orig_master_inserts = std::mem::take(&mut self.master_bus.inserts);
        let orig_track_vol = self.tracks[target_idx].volume;
        let orig_track_pan = self.tracks[target_idx].pan;
        let orig_master_gain = self.master_gain;
        let orig_master_vol = self.master_bus.volume;
        let orig_master_pan = self.master_bus.pan;

        // Solo target track at unity gain and mute aux return buses to capture isolated pure track stem
        for t in &mut self.tracks {
            t.solo = t.id == track_id;
            t.muted = false;
        }
        self.tracks[target_idx].volume = 1.0;
        self.tracks[target_idx].pan = 0.0;
        self.master_gain = 1.0;
        self.master_bus.volume = 1.0;
        self.master_bus.pan = 0.0;
        for b in &mut self.aux_buses {
            b.muted = true;
        }

        self.timeline.seek_tick(0);
        self.timeline.loop_enabled = false;
        self.timeline.play();

        let mut samples_left = Vec::with_capacity(total_samples);
        let mut samples_right = Vec::with_capacity(total_samples);

        let block_size = 512;
        let mut left = vec![0.0f32; block_size];
        let mut right = vec![0.0f32; block_size];
        let mut rendered = 0;

        while rendered < total_samples {
            let current_block = (total_samples - rendered).min(block_size);
            self.process_block(&mut left[..current_block], &mut right[..current_block]);

            samples_left.extend_from_slice(&left[..current_block]);
            samples_right.extend_from_slice(&right[..current_block]);

            rendered += current_block;
        }

        self.timeline.pause();
        for t in &mut self.tracks {
            t.all_notes_off();
        }

        // Restore target track stems, faders, and states
        self.tracks[target_idx].active_block_stems = orig_active_stems;
        self.tracks[target_idx].volume = orig_track_vol;
        self.tracks[target_idx].pan = orig_track_pan;
        self.master_gain = orig_master_gain;
        self.master_bus.volume = orig_master_vol;
        self.master_bus.pan = orig_master_pan;
        for (i, t) in self.tracks.iter_mut().enumerate() {
            if i < orig_states.len() {
                t.muted = orig_states[i].0;
                t.solo = orig_states[i].1;
            }
        }
        for (i, b) in self.aux_buses.iter_mut().enumerate() {
            if i < orig_aux_mutes.len() {
                b.muted = orig_aux_mutes[i];
            }
        }
        self.master_bus.inserts = orig_master_inserts;
        self.timeline.loop_enabled = orig_loop;
        self.timeline.seek_tick(orig_pos);

        Ok(crate::sampler::SampleBuffer {
            channels: 2,
            sample_rate: self.sample_rate as u32,
            samples_left,
            samples_right,
        })
    }

    /// Re-render only a single modified track block and atomically swap into track.active_block_stems
    pub fn rebake_block(&mut self, track_id: &str, block_id: &str) -> bool {
        let maybe_resolved = self.timeline.get_resolved_blocks_for_track(track_id)
            .into_iter()
            .find(|(b, _)| b.id == block_id)
            .map(|(b, tick)| (b.clone(), tick));

        let (block, abs_start_tick) = match maybe_resolved {
            Some((b, tick)) => (b, tick),
            None => {
                if let Some(&t_idx) = self.track_map.get(track_id) {
                    self.tracks[t_idx].active_block_stems.retain(|bs| bs.block_id != block_id);
                }
                return true;
            }
        };

        let &t_idx = match self.track_map.get(track_id) {
            Some(idx) => idx,
            None => return false,
        };

        let eff_bpm = if let Some(ref sec_id) = block.section_id {
            let sec_bpm = self.timeline.sections.iter()
                .find(|s| &s.id == sec_id)
                .map(|s| s.effective_bpm(self.timeline.bpm))
                .unwrap_or(self.timeline.bpm);
            block.effective_bpm(sec_bpm)
        } else {
            block.effective_bpm(self.timeline.bpm)
        };

        match crate::block_renderer::render_block(&mut self.tracks[t_idx], &block, eff_bpm, self.sample_rate) {
            Ok(buf) => {
                let start_sample = self.timeline.ticks_to_samples(abs_start_tick) as usize;
                let end_sample = start_sample + buf.samples_left.len();

                let new_stem = crate::track::BlockStem {
                    block_id: block.id.clone(),
                    track_id: track_id.to_string(),
                    start_sample,
                    end_sample,
                    buffer: buf,
                };

                if let Some(pos) = self.tracks[t_idx].active_block_stems.iter().position(|bs| bs.block_id == block.id) {
                    self.tracks[t_idx].active_block_stems[pos] = new_stem;
                } else {
                    self.tracks[t_idx].active_block_stems.push(new_stem);
                }
                true
            }
            Err(e) => {
                eprintln!("[jamseq] Failed to rebake block {}: {}", block_id, e);
                false
            }
        }
    }

    /// Rebake all blocks for a given track (e.g. after parameter or preset changes)
    pub fn rebake_blocks_for_track(&mut self, track_id: &str) -> usize {
        let &t_idx = match self.track_map.get(track_id) {
            Some(idx) => idx,
            None => return 0,
        };

        let resolved: Vec<(crate::timeline::TrackBlock, u64)> = self.timeline
            .get_resolved_blocks_for_track(track_id)
            .into_iter()
            .map(|(b, tick)| (b.clone(), tick))
            .collect();

        let mut count = 0;
        for (block, abs_start_tick) in resolved {
            let eff_bpm = if let Some(ref sec_id) = block.section_id {
                let sec_bpm = self.timeline.sections.iter()
                    .find(|s| &s.id == sec_id)
                    .map(|s| s.effective_bpm(self.timeline.bpm))
                    .unwrap_or(self.timeline.bpm);
                block.effective_bpm(sec_bpm)
            } else {
                block.effective_bpm(self.timeline.bpm)
            };

            if let Ok(buf) = crate::block_renderer::render_block(&mut self.tracks[t_idx], &block, eff_bpm, self.sample_rate) {
                let start_sample = self.timeline.ticks_to_samples(abs_start_tick) as usize;
                let end_sample = start_sample + buf.samples_left.len();

                let new_stem = crate::track::BlockStem {
                    block_id: block.id.clone(),
                    track_id: track_id.to_string(),
                    start_sample,
                    end_sample,
                    buffer: buf,
                };

                if let Some(pos) = self.tracks[t_idx].active_block_stems.iter().position(|bs| bs.block_id == block.id) {
                    self.tracks[t_idx].active_block_stems[pos] = new_stem;
                } else {
                    self.tracks[t_idx].active_block_stems.push(new_stem);
                }
                count += 1;
            }
        }
        count
    }

    /// Rebake all blocks in the arrangement
    pub fn rebake_all_blocks(&mut self) -> usize {
        let track_ids: Vec<String> = self.tracks.iter().map(|t| t.id.clone()).collect();
        let mut total = 0;
        for tid in track_ids {
            total += self.rebake_blocks_for_track(&tid);
        }
        total
    }

    /// Freeze a track by rendering it offline to audio and replacing instrument/inserts with a zero-CPU audio clip player
    pub fn freeze_track(&mut self, track_id: &str) -> Result<String> {
        let &idx = self.track_map.get(track_id).ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;
        if self.tracks[idx].is_frozen {
            anyhow::bail!("Track '{}' is already frozen", track_id);
        }

        let wav_path = if let Some(ref p) = self.project_path {
            let project_dir = Path::new(p).parent().unwrap_or_else(|| Path::new("."));
            let freeze_dir = project_dir.join("frozen");
            let _ = std::fs::create_dir_all(&freeze_dir);
            freeze_dir.join(format!("{}.wav", track_id))
        } else {
            std::env::temp_dir().join(format!("jamseq_frozen_{}.wav", track_id))
        };

        let num_bars = ((self.timeline.total_ticks / (4 * crate::timeline::PPQ)) as u32).max(1);
        self.render_track_to_wav(track_id, &wav_path, num_bars)?;

        let track = &mut self.tracks[idx];
        let mut clip_track = AudioClipTrack::new();
        clip_track.add_clip(AudioClip::new(&wav_path, 0));

        let orig_instrument = std::mem::replace(&mut track.instrument, TrackInstrument::AudioClips(clip_track));
        let orig_inserts = std::mem::take(&mut track.inserts);

        self.frozen_tracks.insert(track_id.to_string(), FrozenTrackState {
            instrument: orig_instrument,
            inserts: orig_inserts,
            wav_path: wav_path.to_string_lossy().to_string(),
        });

        track.is_frozen = true;
        println!("[jamseq] ✓ Frozen track '{}' -> {:?}", track_id, wav_path);
        Ok(wav_path.to_string_lossy().to_string())
    }

    /// Unfreeze a track, restoring original synth/plugin instrument and inserts
    pub fn unfreeze_track(&mut self, track_id: &str) -> Result<()> {
        let state = self.frozen_tracks.remove(track_id)
            .ok_or_else(|| anyhow::anyhow!("Track '{}' is not frozen", track_id))?;
        let &idx = self.track_map.get(track_id).ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

        let track = &mut self.tracks[idx];
        track.instrument = state.instrument;
        track.inserts = state.inserts;
        track.is_frozen = false;

        let _ = std::fs::remove_file(&state.wav_path);
        println!("[jamseq] ✓ Unfrozen track '{}'", track_id);
        Ok(())
    }

    /// Bounce track into a new standalone audio clip track
    pub fn bounce_track(&mut self, track_id: &str, new_track_name: &str) -> Result<String> {
        let _ = self.track_map.get(track_id).ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let bounce_id = format!("bounce_{}_{}", track_id, timestamp);

        let wav_path = if let Some(ref p) = self.project_path {
            let project_dir = Path::new(p).parent().unwrap_or_else(|| Path::new("."));
            let bounce_dir = project_dir.join("bounces");
            let _ = std::fs::create_dir_all(&bounce_dir);
            bounce_dir.join(format!("{}.wav", bounce_id))
        } else {
            std::env::temp_dir().join(format!("jamseq_{}.wav", bounce_id))
        };

        let num_bars = ((self.timeline.total_ticks / (4 * crate::timeline::PPQ)) as u32).max(1);
        self.render_track_to_wav(track_id, &wav_path, num_bars)?;

        let mut clip_track = AudioClipTrack::new();
        clip_track.add_clip(AudioClip::new(&wav_path, 0));

        let new_track = Track::new_audio_clips(&bounce_id, new_track_name, clip_track);
        self.add_track(new_track);
        println!("[jamseq] ✓ Bounced track '{}' to new track '{}' ({})", track_id, new_track_name, bounce_id);
        Ok(bounce_id)
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
