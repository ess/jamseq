use crate::engine::JamSeqHost;
use crate::midi_input::MidiDeviceStatus;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::timeline::{ScheduledEvent, SongSection};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args")]
pub enum IpcCommand {
    Play,
    Pause,
    Seek { tick: u64 },
    SetVolume { track_id: String, volume: f32 },
    SetPan { track_id: String, pan: f32 },
    SetMute { track_id: String, muted: bool },
    SetSolo { track_id: String, solo: bool },
    SetMasterVolume { volume: f32 },
    StartRecording { out_path: String },
    StopRecording,
    GetStatus,
    /// Targeted operational edit: update one track's pattern within a specific section scope
    ReplaceSectionEvents {
        section_id: String,
        track_id: String,
        events: Vec<ScheduledEvent>,
    },
    /// Granular edit: Insert a note into a track
    AddNote {
        note_on_id: u64,
        note_off_id: u64,
        track_id: String,
        tick: u64,
        duration_ticks: u64,
        note: u8,
        velocity: f32,
    },
    /// Granular edit: Delete note at tick
    DeleteNoteAt {
        track_id: String,
        tick: u64,
        note: u8,
    },
    /// Granular edit: Move single event
    MoveEvent {
        event_id: u64,
        new_tick: u64,
    },
    /// Granular edit: Remove single event
    RemoveEvent {
        event_id: u64,
    },
    /// Humanize: apply micro-timing drift and dragging to humanize a track
    HumanizeTrack {
        track_id: String,
        section_id: Option<String>,
        timing_drift_ticks: i64,
        drag_bias_ticks: i64,
        velocity_jitter: f32,
    },
    /// Load project JSON arrangement from file path
    LoadProject {
        path: String,
    },
    /// Save current arrangement to project JSON file path
    SaveProject {
        path: String,
    },
    /// Create a new project template
    NewProject {
        title: String,
        bpm: f64,
    },
    /// Manually create and add a new track (synth or audio) to the session
    AddTrack {
        id: String,
        name: String,
        track_type: String, // "synth" or "audio"
    },
    /// Rename an existing track
    RenameTrack {
        track_id: String,
        new_name: String,
    },
    /// Tweak a live DSP / synth / plugin parameter
    SetTrackParam {
        track_id: String,
        param_name: String,
        value: f32,
    },
    /// Save track's current synth patch to a preset JSON file
    SaveTrackPreset {
        track_id: String,
        preset_name: String,
    },
    /// Load a preset JSON file into a track's synth
    LoadTrackPreset {
        track_id: String,
        preset_name: String,
    },
    /// Switch a track's instrument to a specific VST/LV2 plugin or internal synth
    SwitchTrackPlugin {
        track_id: String,
        plugin_uri_or_type: String,
        plugin_name: String,
    },
    /// Delete an existing track
    DeleteTrack {
        track_id: String,
    },
    /// Reorder a track from index to index
    ReorderTrack {
        from: usize,
        to: usize,
    },
    /// Schedule an automation point on the timeline
    AddAutomationPoint {
        id: u64,
        tick: u64,
        track_id: String,
        param_name: String,
        value: f32,
    },
    /// Add an insert effect plugin to a track's signal chain
    AddTrackInsert {
        track_id: String,
        plugin_uri_or_path: String,
        plugin_name: String,
    },
    /// Remove an insert effect plugin from a track by index
    RemoveTrackInsert {
        track_id: String,
        insert_index: usize,
    },
    /// Add an insert effect plugin to the Master Bus
    AddMasterInsert {
        plugin_uri_or_path: String,
        plugin_name: String,
    },
    /// Remove an insert effect plugin from the Master Bus
    RemoveMasterInsert {
        insert_index: usize,
    },
    /// Tweak a Master insert parameter
    SetMasterInsertParam {
        insert_index: usize,
        param_name: String,
        value: f32,
    },
    /// Set master pan
    SetMasterPan {
        pan: f32,
    },
    /// Create a new Aux Bus
    AddAuxBus {
        id: String,
        name: String,
    },
    /// Remove an Aux Bus
    RemoveAuxBus {
        bus_id: String,
    },
    /// Add an insert effect plugin to an Aux Bus
    AddAuxInsert {
        bus_id: String,
        plugin_uri_or_path: String,
        plugin_name: String,
    },
    /// Remove an insert effect plugin from an Aux Bus
    RemoveAuxInsert {
        bus_id: String,
        insert_index: usize,
    },
    /// Tweak an Aux insert parameter
    SetAuxInsertParam {
        bus_id: String,
        insert_index: usize,
        param_name: String,
        value: f32,
    },
    /// Configure a track's aux send level and pre/post-fader switch
    SetTrackAuxSend {
        track_id: String,
        bus_id: String,
        gain: f32,
        is_pre_fader: bool,
    },
    /// Remove a track's aux send
    RemoveTrackAuxSend {
        track_id: String,
        bus_id: String,
    },
    /// List hardware and virtual MIDI input devices
    ListMidiDevices,
    /// Connect to a MIDI input device by index
    ConnectMidiDevice {
        port_index: usize,
    },
    /// Disconnect active MIDI input device
    DisconnectMidiDevice,
    /// Set armed track ID for live MIDI performance / recording
    SetArmedTrack {
        track_id: Option<String>,
    },
    /// Configure sidechain audio routing from a track into a target track's insert effect
    SetInsertSidechain {
        track_id: String,
        insert_index: usize,
        source_track_id: String,
        gain: f32,
    },
    /// Remove sidechain audio routing from a target track's insert effect
    RemoveInsertSidechain {
        track_id: String,
        insert_index: usize,
    },
    /// Freeze a track into a cached offline audio stem
    FreezeTrack {
        track_id: String,
    },
    /// Restore frozen track to interactive synth/plugin and editable timeline events
    UnfreezeTrack {
        track_id: String,
    },
    /// Bounce a track offline into a standalone audio clip track
    BounceTrack {
        track_id: String,
        new_track_name: String,
    },
    /// Rescan system for newly installed LV2 and CLAP plugins
    RescanPlugins,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInsertStatus {
    pub name: String,
    pub uri: String,
    pub params: Vec<TrackParamStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackParamStatus {
    pub name: String,
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxSendStatus {
    pub target_bus_id: String,
    pub gain: f32,
    pub is_pre_fader: bool,
    pub is_muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidechainSendStatus {
    pub target_track_id: String,
    pub target_insert_index: usize,
    pub gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxBusStatus {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub peak_level: f32,
    pub inserts: Vec<TrackInsertStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackStatus {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub is_frozen: bool,
    pub peak_level: f32,
    pub params: Vec<TrackParamStatus>,
    pub inserts: Vec<TrackInsertStatus>,
    pub sends: Vec<AuxSendStatus>,
    pub sidechains: Vec<SidechainSendStatus>,
    pub current_plugin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcStatusResponse {
    pub project_title: String,
    pub project_path: Option<String>,
    pub is_playing: bool,
    pub current_tick: u64,
    pub current_sample: u64,
    pub total_ticks: u64,
    pub bpm: f64,
    pub master_volume: f32,
    pub master_peak: f32,
    pub master_inserts: Vec<TrackInsertStatus>,
    pub aux_buses: Vec<AuxBusStatus>,
    pub armed_track_id: Option<String>,
    pub midi_devices: Vec<MidiDeviceStatus>,
    pub is_recording: bool,
    pub mic_peak: f32,
    pub active_section_id: Option<String>,
    pub sections: Vec<SongSection>,
    pub tracks: Vec<TrackStatus>,
    pub events: Vec<ScheduledEvent>,
    pub available_plugins: Vec<crate::plugin_scanner::DiscoveredPlugin>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    pub status: Option<IpcStatusResponse>,
    pub error: Option<String>,
}

pub struct IpcServer {
    socket_path: PathBuf,
    running: Arc<AtomicBool>,
}

impl IpcServer {
    pub fn new<P: AsRef<Path>>(socket_path: P) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, host: Arc<Mutex<JamSeqHost>>) -> Result<()> {
        if self.socket_path.exists() {
            let _ = fs::remove_file(&self.socket_path);
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        listener.set_nonblocking(true)?;

        self.running.store(true, Ordering::SeqCst);
        let running_flag = Arc::clone(&self.running);

        thread::spawn(move || {
            while running_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        let host_clone = Arc::clone(&host);
                        thread::spawn(move || {
                            handle_client(stream, host_clone);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        eprintln!("[jamseq:ipc] Listener accept error: {:?}", e);
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });

        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if self.socket_path.exists() {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

fn handle_client(stream: UnixStream, host: Arc<Mutex<JamSeqHost>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<IpcCommand>(&line) {
            Ok(cmd) => execute_command(cmd, &host),
            Err(e) => IpcResponse {
                ok: false,
                status: None,
                error: Some(format!("Invalid command JSON: {}", e)),
            },
        };

        if let Ok(resp_json) = serde_json::to_string(&response) {
            let _ = writeln!(writer, "{}", resp_json);
            let _ = writer.flush();
        }
    }
}

fn execute_command(cmd: IpcCommand, host: &Arc<Mutex<JamSeqHost>>) -> IpcResponse {
    let mut h = match host.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return IpcResponse {
                ok: false,
                status: None,
                error: Some("Host mutex poisoned".to_string()),
            }
        }
    };

    match cmd {
        IpcCommand::Play => {
            h.play();
            build_status_response(&h)
        }
        IpcCommand::Pause => {
            h.pause();
            build_status_response(&h)
        }
        IpcCommand::Seek { tick } => {
            h.timeline.seek_tick(tick);
            build_status_response(&h)
        }
        IpcCommand::SetVolume { track_id, volume } => {
            if let Some(&idx) = h.track_map.get(&track_id) {
                h.tracks[idx].volume = volume;
            }
            build_status_response(&h)
        }
        IpcCommand::SetPan { track_id, pan } => {
            if let Some(&idx) = h.track_map.get(&track_id) {
                h.tracks[idx].pan = pan;
            }
            build_status_response(&h)
        }
        IpcCommand::SetMute { track_id, muted } => {
            if let Some(&idx) = h.track_map.get(&track_id) {
                h.tracks[idx].muted = muted;
            }
            build_status_response(&h)
        }
        IpcCommand::SetSolo { track_id, solo } => {
            if let Some(&idx) = h.track_map.get(&track_id) {
                h.tracks[idx].solo = solo;
            }
            build_status_response(&h)
        }
        IpcCommand::SetMasterVolume { volume } => {
            let v = volume.clamp(0.0, 2.0);
            h.master_gain = v;
            h.master_bus.volume = v;
            build_status_response(&h)
        }
        IpcCommand::StartRecording { out_path } => {
            let start_sample = h.timeline.current_sample;
            h.recorder.start_recording(start_sample, Path::new(&out_path));
            build_status_response(&h)
        }
        IpcCommand::StopRecording => {
            let sr = h.sample_rate as u32;
            if let Ok(clip_info) = h.recorder.stop_recording(sr) {
                h.attach_recorded_clip(&clip_info);
            }
            build_status_response(&h)
        }
        IpcCommand::ReplaceSectionEvents { section_id, track_id, events } => {
            h.timeline.replace_section_track_events(&section_id, &track_id, events);
            build_status_response(&h)
        }
        IpcCommand::AddNote { note_on_id, note_off_id, track_id, tick, duration_ticks, note, velocity } => {
            h.timeline.add_note(note_on_id, note_off_id, &track_id, tick, duration_ticks, note, velocity);
            build_status_response(&h)
        }
        IpcCommand::DeleteNoteAt { track_id, tick, note } => {
            h.timeline.delete_note_at(&track_id, tick, note);
            build_status_response(&h)
        }
        IpcCommand::MoveEvent { event_id, new_tick } => {
            h.timeline.move_event(event_id, new_tick);
            build_status_response(&h)
        }
        IpcCommand::RemoveEvent { event_id } => {
            h.timeline.remove_event(event_id);
            build_status_response(&h)
        }
        IpcCommand::HumanizeTrack { track_id, section_id, timing_drift_ticks, drag_bias_ticks, velocity_jitter } => {
            h.timeline.humanize_track(&track_id, section_id.as_deref(), timing_drift_ticks, drag_bias_ticks, velocity_jitter);
            build_status_response(&h)
        }
        IpcCommand::LoadProject { path } => {
            match h.load_project_from_file(&path) {
                Ok(()) => build_status_response(&h),
                Err(e) => IpcResponse {
                    ok: false,
                    status: Some(build_status_struct(&h)),
                    error: Some(format!("Failed to load project: {}", e)),
                },
            }
        }
        IpcCommand::SaveProject { path } => {
            match h.save_project_to_file(&path) {
                Ok(()) => build_status_response(&h),
                Err(e) => IpcResponse {
                    ok: false,
                    status: Some(build_status_struct(&h)),
                    error: Some(format!("Failed to save project: {}", e)),
                },
            }
        }
        IpcCommand::NewProject { title, bpm } => {
            h.new_project(&title, bpm);
            build_status_response(&h)
        }
        IpcCommand::AddTrack { id, name, track_type } => {
            h.add_custom_track(&id, &name, &track_type);
            build_status_response(&h)
        }
        IpcCommand::RenameTrack { track_id, new_name } => {
            h.rename_track(&track_id, &new_name);
            build_status_response(&h)
        }
        IpcCommand::SetTrackParam { track_id, param_name, value } => {
            if let Some(&idx) = h.track_map.get(&track_id) {
                h.tracks[idx].set_parameter(&param_name, value);
            }
            build_status_response(&h)
        }
        IpcCommand::SaveTrackPreset { track_id, preset_name } => {
            let filename = if preset_name.ends_with(".json") {
                preset_name
            } else {
                format!("{}.json", preset_name)
            };
            let path = std::path::Path::new("presets").join(filename);
            if let Some(&idx) = h.track_map.get(&track_id) {
                let _ = h.tracks[idx].save_patch_to_file(&path);
            }
            build_status_response(&h)
        }
        IpcCommand::LoadTrackPreset { track_id, preset_name } => {
            let filename = if preset_name.ends_with(".json") {
                preset_name.clone()
            } else {
                format!("{}.json", preset_name)
            };
            let path = std::path::Path::new("presets").join(&filename);
            let fallback_path = std::path::Path::new("jamsynth/presets").join(&filename);
            if let Some(&idx) = h.track_map.get(&track_id) {
                if path.exists() {
                    let _ = h.tracks[idx].load_patch_from_file(&path);
                } else if fallback_path.exists() {
                    let _ = h.tracks[idx].load_patch_from_file(&fallback_path);
                }
            }
            build_status_response(&h)
        }
        IpcCommand::SwitchTrackPlugin { track_id, plugin_uri_or_type, plugin_name } => {
            h.switch_track_instrument(&track_id, &plugin_uri_or_type, &plugin_name);
            build_status_response(&h)
        }
        IpcCommand::AddTrackInsert { track_id, plugin_uri_or_path, plugin_name } => {
            h.add_track_insert(&track_id, &plugin_uri_or_path, &plugin_name);
            build_status_response(&h)
        }
        IpcCommand::RemoveTrackInsert { track_id, insert_index } => {
            h.remove_track_insert(&track_id, insert_index);
            build_status_response(&h)
        }
        IpcCommand::AddMasterInsert { plugin_uri_or_path, plugin_name } => {
            h.add_master_insert(&plugin_uri_or_path, &plugin_name);
            build_status_response(&h)
        }
        IpcCommand::RemoveMasterInsert { insert_index } => {
            h.remove_master_insert(insert_index);
            build_status_response(&h)
        }
        IpcCommand::SetMasterInsertParam { insert_index, param_name, value } => {
            if insert_index < h.master_bus.inserts.len() {
                h.master_bus.inserts[insert_index].set_parameter(&param_name, value);
            }
            build_status_response(&h)
        }
        IpcCommand::SetMasterPan { pan } => {
            h.master_bus.pan = pan;
            build_status_response(&h)
        }
        IpcCommand::AddAuxBus { id, name } => {
            h.add_aux_bus(&id, &name);
            build_status_response(&h)
        }
        IpcCommand::RemoveAuxBus { bus_id } => {
            h.remove_aux_bus(&bus_id);
            build_status_response(&h)
        }
        IpcCommand::AddAuxInsert { bus_id, plugin_uri_or_path, plugin_name } => {
            h.add_aux_insert(&bus_id, &plugin_uri_or_path, &plugin_name);
            build_status_response(&h)
        }
        IpcCommand::RemoveAuxInsert { bus_id, insert_index } => {
            h.remove_aux_insert(&bus_id, insert_index);
            build_status_response(&h)
        }
        IpcCommand::SetAuxInsertParam { bus_id, insert_index, param_name, value } => {
            if let Some(&idx) = h.aux_map.get(&bus_id) {
                let bus = &mut h.aux_buses[idx];
                if insert_index < bus.inserts.len() {
                    bus.inserts[insert_index].set_parameter(&param_name, value);
                }
            }
            build_status_response(&h)
        }
        IpcCommand::SetTrackAuxSend { track_id, bus_id, gain, is_pre_fader } => {
            h.set_track_aux_send(&track_id, &bus_id, gain, is_pre_fader);
            build_status_response(&h)
        }
        IpcCommand::RemoveTrackAuxSend { track_id, bus_id } => {
            h.remove_track_aux_send(&track_id, &bus_id);
            build_status_response(&h)
        }
        IpcCommand::DeleteTrack { track_id } => {
            h.delete_track(&track_id);
            build_status_response(&h)
        }
        IpcCommand::ReorderTrack { from, to } => {
            h.reorder_tracks(from, to);
            build_status_response(&h)
        }
        IpcCommand::AddAutomationPoint { id, tick, track_id, param_name, value } => {
            h.timeline.add_event(ScheduledEvent::new(
                id,
                tick,
                &track_id,
                crate::timeline::ScheduledAction::SetParam { param_name, value },
            ));
            build_status_response(&h)
        }
        IpcCommand::ListMidiDevices => build_status_response(&h),
        IpcCommand::ConnectMidiDevice { port_index } => {
            let _ = h.connect_midi_device(port_index);
            build_status_response(&h)
        }
        IpcCommand::DisconnectMidiDevice => {
            h.disconnect_midi_device();
            build_status_response(&h)
        }
        IpcCommand::SetArmedTrack { track_id } => {
            h.set_armed_track(track_id);
            build_status_response(&h)
        }
        IpcCommand::SetInsertSidechain { track_id, insert_index, source_track_id, gain } => {
            h.set_insert_sidechain(&track_id, insert_index, &source_track_id, gain);
            build_status_response(&h)
        }
        IpcCommand::RemoveInsertSidechain { track_id, insert_index } => {
            h.remove_insert_sidechain(&track_id, insert_index);
            build_status_response(&h)
        }
        IpcCommand::FreezeTrack { track_id } => {
            let _ = h.freeze_track(&track_id);
            build_status_response(&h)
        }
        IpcCommand::UnfreezeTrack { track_id } => {
            let _ = h.unfreeze_track(&track_id);
            build_status_response(&h)
        }
        IpcCommand::BounceTrack { track_id, new_track_name } => {
            let _ = h.bounce_track(&track_id, &new_track_name);
            build_status_response(&h)
        }
        IpcCommand::RescanPlugins => {
            h.rescan_plugins();
            build_status_response(&h)
        }
        IpcCommand::GetStatus => build_status_response(&h),
    }
}

fn build_status_struct(h: &JamSeqHost) -> IpcStatusResponse {
    let track_statuses: Vec<TrackStatus> = h
        .tracks
        .iter()
        .map(|t| {
            let params = t
                .get_parameters()
                .into_iter()
                .map(|(name, value, min, max)| TrackParamStatus { name, value, min, max })
                .collect();

            let inserts = t
                .inserts
                .iter()
                .map(|ins| TrackInsertStatus {
                    name: ins.name().to_string(),
                    uri: ins.uri().to_string(),
                    params: ins
                        .get_parameters()
                        .into_iter()
                        .map(|(name, value, min, max)| TrackParamStatus { name, value, min, max })
                        .collect(),
                })
                .collect();

            let sends = t
                .sends
                .iter()
                .map(|s| AuxSendStatus {
                    target_bus_id: s.target_bus_id.clone(),
                    gain: s.gain,
                    is_pre_fader: s.is_pre_fader,
                    is_muted: s.is_muted,
                })
                .collect();

            let sidechains = t
                .sidechains
                .iter()
                .map(|sc| SidechainSendStatus {
                    target_track_id: sc.target_track_id.clone(),
                    target_insert_index: sc.target_insert_index,
                    gain: sc.gain,
                })
                .collect();

            TrackStatus {
                id: t.id.clone(),
                name: t.name.clone(),
                volume: t.volume,
                pan: t.pan,
                muted: t.muted,
                solo: t.solo,
                is_frozen: t.is_frozen,
                peak_level: t.peak_level,
                params,
                inserts,
                sends,
                sidechains,
                current_plugin: t.current_plugin.clone(),
            }
        })
        .collect();

    let master_inserts = h
        .master_bus
        .inserts
        .iter()
        .map(|ins| TrackInsertStatus {
            name: ins.name().to_string(),
            uri: ins.uri().to_string(),
            params: ins
                .get_parameters()
                .into_iter()
                .map(|(name, value, min, max)| TrackParamStatus { name, value, min, max })
                .collect(),
        })
        .collect();

    let aux_buses = h
        .aux_buses
        .iter()
        .map(|bus| {
            let inserts = bus
                .inserts
                .iter()
                .map(|ins| TrackInsertStatus {
                    name: ins.name().to_string(),
                    uri: ins.uri().to_string(),
                    params: ins
                        .get_parameters()
                        .into_iter()
                        .map(|(name, value, min, max)| TrackParamStatus { name, value, min, max })
                        .collect(),
                })
                .collect();
            AuxBusStatus {
                id: bus.id.clone(),
                name: bus.name.clone(),
                volume: bus.volume,
                pan: bus.pan,
                muted: bus.muted,
                solo: bus.solo,
                peak_level: bus.peak_level,
                inserts,
            }
        })
        .collect();

    let active_section = h.timeline.get_section_at_tick(h.timeline.current_tick).map(|s| s.id.clone());

    IpcStatusResponse {
        project_title: h.timeline.title.clone(),
        project_path: h.project_path.clone(),
        is_playing: h.timeline.is_playing,
        current_tick: h.timeline.current_tick,
        current_sample: h.timeline.current_sample,
        total_ticks: h.timeline.total_ticks,
        bpm: h.timeline.bpm,
        master_volume: h.master_gain,
        master_peak: h.master_peak,
        master_inserts,
        aux_buses,
        armed_track_id: h.armed_track_id.clone(),
        midi_devices: h.list_midi_devices(),
        is_recording: h.recorder.is_recording(),
        mic_peak: h.recorder.get_peak_input(),
        active_section_id: active_section,
        sections: h.timeline.sections.clone(),
        tracks: track_statuses,
        events: h.timeline.events.clone(),
        available_plugins: h.available_plugins.clone(),
    }
}

fn build_status_response(h: &JamSeqHost) -> IpcResponse {
    IpcResponse {
        ok: true,
        status: Some(build_status_struct(h)),
        error: None,
    }
}
