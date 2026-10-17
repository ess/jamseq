use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PPQ: u64 = 960;
pub const TICKS_PER_16TH: u64 = PPQ / 4; // 240 ticks per 16th note

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScheduledAction {
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    ControlChange { cc: u8, value: u8 },
    SetParam { param_name: String, value: f32 },
    LoadPatchJson { json: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutomationPoint {
    pub tick: u64,
    pub param_name: String,
    pub value: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TimeSignature {
    pub numerator: u32,
    pub denominator: u32,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    pub fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator: numerator.max(1),
            denominator: denominator.max(1),
        }
    }

    pub fn ticks_per_bar(&self) -> u64 {
        // numerator beats per bar, each beat is PPQ * 4 / denominator
        (self.numerator as u64 * PPQ * 4) / self.denominator.max(1) as u64
    }

    pub fn ticks_per_beat(&self) -> u64 {
        (PPQ * 4) / self.denominator.max(1) as u64
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum TempoOverride {
    AbsoluteBpm(f64),
    Ratio(u32, u32),
}

impl TempoOverride {
    pub fn compute(&self, parent_bpm: f64) -> f64 {
        match self {
            Self::AbsoluteBpm(bpm) => *bpm,
            Self::Ratio(num, den) => {
                if *den == 0 {
                    parent_bpm
                } else {
                    parent_bpm * (*num as f64 / *den as f64)
                }
            }
        }
    }
}

/// An individual musical block / clip assigned to a specific track inside a section.
/// Follows REAPER-style item orientation within a region: plays once by default
/// at its specified start offset relative to the section downbeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackBlock {
    pub id: String,
    pub track_id: String,
    #[serde(default)]
    pub section_id: Option<String>,
    #[serde(default)]
    pub start_offset_ticks: u64,
    #[serde(default)]
    pub length_ticks: u64,
    #[serde(default)]
    pub tempo: Option<TempoOverride>,
    #[serde(default)]
    pub time_signature: Option<TimeSignature>,
    #[serde(default)]
    pub events: Vec<ScheduledEvent>,
    #[serde(default)]
    pub audio_file: Option<String>,
}

impl TrackBlock {
    pub fn new(id: &str, track_id: &str, length_ticks: u64) -> Self {
        Self {
            id: id.to_string(),
            track_id: track_id.to_string(),
            section_id: None,
            start_offset_ticks: 0,
            length_ticks,
            tempo: None,
            time_signature: None,
            events: Vec::new(),
            audio_file: None,
        }
    }

    pub fn new_section(id: &str, track_id: &str, section_id: &str, length_ticks: u64) -> Self {
        Self {
            id: id.to_string(),
            track_id: track_id.to_string(),
            section_id: Some(section_id.to_string()),
            start_offset_ticks: 0,
            length_ticks,
            tempo: None,
            time_signature: None,
            events: Vec::new(),
            audio_file: None,
        }
    }

    pub fn new_timeline(id: &str, track_id: &str, start_tick: u64, length_ticks: u64) -> Self {
        Self {
            id: id.to_string(),
            track_id: track_id.to_string(),
            section_id: None,
            start_offset_ticks: start_tick,
            length_ticks,
            tempo: None,
            time_signature: None,
            events: Vec::new(),
            audio_file: None,
        }
    }

    pub fn start_tick(&self) -> u64 {
        self.start_offset_ticks
    }

    pub fn length_ticks(&self) -> u64 {
        self.length_ticks
    }

    pub fn end_tick(&self) -> u64 {
        self.start_offset_ticks + self.length_ticks
    }

    pub fn with_offset(mut self, offset_ticks: u64) -> Self {
        self.start_offset_ticks = offset_ticks;
        self
    }

    pub fn with_tempo(mut self, tempo: TempoOverride) -> Self {
        self.tempo = Some(tempo);
        self
    }

    pub fn with_time_signature(mut self, ts: TimeSignature) -> Self {
        self.time_signature = Some(ts);
        self
    }

    pub fn effective_bpm(&self, section_bpm: f64) -> f64 {
        match &self.tempo {
            Some(t) => t.compute(section_bpm),
            None => section_bpm,
        }
    }

    pub fn effective_time_signature(&self, section_ts: TimeSignature) -> TimeSignature {
        self.time_signature.unwrap_or(section_ts)
    }

    pub fn effective_length_ticks(&self) -> u64 {
        self.length_ticks
    }

    /// End tick boundary within the section (start_offset_ticks + effective_length_ticks)
    pub fn effective_end_tick(&self) -> u64 {
        self.start_offset_ticks + self.length_ticks
    }

    /// Returns all (start_tick, end_tick) intervals for notes of the given pitch in this block.
    pub fn note_intervals(&self, pitch: u8) -> Vec<(u64, u64)> {
        let mut intervals = Vec::new();
        for (i, ev) in self.events.iter().enumerate() {
            if let ScheduledAction::NoteOn { note, .. } = ev.action {
                if note == pitch {
                    let end_tick = self.events.iter().skip(i + 1).find(|off_ev| {
                        off_ev.tick > ev.tick
                            && match off_ev.action {
                                ScheduledAction::NoteOff { note: off_n } => off_n == note,
                                _ => false,
                            }
                    }).map(|off_ev| off_ev.tick).unwrap_or(ev.tick + 240);
                    intervals.push((ev.tick, end_tick.max(ev.tick + 60)));
                }
            }
        }
        intervals.sort_by_key(|(s, _)| *s);
        intervals
    }

    /// Check if a tick falls inside an existing note of the given pitch.
    pub fn has_note_at(&self, pitch: u8, tick: u64) -> bool {
        self.note_intervals(pitch).into_iter().any(|(start, end)| tick >= start && tick < end)
    }

    /// Returns the tick of the next note start on this pitch strictly after `from_tick`, if any.
    pub fn next_note_start(&self, pitch: u8, from_tick: u64) -> Option<u64> {
        self.note_intervals(pitch).into_iter().find(|(start, _)| *start > from_tick).map(|(start, _)| start)
    }

    pub fn add_note(
        &mut self,
        note_on_id: u64,
        note_off_id: u64,
        tick: u64,
        duration_ticks: u64,
        note: u8,
        velocity: f32,
    ) {
        // Prevent overlapping notes on the same pitch:
        if self.has_note_at(note, tick) {
            return;
        }
        let eff_duration = if let Some(next_start) = self.next_note_start(note, tick) {
            duration_ticks.min(next_start.saturating_sub(tick))
        } else {
            duration_ticks
        };
        if eff_duration == 0 {
            return;
        }

        let on = ScheduledEvent::new(note_on_id, tick, &self.track_id, ScheduledAction::NoteOn { note, velocity });
        let off = ScheduledEvent::new(note_off_id, tick + eff_duration, &self.track_id, ScheduledAction::NoteOff { note });
        self.events.push(on);
        self.events.push(off);
        self.events.sort_by_key(|e| e.tick);
    }
}

/// Track Arranger Mode distinguishing between Grid-based section looping
/// and Ad-hoc free-floating timeline tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackArrangerMode {
    Section,  // Grid-based, bound to song sections
    Timeline, // Ad-hoc, free-floating across the timeline ruler
}

impl Default for TrackArrangerMode {
    fn default() -> Self {
        TrackArrangerMode::Section
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongSection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub time_signature: Option<TimeSignature>,
    #[serde(default)]
    pub tempo: Option<TempoOverride>,
    #[serde(default)]
    pub track_blocks: HashMap<String, TrackBlock>,

    // Backward-compatibility fallback fields:
    #[serde(default)]
    pub start_tick: u64,
    #[serde(default)]
    pub length_ticks: u64,
    #[serde(default)]
    pub start_bar: u32,
    #[serde(default)]
    pub bars: u32,
}

impl SongSection {
    pub fn new(id: &str, name: &str, start_bar: u32, bars: u32) -> Self {
        let ts = TimeSignature::default();
        let tpb = ts.ticks_per_bar();
        let start_tick = if start_bar > 0 { (start_bar as u64 - 1) * tpb } else { 0 };
        let length_ticks = bars as u64 * tpb;
        Self {
            id: id.to_string(),
            name: name.to_string(),
            time_signature: None,
            tempo: None,
            track_blocks: HashMap::new(),
            start_tick,
            length_ticks,
            start_bar,
            bars,
        }
    }

    pub fn new_empty(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            time_signature: None,
            tempo: None,
            track_blocks: HashMap::new(),
            start_tick: 0,
            length_ticks: 0,
            start_bar: 0,
            bars: 0,
        }
    }

    pub fn from_ticks(id: &str, name: &str, start_tick: u64, length_ticks: u64) -> Self {
        let ts = TimeSignature::default();
        let tpb = ts.ticks_per_bar().max(1);
        let start_bar = (start_tick / tpb) as u32 + 1;
        let bars = (length_ticks as f64 / tpb as f64).ceil() as u32;
        Self {
            id: id.to_string(),
            name: name.to_string(),
            time_signature: None,
            tempo: None,
            track_blocks: HashMap::new(),
            start_tick,
            length_ticks,
            start_bar,
            bars,
        }
    }

    pub fn with_tempo(mut self, tempo: TempoOverride) -> Self {
        self.tempo = Some(tempo);
        self
    }

    pub fn with_time_signature(mut self, ts: TimeSignature) -> Self {
        self.time_signature = Some(ts);
        self
    }

    pub fn time_signature(&self) -> TimeSignature {
        self.time_signature.unwrap_or_default()
    }

    pub fn effective_time_signature(&self, project_ts: TimeSignature) -> TimeSignature {
        self.time_signature.unwrap_or(project_ts)
    }

    pub fn ticks_per_bar(&self) -> u64 {
        self.time_signature().ticks_per_bar()
    }

    pub fn with_block(mut self, block: TrackBlock) -> Self {
        self.track_blocks.insert(block.track_id.clone(), block);
        self
    }

    pub fn set_block(&mut self, block: TrackBlock) {
        self.track_blocks.insert(block.track_id.clone(), block);
    }

    pub fn remove_block(&mut self, track_id: &str) -> Option<TrackBlock> {
        self.track_blocks.remove(track_id)
    }

    pub fn effective_bpm(&self, project_bpm: f64) -> f64 {
        match &self.tempo {
            Some(t) => t.compute(project_bpm),
            None => project_bpm,
        }
    }

    pub fn start_tick(&self) -> u64 {
        if self.start_tick > 0 || self.length_ticks > 0 {
            self.start_tick
        } else if self.start_bar > 0 {
            (self.start_bar as u64 - 1) * self.ticks_per_bar()
        } else {
            0
        }
    }

    /// Content-Driven Length: calculated purely on-the-fly from the furthest-reaching track block.
    /// An empty section has length 0 and is not stored as a static field.
    pub fn length_ticks(&self) -> u64 {
        if !self.track_blocks.is_empty() {
            self.track_blocks
                .values()
                .map(|b| b.effective_end_tick())
                .max()
                .unwrap_or(0)
        } else if self.length_ticks > 0 {
            self.length_ticks
        } else if self.bars > 0 {
            self.bars as u64 * self.ticks_per_bar()
        } else {
            0
        }
    }

    pub fn end_tick(&self) -> u64 {
        self.start_tick() + self.length_ticks()
    }

    /// Calculate (1-indexed bar, 1-indexed beat within bar)
    pub fn start_bar_beat(&self) -> (u32, f32) {
        let ts = self.time_signature();
        let tpb = ts.ticks_per_bar().max(1);
        let ticks = self.start_tick();
        let bar = (ticks / tpb) as u32 + 1;
        let rem_ticks = ticks % tpb;
        let ticks_per_beat = ts.ticks_per_beat().max(1);
        let beat = (rem_ticks as f32 / ticks_per_beat as f32) + 1.0;
        (bar, beat)
    }

    pub fn start_bar_beat_with_project_ts(&self, project_ts: TimeSignature) -> (u32, f32) {
        let ts = self.effective_time_signature(project_ts);
        let tpb = ts.ticks_per_bar().max(1);
        let ticks = self.start_tick();
        let bar = (ticks / tpb) as u32 + 1;
        let rem_ticks = ticks % tpb;
        let ticks_per_beat = ts.ticks_per_beat().max(1);
        let beat = (rem_ticks as f32 / ticks_per_beat as f32) + 1.0;
        (bar, beat)
    }

    /// Snap boundaries to nearest quarter-note beat (PPQ = 480 ticks)
    pub fn normalize_to_beat(&mut self) {
        let ticks_per_beat = PPQ;
        self.start_tick = ((self.start_tick() + ticks_per_beat / 2) / ticks_per_beat) * ticks_per_beat;
        let len = self.length_ticks();
        self.length_ticks = (((len + ticks_per_beat / 2) / ticks_per_beat).max(1)) * ticks_per_beat;
        self.recompute_bars();
    }

    /// Snap boundaries to full bar
    pub fn normalize_to_bar(&mut self) {
        let tpb = self.ticks_per_bar().max(1);
        self.start_tick = ((self.start_tick() + tpb / 2) / tpb) * tpb;
        let len = self.length_ticks();
        self.length_ticks = (((len + tpb / 2) / tpb).max(1)) * tpb;
        self.recompute_bars();
    }

    fn recompute_bars(&mut self) {
        let tpb = self.ticks_per_bar().max(1);
        let len = self.length_ticks();
        self.start_bar = (self.start_tick / tpb) as u32 + 1;
        self.bars = (len as f64 / tpb as f64).ceil() as u32;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduledEvent {
    pub id: u64,
    pub tick: u64,
    pub track_id: String,
    pub action: ScheduledAction,
}

impl ScheduledEvent {
    pub fn new(id: u64, tick: u64, track_id: &str, action: ScheduledAction) -> Self {
        Self {
            id,
            tick,
            track_id: track_id.to_string(),
            action,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InsertConfig {
    pub name: String,
    pub uri: String,
    #[serde(default)]
    pub parameters: HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuxSendConfig {
    pub target_bus_id: String,
    pub gain: f32,
    #[serde(default)]
    pub is_pre_fader: bool,
    #[serde(default)]
    pub is_muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SidechainSendConfig {
    pub target_track_id: String,
    pub target_insert_index: usize,
    pub gain: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_track_type")]
    pub track_type: String,
    #[serde(default)]
    pub arranger_mode: TrackArrangerMode,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub current_plugin: Option<String>,
    #[serde(default)]
    pub parameters: HashMap<String, f32>,
    #[serde(default)]
    pub inserts: Vec<InsertConfig>,
    #[serde(default)]
    pub sends: Vec<AuxSendConfig>,
    #[serde(default)]
    pub sidechains: Vec<SidechainSendConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MasterBusConfig {
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub inserts: Vec<InsertConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuxBusConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub inserts: Vec<InsertConfig>,
}

fn default_track_type() -> String {
    "synth".to_string()
}

fn default_volume() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineArrangement {
    pub title: String,
    pub bpm: f64,
    #[serde(default)]
    pub time_signature: TimeSignature,
    pub total_bars: u32,
    pub loop_enabled: bool,
    #[serde(default)]
    pub tracks: Vec<TrackConfig>,
    #[serde(default)]
    pub master_bus: Option<MasterBusConfig>,
    #[serde(default)]
    pub aux_buses: Vec<AuxBusConfig>,
    pub sections: Vec<SongSection>,
    #[serde(default)]
    pub timeline_blocks: Vec<TrackBlock>,
    #[serde(skip_serializing, default)]
    pub events: Vec<ScheduledEvent>,
}

impl Default for TimelineArrangement {
    fn default() -> Self {
        Self {
            title: "Untitled".to_string(),
            bpm: 120.0,
            time_signature: TimeSignature::default(),
            total_bars: 0,
            loop_enabled: true,
            tracks: Vec::new(),
            master_bus: None,
            aux_buses: Vec::new(),
            sections: Vec::new(),
            timeline_blocks: Vec::new(),
            events: Vec::new(),
        }
    }
}

impl TimelineArrangement {
    /// Auto-discovers tracks referenced in sections or events if the tracks array is empty
    pub fn discover_tracks_from_content(&mut self) {
        if !self.tracks.is_empty() {
            return;
        }

        let mut discovered_ids = Vec::new();
        for sec in &self.sections {
            for (t_id, _) in &sec.track_blocks {
                if !discovered_ids.contains(t_id) {
                    discovered_ids.push(t_id.clone());
                }
            }
        }
        for tb in &self.timeline_blocks {
            if !discovered_ids.contains(&tb.track_id) {
                discovered_ids.push(tb.track_id.clone());
            }
        }
        for ev in &self.events {
            if !discovered_ids.contains(&ev.track_id) {
                discovered_ids.push(ev.track_id.clone());
            }
        }

        for id in discovered_ids {
            let name = if id.starts_with("track_") {
                let num = id.strip_prefix("track_").unwrap_or("1");
                format!("MIDI Track {}", num)
            } else if id.starts_with("audio_") {
                let num = id.strip_prefix("audio_").unwrap_or("1");
                format!("Audio Track {}", num)
            } else {
                let mut c = id.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            };
            let track_type = if id.starts_with("audio_") || id.starts_with("mic_") {
                "audio".to_string()
            } else {
                "synth".to_string()
            };
            self.tracks.push(TrackConfig {
                id,
                name,
                track_type,
                arranger_mode: TrackArrangerMode::Section,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                current_plugin: None,
                parameters: HashMap::new(),
                inserts: Vec::new(),
                sends: Vec::new(),
                sidechains: Vec::new(),
            });
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct TrackPlaybackCursor {
    pub current_tick: f64,
}

impl TrackPlaybackCursor {
    pub fn new(start_tick: f64) -> Self {
        Self { current_tick: start_tick }
    }

    pub fn advance(&mut self, samples: usize, effective_bpm: f64, sample_rate: f32) -> (u64, u64) {
        let start_tick = self.current_tick as u64;
        let delta_ticks = (samples as f64 * effective_bpm * PPQ as f64) / (60.0 * sample_rate as f64);
        self.current_tick += delta_ticks;
        let end_tick = self.current_tick as u64;
        (start_tick, end_tick)
    }
}

pub struct Timeline {
    pub title: String,
    pub bpm: f64,
    pub time_signature: TimeSignature,
    pub sample_rate: f64,
    pub current_sample: u64,
    pub current_tick: u64,
    pub current_sub_tick: f64,
    pub total_ticks: u64,
    pub loop_enabled: bool,
    pub loop_start_tick: u64,
    pub loop_end_tick: u64,
    pub is_playing: bool,
    pub sections: Vec<SongSection>,
    pub timeline_blocks: Vec<TrackBlock>,
    pub events: Vec<ScheduledEvent>,
    pub active_events: Vec<ScheduledEvent>,
    pub track_cursors: HashMap<String, TrackPlaybackCursor>,

    event_cursor: usize,
}

impl Timeline {
    pub fn new(bpm: f64, sample_rate: f64) -> Self {
        let mut t = Self {
            title: "Untitled".to_string(),
            bpm: if bpm > 10.0 { bpm } else { 120.0 },
            time_signature: TimeSignature::default(),
            sample_rate: if sample_rate > 0.0 { sample_rate } else { 44100.0 },
            current_sample: 0,
            current_tick: 0,
            current_sub_tick: 0.0,
            total_ticks: 0,
            loop_enabled: true,
            loop_start_tick: 0,
            loop_end_tick: 0,
            is_playing: false,
            sections: Vec::new(),
            timeline_blocks: Vec::new(),
            events: Vec::new(),
            active_events: Vec::new(),
            track_cursors: HashMap::new(),
            event_cursor: 0,
        };
        t.rebuild_active_events();
        t
    }

    pub fn samples_to_ticks(&self, samples: u64) -> u64 {
        let seconds = samples as f64 / self.sample_rate;
        let beats = seconds * (self.bpm / 60.0);
        (beats * PPQ as f64) as u64
    }

    pub fn ticks_to_samples(&self, ticks: u64) -> u64 {
        let beats = ticks as f64 / PPQ as f64;
        let seconds = beats / (self.bpm / 60.0);
        (seconds * self.sample_rate) as u64
    }

    pub fn load_arrangement(&mut self, arrangement: TimelineArrangement) {
        self.title = arrangement.title;
        self.bpm = arrangement.bpm;
        self.time_signature = arrangement.time_signature;
        self.sections = arrangement.sections;
        for sec in &mut self.sections {
            for b in sec.track_blocks.values_mut() {
                b.section_id = Some(sec.id.clone());
            }
        }
        self.timeline_blocks = arrangement.timeline_blocks;
        for b in &mut self.timeline_blocks {
            b.section_id = None;
        }
        let computed = self.computed_total_ticks();
        self.total_ticks = if computed > 0 {
            computed
        } else {
            arrangement.total_bars as u64 * 4 * PPQ
        };
        self.loop_enabled = arrangement.loop_enabled;
        self.loop_start_tick = 0;
        self.loop_end_tick = self.total_ticks;

        let mut events = arrangement.events;
        // Sort chronologically
        events.sort_by_key(|e| e.tick);
        self.events = events.clone();

        // If track blocks have no events but arrangement has flat events, migrate flat events into section track blocks
        let blocks_have_events = self.sections.iter().any(|s| s.track_blocks.values().any(|b| !b.events.is_empty()));
        if !blocks_have_events && !events.is_empty() && !self.sections.is_empty() {
            let mut sec_accum = 0u64;
            for (idx, sec) in self.sections.iter_mut().enumerate() {
                let sec_len = if sec.length_ticks > 0 {
                    sec.length_ticks
                } else if sec.bars > 0 {
                    sec.bars as u64 * sec.ticks_per_bar()
                } else if sec.length_ticks() > 0 {
                    sec.length_ticks()
                } else {
                    4 * PPQ
                };
                let sec_start = if idx == 0 {
                    0
                } else if sec.start_tick >= sec_accum {
                    sec.start_tick
                } else {
                    sec_accum
                };
                let sec_end = sec_start + sec_len;

                for ev in &events {
                    if ev.tick >= sec_start && ev.tick < sec_end {
                        let block = sec.track_blocks.entry(ev.track_id.clone()).or_insert_with(|| {
                            TrackBlock::new(&format!("{}_{}", sec.id, ev.track_id), &ev.track_id, sec_len)
                        });
                        block.length_ticks = block.length_ticks.max(sec_len);
                        let mut local_ev = ev.clone();
                        local_ev.tick = (ev.tick.saturating_sub(sec_start)).saturating_sub(block.start_offset_ticks);
                        block.events.push(local_ev);
                    }
                }
                sec_accum = sec_end;
            }
            for sec in &mut self.sections {
                for block in sec.track_blocks.values_mut() {
                    block.events.sort_by_key(|e| e.tick);
                    if let Some(max_tick) = block.events.iter().map(|e| e.tick).max() {
                        if max_tick >= block.length_ticks {
                            block.length_ticks = ((max_tick / 240) + 1) * 240;
                        }
                    }
                }
            }
            let computed = self.computed_total_ticks();
            if computed > 0 {
                self.total_ticks = computed;
                self.loop_end_tick = self.total_ticks;
            }
        }
        self.events.clear();

        self.rebuild_active_events();
        self.reset();
    }

    /// Export the current live state into a TimelineArrangement for saving
    pub fn to_arrangement(&self) -> TimelineArrangement {
        let total_bars = (self.total_ticks / (4 * PPQ)) as u32;
        TimelineArrangement {
            title: self.title.clone(),
            bpm: self.bpm,
            time_signature: self.time_signature,
            total_bars,
            loop_enabled: self.loop_enabled,
            tracks: Vec::new(),
            master_bus: None,
            aux_buses: Vec::new(),
            sections: self.sections.clone(),
            timeline_blocks: self.timeline_blocks.clone(),
            events: Vec::new(),
        }
    }

    /// Replace all scheduled events for a specific track within a specific section's tick window.
    /// New events should have their ticks already positioned in absolute project ticks.
    pub fn replace_section_track_events(&mut self, section_id: &str, track_id: &str, new_events: Vec<ScheduledEvent>) -> bool {
        let section = match self.sections.iter().find(|s| s.id == section_id) {
            Some(s) => s.clone(),
            None => return false,
        };

        let start = section.start_tick();
        let end = section.end_tick();

        // Retain events that are outside this section OR for other tracks
        self.events.retain(|e| {
            !(e.track_id == track_id && e.tick >= start && e.tick < end)
        });

        // Insert new events
        for ev in new_events {
            if ev.tick >= start && ev.tick < end {
                self.events.push(ev);
            }
        }

        // Re-sort
        self.events.sort_by_key(|e| e.tick);
        // Refresh cursor position
        self.seek_tick(self.current_tick);
        true
    }

    /// Granular: Add a single scheduled event to the timeline
    pub fn add_event(&mut self, event: ScheduledEvent) {
        self.events.push(event);
        self.events.sort_by_key(|e| e.tick);
        self.seek_tick(self.current_tick);
    }

    /// Granular: Remove an event by unique event ID
    pub fn remove_event(&mut self, event_id: u64) -> bool {
        let prev_len = self.events.len();
        self.events.retain(|e| e.id != event_id);
        if self.events.len() != prev_len {
            self.seek_tick(self.current_tick);
            true
        } else {
            false
        }
    }

    /// Granular: Move an event to a new tick
    pub fn move_event(&mut self, event_id: u64, new_tick: u64) -> bool {
        if let Some(pos) = self.events.iter().position(|e| e.id == event_id) {
            self.events[pos].tick = new_tick;
            self.events.sort_by_key(|e| e.tick);
            self.seek_tick(self.current_tick);
            true
        } else {
            false
        }
    }

    /// Granular musical primitive: Insert a complete note (NoteOn + NoteOff pair)
    pub fn add_note(
        &mut self,
        note_on_id: u64,
        note_off_id: u64,
        track_id: &str,
        tick: u64,
        duration_ticks: u64,
        note: u8,
        velocity: f32,
    ) {
        let on = ScheduledEvent::new(note_on_id, tick, track_id, ScheduledAction::NoteOn { note, velocity });
        let off = ScheduledEvent::new(note_off_id, tick + duration_ticks, track_id, ScheduledAction::NoteOff { note });
        self.events.push(on);
        self.events.push(off);
        self.events.sort_by_key(|e| e.tick);
        self.seek_tick(self.current_tick);
    }

    /// Granular musical primitive: Delete note at tick
    pub fn delete_note_at(&mut self, track_id: &str, tick: u64, note: u8) -> bool {
        let prev_len = self.events.len();
        self.events.retain(|e| {
            if e.track_id == track_id {
                match e.action {
                    ScheduledAction::NoteOn { note: n, .. } if n == note && (e.tick as i64 - tick as i64).abs() < 120 => false,
                    _ => true,
                }
            } else {
                true
            }
        });
        if self.events.len() != prev_len {
            self.seek_tick(self.current_tick);
            true
        } else {
            false
        }
    }

    /// Humanize: apply micro-timing drift, drag bias (laying back), and velocity jitter
    pub fn humanize_track(
        &mut self,
        track_id: &str,
        section_id: Option<&str>,
        timing_drift_ticks: i64,
        drag_bias_ticks: i64,
        velocity_jitter: f32,
    ) {
        let (start, end) = if let Some(sec_id) = section_id {
            if let Some(sec) = self.sections.iter().find(|s| s.id == sec_id) {
                (sec.start_tick(), sec.end_tick())
            } else {
                (0, self.total_ticks)
            }
        } else {
            (0, self.total_ticks)
        };

        // Pseudo-random deterministic generator based on event id and tick
        for ev in &mut self.events {
            if ev.track_id == track_id && ev.tick >= start && ev.tick < end {
                let pseudo_hash = ((ev.id.wrapping_mul(6364136223846793005) ^ ev.tick) % 1000) as f32 / 1000.0;
                let centered_rnd = pseudo_hash * 2.0 - 1.0; // -1.0 to 1.0

                // Timing offset = centered random drift + constant drag bias (laying back behind the beat)
                let offset = (centered_rnd * timing_drift_ticks as f32) as i64 + drag_bias_ticks;
                let new_tick = (ev.tick as i64 + offset).max(0) as u64;
                ev.tick = new_tick;

                // Velocity jitter for NoteOn events
                if let ScheduledAction::NoteOn { velocity, .. } = &mut ev.action {
                    let v_offset = (pseudo_hash * 2.0 - 1.0) * velocity_jitter;
                    *velocity = (*velocity + v_offset).clamp(0.1, 1.0);
                }
            }
        }

        self.events.sort_by_key(|e| e.tick);
        self.seek_tick(self.current_tick);
    }

    /// Calculate the derived start tick of the section at the given index.
    /// Each section starts immediately after the preceding section.
    pub fn section_start_tick(&self, section_idx: usize) -> u64 {
        self.sections.iter().take(section_idx).map(|s| s.length_ticks()).sum()
    }

    /// Total arrangement ticks derived from all sections and ad-hoc timeline blocks.
    pub fn computed_total_ticks(&self) -> u64 {
        let sec_total: u64 = self.sections.iter().map(|s| s.length_ticks()).sum();
        let tb_max = self.timeline_blocks.iter().map(|b| b.end_tick()).max().unwrap_or(0);
        sec_total.max(tb_max)
    }

    /// Refresh timeline derived boundaries (total_ticks and loop points)
    pub fn refresh_boundaries(&mut self) {
        let computed = self.computed_total_ticks();
        if computed > 0 {
            self.total_ticks = computed;
            self.loop_end_tick = computed;
        }
    }

    pub fn get_section_at_tick(&self, tick: u64) -> Option<&SongSection> {
        let mut accum = 0u64;
        for s in &self.sections {
            let len = s.length_ticks();
            if len > 0 && tick >= accum && tick < accum + len {
                return Some(s);
            }
            accum += len;
        }
        // Fallback for legacy sections with explicit start_tick
        self.sections.iter().find(|s| tick >= s.start_tick() && tick < s.end_tick())
    }

    pub fn add_section(&mut self, section: SongSection) {
        let sec_end = section.end_tick();
        if sec_end > self.total_ticks {
            self.total_ticks = sec_end;
            self.loop_end_tick = sec_end;
        }
        self.sections.retain(|s| s.id != section.id);
        self.sections.push(section);
        self.rebuild_active_events();
    }

    pub fn remove_section(&mut self, section_id: &str) -> bool {
        let orig_len = self.sections.len();
        self.sections.retain(|s| s.id != section_id);
        let removed = self.sections.len() < orig_len;
        if removed {
            self.rebuild_active_events();
        }
        removed
    }

    pub fn update_section(&mut self, section: SongSection) -> bool {
        if let Some(pos) = self.sections.iter().position(|s| s.id == section.id) {
            let sec_end = section.end_tick();
            if sec_end > self.total_ticks {
                self.total_ticks = sec_end;
                self.loop_end_tick = sec_end;
            }
            self.sections[pos] = section;
            self.rebuild_active_events();
            true
        } else {
            false
        }
    }

    /// Add or update a TrackBlock in a section
    pub fn set_track_block(&mut self, section_id: &str, mut block: TrackBlock) -> bool {
        if let Some(sec) = self.sections.iter_mut().find(|s| s.id == section_id) {
            block.section_id = Some(section_id.to_string());
            sec.track_blocks.insert(block.track_id.clone(), block);
            self.rebuild_active_events();
            true
        } else {
            false
        }
    }

    /// Remove a TrackBlock from a section
    pub fn remove_track_block(&mut self, section_id: &str, track_id: &str) -> bool {
        if let Some(sec) = self.sections.iter_mut().find(|s| s.id == section_id) {
            let removed = sec.track_blocks.remove(track_id).is_some();
            if removed {
                self.rebuild_active_events();
            }
            removed
        } else {
            false
        }
    }

    /// Add or update an ad-hoc timeline block
    pub fn set_timeline_block(&mut self, block: TrackBlock) -> bool {
        if let Some(pos) = self.timeline_blocks.iter().position(|b| b.id == block.id) {
            self.timeline_blocks[pos] = block;
        } else {
            self.timeline_blocks.push(block);
        }
        self.rebuild_active_events();
        true
    }

    /// Remove an ad-hoc timeline block by id
    pub fn remove_timeline_block(&mut self, block_id: &str) -> bool {
        let orig_len = self.timeline_blocks.len();
        self.timeline_blocks.retain(|b| b.id != block_id);
        let removed = self.timeline_blocks.len() < orig_len;
        if removed {
            self.rebuild_active_events();
        }
        removed
    }

    pub fn get_timeline_block(&self, block_id: &str) -> Option<&TrackBlock> {
        self.timeline_blocks.iter().find(|b| b.id == block_id)
    }

    pub fn get_timeline_blocks_for_track(&self, track_id: &str) -> Vec<&TrackBlock> {
        self.timeline_blocks.iter().filter(|b| b.track_id == track_id).collect()
    }

    /// Returns all blocks for the given track along with their absolute start tick on the timeline:
    /// (block, absolute_start_tick)
    pub fn get_resolved_blocks_for_track<'a>(&'a self, track_id: &str) -> Vec<(&'a TrackBlock, u64)> {
        let mut resolved = Vec::new();

        // 1. Blocks from sections:
        for (sec_idx, sec) in self.sections.iter().enumerate() {
            if let Some(block) = sec.track_blocks.get(track_id) {
                let sec_start_tick = self.section_start_tick(sec_idx);
                let block_start_tick = sec_start_tick + block.start_offset_ticks;
                resolved.push((block, block_start_tick));
            }
        }

        // 2. Timeline blocks (where start_offset_ticks is the absolute timeline tick):
        for block in self.timeline_blocks.iter().filter(|b| b.track_id == track_id) {
            resolved.push((block, block.start_offset_ticks));
        }

        resolved
    }

    /// Set tempo override on a section
    pub fn set_section_tempo(&mut self, section_id: &str, tempo: Option<TempoOverride>) -> bool {
        if let Some(sec) = self.sections.iter_mut().find(|s| s.id == section_id) {
            sec.tempo = tempo;
            self.rebuild_active_events();
            true
        } else {
            false
        }
    }

    /// Set tempo override on a specific track block
    pub fn set_block_tempo(&mut self, section_id: &str, track_id: &str, tempo: Option<TempoOverride>) -> bool {
        if let Some(sec) = self.sections.iter_mut().find(|s| s.id == section_id) {
            if let Some(block) = sec.track_blocks.get_mut(track_id) {
                block.tempo = tempo;
                self.rebuild_active_events();
                return true;
            }
        }
        false
    }

    /// Set master project time signature
    pub fn set_project_time_signature(&mut self, ts: TimeSignature) {
        self.time_signature = ts;
        self.refresh_boundaries();
    }

    /// Set time signature override on a section (None to inherit project meter)
    pub fn set_section_time_signature(&mut self, section_id: &str, ts: Option<TimeSignature>) -> bool {
        if let Some(sec) = self.sections.iter_mut().find(|s| s.id == section_id) {
            sec.time_signature = ts;
            self.rebuild_active_events();
            true
        } else {
            false
        }
    }

    /// Set time signature override on a track block for polymeter (None to inherit section meter)
    pub fn set_block_time_signature(&mut self, section_id: &str, track_id: &str, ts: Option<TimeSignature>) -> bool {
        if let Some(sec) = self.sections.iter_mut().find(|s| s.id == section_id) {
            if let Some(block) = sec.track_blocks.get_mut(track_id) {
                block.time_signature = ts;
                self.rebuild_active_events();
                return true;
            }
        }
        false
    }

    /// Get effective time signature for a given section
    pub fn get_section_effective_time_signature(&self, section: &SongSection) -> TimeSignature {
        section.effective_time_signature(self.time_signature)
    }

    /// Get effective time signature for a specific track block in a section
    pub fn get_block_effective_time_signature(&self, section: &SongSection, track_id: &str) -> TimeSignature {
        let sec_ts = self.get_section_effective_time_signature(section);
        if let Some(block) = section.track_blocks.get(track_id) {
            block.effective_time_signature(sec_ts)
        } else {
            sec_ts
        }
    }

    /// Get effective time signature at a given tick
    pub fn get_time_signature_at_tick(&self, tick: u64) -> TimeSignature {
        if let Some(sec) = self.get_section_at_tick(tick) {
            self.get_section_effective_time_signature(sec)
        } else {
            self.time_signature
        }
    }

    /// Get effective time signature for a track at a given tick (taking polymeter into account)
    pub fn get_track_effective_time_signature(&self, track_id: &str, tick: u64) -> TimeSignature {
        if let Some(sec) = self.get_section_at_tick(tick) {
            self.get_block_effective_time_signature(sec, track_id)
        } else {
            self.time_signature
        }
    }

    /// Rebuild the materialized active events cache and refresh boundaries
    pub fn rebuild_active_events(&mut self) {
        self.refresh_boundaries();
        self.active_events = self.materialize_all_events();
        self.event_cursor = self.active_events.partition_point(|e| e.tick < self.current_tick);
        for sec in &self.sections {
            for track_id in sec.track_blocks.keys() {
                self.track_cursors.entry(track_id.clone()).or_insert_with(|| TrackPlaybackCursor::new(self.current_tick as f64));
            }
        }
        for tb in &self.timeline_blocks {
            self.track_cursors.entry(tb.track_id.clone()).or_insert_with(|| TrackPlaybackCursor::new(self.current_tick as f64));
        }
    }

    /// Get effective tempo for a specific track at a given tick position
    pub fn get_track_effective_bpm(&self, track_id: &str, tick: u64) -> f64 {
        if let Some(sec) = self.get_section_at_tick(tick) {
            let sec_bpm = sec.effective_bpm(self.bpm);
            if let Some(block) = sec.track_blocks.get(track_id) {
                block.effective_bpm(sec_bpm)
            } else {
                sec_bpm
            }
        } else {
            self.bpm
        }
    }

    /// Materialize all scheduled events from block-based events and ad-hoc timeline blocks
    pub fn materialize_all_events(&self) -> Vec<ScheduledEvent> {
        let mut all_events = Vec::new();
        let mut accum_tick = 0u64;

        for sec in &self.sections {
            let sec_start = accum_tick;
            let sec_len = sec.length_ticks();
            let sec_bpm = sec.effective_bpm(self.bpm);

            for block in sec.track_blocks.values() {
                let block_bpm = block.effective_bpm(sec_bpm);
                let tempo_scale = if block_bpm > 0.0 { sec_bpm / block_bpm } else { 1.0 };
                let block_start = sec_start + block.start_offset_ticks;

                for ev in &block.events {
                    let scaled_tick = (ev.tick as f64 * tempo_scale).round() as u64;
                    let abs_tick = block_start + scaled_tick;

                    let mut materialized = ev.clone();
                    materialized.tick = abs_tick;
                    all_events.push(materialized);
                }
            }
            accum_tick += sec_len;
        }

        // Materialize ad-hoc continuous timeline blocks
        for block in &self.timeline_blocks {
            for ev in &block.events {
                let mut materialized = ev.clone();
                materialized.tick = block.start_offset_ticks + ev.tick;
                materialized.track_id = block.track_id.clone();
                all_events.push(materialized);
            }
        }

        all_events.sort_by_key(|e| e.tick);
        all_events
    }

    pub fn play(&mut self) {
        self.rebuild_active_events();
        self.is_playing = true;
    }

    pub fn pause(&mut self) {
        self.is_playing = false;
    }

    pub fn reset(&mut self) {
        self.current_sample = 0;
        self.current_tick = 0;
        self.current_sub_tick = 0.0;
        self.event_cursor = 0;
        for cursor in self.track_cursors.values_mut() {
            cursor.current_tick = 0.0;
        }
    }

    pub fn seek_tick(&mut self, tick: u64) {
        self.rebuild_active_events();
        self.current_tick = tick;
        self.current_sub_tick = tick as f64;
        self.current_sample = self.ticks_to_samples(tick);
        self.event_cursor = self.active_events.partition_point(|e| e.tick < tick);
        for cursor in self.track_cursors.values_mut() {
            cursor.current_tick = tick as f64;
        }
    }

    /// Advance the timeline by `num_samples` and return all scheduled events occurring in this window.
    pub fn advance_samples(&mut self, num_samples: usize) -> Vec<ScheduledEvent> {
        if !self.is_playing || self.active_events.is_empty() {
            return Vec::new();
        }

        let current_sec = self.get_section_at_tick(self.current_tick);
        let effective_bpm = current_sec.map(|s| s.effective_bpm(self.bpm)).unwrap_or(self.bpm);

        let delta_ticks = (num_samples as f64 * effective_bpm * PPQ as f64) / (60.0 * self.sample_rate);
        let next_sub_tick = self.current_sub_tick + delta_ticks;
        let end_tick = next_sub_tick as u64;

        let mut due_events = Vec::new();

        // Check if we hit the loop boundary
        if self.loop_enabled && self.loop_end_tick > self.loop_start_tick && end_tick >= self.loop_end_tick {
            // Collect events up to the loop end
            while self.event_cursor < self.active_events.len() {
                let ev = &self.active_events[self.event_cursor];
                if ev.tick <= self.loop_end_tick {
                    due_events.push(ev.clone());
                    self.event_cursor += 1;
                } else {
                    break;
                }
            }

            // Loop back to start
            let loop_span = (self.loop_end_tick - self.loop_start_tick) as f64;
            let overshoot = next_sub_tick - self.loop_end_tick as f64;
            self.current_sub_tick = self.loop_start_tick as f64 + (overshoot % loop_span);
            self.current_tick = self.current_sub_tick as u64;
            self.current_sample = self.ticks_to_samples(self.current_tick);
            self.event_cursor = self.active_events.partition_point(|e| e.tick < self.current_tick);
        } else {
            while self.event_cursor < self.active_events.len() {
                let ev = &self.active_events[self.event_cursor];
                if ev.tick <= end_tick {
                    due_events.push(ev.clone());
                    self.event_cursor += 1;
                } else {
                    break;
                }
            }
            self.current_sub_tick = next_sub_tick;
            self.current_tick = end_tick;
            self.current_sample += num_samples as u64;
        }

        // Advance per-track playback cursors
        let track_ids: Vec<String> = self.track_cursors.keys().cloned().collect();
        for track_id in track_ids {
            let track_bpm = self.get_track_effective_bpm(&track_id, self.current_tick);
            if let Some(cursor) = self.track_cursors.get_mut(&track_id) {
                cursor.advance(num_samples, track_bpm, self.sample_rate as f32);
            }
        }

        due_events
    }
}
