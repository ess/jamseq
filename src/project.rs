use std::path::Path;
use anyhow::{Context, Result};
use crate::engine::JamSeqHost;
use crate::timeline::TimelineArrangement;

impl JamSeqHost {
    /// Serialize the current live state into a TimelineArrangement
    pub fn save_arrangement(&self) -> TimelineArrangement {
        let mut arrangement = self.timeline.to_arrangement();

        // 1. Serialize Tracks with full DSP & parameter state
        arrangement.tracks = self.tracks.iter().map(|t| {
            let t_type = match &t.instrument {
                crate::track::TrackInstrument::AudioClips(_) => "audio".to_string(),
                crate::track::TrackInstrument::Sampler(_) => "sampler".to_string(),
                crate::track::TrackInstrument::Vst3(v) => format!("vst3:{}", v.path),
                crate::track::TrackInstrument::Lv2(l) => l.uri.clone(),
                crate::track::TrackInstrument::Clap(c) => c.path.clone(),
                _ => "synth".to_string(),
            };

            let parameters: std::collections::HashMap<String, f32> = t.get_parameters()
                .into_iter()
                .map(|(name, val, _, _)| (name, val))
                .collect();

            let inserts: Vec<crate::timeline::InsertConfig> = t.inserts.iter().map(|ins| {
                let ins_params: std::collections::HashMap<String, f32> = ins.get_parameters()
                    .into_iter()
                    .map(|(name, val, _, _)| (name, val))
                    .collect();
                crate::timeline::InsertConfig {
                    name: ins.name().to_string(),
                    uri: ins.uri().to_string(),
                    parameters: ins_params,
                }
            }).collect();

            let sends = t.sends.iter().map(|s| crate::timeline::AuxSendConfig {
                target_bus_id: s.target_bus_id.clone(),
                gain: s.gain,
                is_pre_fader: s.is_pre_fader,
                is_muted: s.is_muted,
            }).collect();

            let sidechains = t.sidechains.iter().map(|s| crate::timeline::SidechainSendConfig {
                target_track_id: s.target_track_id.clone(),
                target_insert_index: s.target_insert_index,
                gain: s.gain,
            }).collect();

            crate::timeline::TrackConfig {
                id: t.id.clone(),
                name: t.name.clone(),
                track_type: t_type,
                arranger_mode: t.arranger_mode,
                volume: t.volume,
                pan: t.pan,
                muted: t.muted,
                solo: t.solo,
                current_plugin: t.current_plugin.clone(),
                parameters,
                inserts,
                sends,
                sidechains,
            }
        }).collect();

        // 2. Serialize Master Bus
        let master_inserts: Vec<crate::timeline::InsertConfig> = self.master_bus.inserts.iter().map(|ins| {
            let ins_params: std::collections::HashMap<String, f32> = ins.get_parameters()
                .into_iter()
                .map(|(name, val, _, _)| (name, val))
                .collect();
            crate::timeline::InsertConfig {
                name: ins.name().to_string(),
                uri: ins.uri().to_string(),
                parameters: ins_params,
            }
        }).collect();

        arrangement.master_bus = Some(crate::timeline::MasterBusConfig {
            volume: self.master_bus.volume,
            pan: self.master_bus.pan,
            muted: self.master_bus.muted,
            inserts: master_inserts,
        });

        // 3. Serialize Aux Buses
        arrangement.aux_buses = self.aux_buses.iter().map(|b| {
            let bus_inserts: Vec<crate::timeline::InsertConfig> = b.inserts.iter().map(|ins| {
                let ins_params: std::collections::HashMap<String, f32> = ins.get_parameters()
                    .into_iter()
                    .map(|(name, val, _, _)| (name, val))
                    .collect();
                crate::timeline::InsertConfig {
                    name: ins.name().to_string(),
                    uri: ins.uri().to_string(),
                    parameters: ins_params,
                }
            }).collect();

            crate::timeline::AuxBusConfig {
                id: b.id.clone(),
                name: b.name.clone(),
                volume: b.volume,
                pan: b.pan,
                muted: b.muted,
                solo: b.solo,
                inserts: bus_inserts,
            }
        }).collect();

        arrangement
    }

    /// Save the current live arrangement to a project JSON on disk
    pub fn save_project<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path_ref = path.as_ref();
        let path_str = path_ref.to_string_lossy();
        let final_path = if path_str.ends_with(".jam.json") || path_str.ends_with(".json") {
            path_ref.to_path_buf()
        } else {
            std::path::PathBuf::from(format!("{}.jam.json", path_str))
        };

        let arrangement = self.save_arrangement();
        let json_data = serde_json::to_string_pretty(&arrangement)
            .context("Failed to serialize arrangement to JSON")?;
        if let Some(parent) = final_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&final_path, json_data)
            .with_context(|| format!("Failed to write project file to {:?}", final_path))?;
        self.project_path = Some(final_path.to_string_lossy().to_string());
        Ok(())
    }

    pub fn save_project_to_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.save_project(path)
    }

    /// Load a project arrangement JSON from disk
    pub fn load_project<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let path_ref = path.as_ref();
        let content = std::fs::read_to_string(path_ref)
            .with_context(|| format!("Failed to read project file at {:?}", path_ref))?;
        let mut arrangement: TimelineArrangement = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse project JSON at {:?}", path_ref))?;
        if arrangement.tracks.is_empty() {
            arrangement.discover_tracks_from_content();
        }
        self.load_arrangement(arrangement);
        self.project_path = Some(path_ref.to_string_lossy().to_string());
        Ok(())
    }

    pub fn load_project_from_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.load_project(path)
    }

    /// Reset and start a fresh empty project with default sections and tempo
    pub fn new_project(&mut self, title: &str, bpm: f64) {
        let mut arrangement = TimelineArrangement::default();
        arrangement.title = title.to_string();
        arrangement.bpm = if bpm > 10.0 { bpm } else { 120.0 };
        arrangement.total_bars = 0;
        arrangement.sections = Vec::new();
        arrangement.events = Vec::new();
        self.tracks.clear();
        self.track_map.clear();
        self.aux_buses.clear();
        self.aux_map.clear();
        self.aux_left.clear();
        self.aux_right.clear();
        self.master_bus.inserts.clear();
        self.frozen_tracks.clear();
        self.sidechain_buffers.clear();
        self.load_arrangement(arrangement);
        self.project_path = None;
    }

    pub fn load_arrangement(&mut self, mut arrangement: TimelineArrangement) {
        // If the arrangement did not have tracks saved (e.g. legacy or minimal project) and host has no tracks, auto-discover
        if arrangement.tracks.is_empty() && self.tracks.is_empty() {
            arrangement.discover_tracks_from_content();
        }

        // 1. Restore Master Bus
        if let Some(ref mb) = arrangement.master_bus {
            self.master_bus.volume = mb.volume;
            self.master_bus.pan = mb.pan;
            self.master_bus.muted = mb.muted;
            self.master_bus.inserts.clear();
            for ins in &mb.inserts {
                if self.add_master_insert(&ins.uri, &ins.name) {
                    if let Some(inst) = self.master_bus.inserts.last_mut() {
                        for (p_name, &p_val) in &ins.parameters {
                            inst.set_parameter(p_name, p_val);
                        }
                    }
                }
            }
        }

        // 2. Restore Aux Buses
        if !arrangement.aux_buses.is_empty() {
            self.aux_buses.clear();
            self.aux_map.clear();
            self.aux_left.clear();
            self.aux_right.clear();
            for ab in &arrangement.aux_buses {
                self.add_aux_bus(&ab.id, &ab.name);
                if let Some(&idx) = self.aux_map.get(&ab.id) {
                    self.aux_buses[idx].volume = ab.volume;
                    self.aux_buses[idx].pan = ab.pan;
                    self.aux_buses[idx].muted = ab.muted;
                    self.aux_buses[idx].solo = ab.solo;
                    for ins in &ab.inserts {
                        if self.add_aux_insert(&ab.id, &ins.uri, &ins.name) {
                            if let Some(inst) = self.aux_buses[idx].inserts.last_mut() {
                                for (p_name, &p_val) in &ins.parameters {
                                    inst.set_parameter(p_name, p_val);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Restore Tracks & Plugins
        if !arrangement.tracks.is_empty() {
            self.tracks.clear();
            self.track_map.clear();
            for tc in &arrangement.tracks {
                let mut resolved_type = tc.track_type.clone();
                let display_name = tc.current_plugin.clone().unwrap_or_else(|| tc.name.clone());

                // Auto-healing: If track_type was saved as libjamsynth.so or "synth", but current_plugin indicates an LV2, VST3, or CLAP plugin
                if (resolved_type.contains("libjamsynth") || resolved_type == "synth")
                    && !display_name.starts_with("JamSynth")
                {
                    if let Some(p) = self.available_plugins.iter().find(|p| {
                        p.name == display_name
                            || format!("{} ({})", p.name, p.plugin_format) == display_name
                    }) {
                        resolved_type = p.uri_or_path.clone();
                    }
                }

                self.add_custom_track(&tc.id, &tc.name, &resolved_type);
                if let Some(&idx) = self.track_map.get(&tc.id) {
                    self.tracks[idx].arranger_mode = tc.arranger_mode;
                    self.tracks[idx].volume = tc.volume;
                    self.tracks[idx].pan = tc.pan;
                    self.tracks[idx].muted = tc.muted;
                    self.tracks[idx].solo = tc.solo;
                    if let Some(ref p) = tc.current_plugin {
                        self.tracks[idx].current_plugin = Some(p.clone());
                    }

                    // Restore instrument parameters!
                    for (p_name, &p_val) in &tc.parameters {
                        self.tracks[idx].set_parameter(p_name, p_val);
                    }

                    // Restore inserts & their parameters!
                    for ins in &tc.inserts {
                        if self.add_track_insert(&tc.id, &ins.uri, &ins.name) {
                            if let Some(inst) = self.tracks[idx].inserts.last_mut() {
                                for (p_name, &p_val) in &ins.parameters {
                                    inst.set_parameter(p_name, p_val);
                                }
                            }
                        }
                    }

                    // Restore aux sends
                    for s in &tc.sends {
                        self.set_track_aux_send(&tc.id, &s.target_bus_id, s.gain, s.is_pre_fader);
                    }

                    // Restore sidechains
                    for sc in &tc.sidechains {
                        self.set_insert_sidechain(&tc.id, sc.target_insert_index, &sc.target_track_id, sc.gain);
                    }
                }
            }
        } else {
            // If arrangement has no tracks specified, ensure any track IDs referenced in sections/events exist
            let mut missing_ids = Vec::new();
            for sec in &arrangement.sections {
                for (t_id, _) in &sec.track_blocks {
                    if !self.track_map.contains_key(t_id) && !missing_ids.contains(t_id) {
                        missing_ids.push(t_id.clone());
                    }
                }
            }
            for ev in &arrangement.events {
                if !self.track_map.contains_key(&ev.track_id) && !missing_ids.contains(&ev.track_id) {
                    missing_ids.push(ev.track_id.clone());
                }
            }
            for id in missing_ids {
                let track_type = if id.starts_with("audio_") || id.starts_with("mic_") { "audio" } else { "synth" };
                let name = if id.starts_with("track_") {
                    let num = id.strip_prefix("track_").unwrap_or("1");
                    format!("MIDI Track {}", num)
                } else if id.starts_with("audio_") {
                    let num = id.strip_prefix("audio_").unwrap_or("1");
                    format!("Audio Track {}", num)
                } else {
                    id.clone()
                };
                self.add_custom_track(&id, &name, track_type);
            }
        }

        self.timeline.load_arrangement(arrangement);
        self.rebake_all_blocks();
    }
}
