use serde::{Deserialize, Serialize};

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
    pub fn ticks_per_bar(&self) -> u64 {
        // numerator beats per bar, each beat is PPQ * 4 / denominator
        (self.numerator as u64 * PPQ * 4) / self.denominator as u64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongSection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub start_tick: u64,
    #[serde(default)]
    pub length_ticks: u64,
    #[serde(default)]
    pub start_bar: u32,
    #[serde(default)]
    pub bars: u32,
    pub energy: u8,
    #[serde(default)]
    pub time_signature: TimeSignature,
}

impl SongSection {
    pub fn new(id: &str, name: &str, start_bar: u32, bars: u32, energy: u8) -> Self {
        let ts = TimeSignature::default();
        let tpb = ts.ticks_per_bar();
        let start_tick = if start_bar > 0 { (start_bar as u64 - 1) * tpb } else { 0 };
        let length_ticks = bars as u64 * tpb;
        Self {
            id: id.to_string(),
            name: name.to_string(),
            start_tick,
            length_ticks,
            start_bar,
            bars,
            energy,
            time_signature: ts,
        }
    }

    pub fn from_ticks(id: &str, name: &str, start_tick: u64, length_ticks: u64, energy: u8) -> Self {
        let ts = TimeSignature::default();
        let tpb = ts.ticks_per_bar().max(1);
        let start_bar = (start_tick / tpb) as u32 + 1;
        let bars = (length_ticks as f64 / tpb as f64).ceil() as u32;
        Self {
            id: id.to_string(),
            name: name.to_string(),
            start_tick,
            length_ticks,
            start_bar,
            bars,
            energy,
            time_signature: ts,
        }
    }

    pub fn start_tick(&self) -> u64 {
        if self.start_tick > 0 || self.length_ticks > 0 {
            self.start_tick
        } else if self.start_bar > 0 {
            (self.start_bar as u64 - 1) * self.time_signature.ticks_per_bar()
        } else {
            0
        }
    }

    pub fn length_ticks(&self) -> u64 {
        if self.length_ticks > 0 {
            self.length_ticks
        } else if self.bars > 0 {
            self.bars as u64 * self.time_signature.ticks_per_bar()
        } else {
            self.time_signature.ticks_per_bar()
        }
    }

    pub fn end_tick(&self) -> u64 {
        self.start_tick() + self.length_ticks()
    }

    /// Calculate (1-indexed bar, 1-indexed beat within bar)
    pub fn start_bar_beat(&self) -> (u32, f32) {
        let tpb = self.time_signature.ticks_per_bar().max(1);
        let ticks = self.start_tick();
        let bar = (ticks / tpb) as u32 + 1;
        let rem_ticks = ticks % tpb;
        let ticks_per_beat = (PPQ * 4) / self.time_signature.denominator as u64;
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
        let tpb = self.time_signature.ticks_per_bar().max(1);
        self.start_tick = ((self.start_tick() + tpb / 2) / tpb) * tpb;
        let len = self.length_ticks();
        self.length_ticks = (((len + tpb / 2) / tpb).max(1)) * tpb;
        self.recompute_bars();
    }

    fn recompute_bars(&mut self) {
        let tpb = self.time_signature.ticks_per_bar().max(1);
        self.start_bar = (self.start_tick / tpb) as u32 + 1;
        self.bars = (self.length_ticks as f64 / tpb as f64).ceil() as u32;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineArrangement {
    pub title: String,
    pub bpm: f64,
    pub total_bars: u32,
    pub loop_enabled: bool,
    pub sections: Vec<SongSection>,
    pub events: Vec<ScheduledEvent>,
}

impl Default for TimelineArrangement {
    fn default() -> Self {
        Self {
            title: "Untitled".to_string(),
            bpm: 68.0,
            total_bars: 4,
            loop_enabled: true,
            sections: vec![SongSection::new("intro", "Intro", 1, 4, 2)],
            events: Vec::new(),
        }
    }
}

pub struct Timeline {
    pub title: String,
    pub bpm: f64,
    pub sample_rate: f64,
    pub current_sample: u64,
    pub current_tick: u64,
    pub total_ticks: u64,
    pub loop_enabled: bool,
    pub loop_start_tick: u64,
    pub loop_end_tick: u64,
    pub is_playing: bool,
    pub sections: Vec<SongSection>,
    pub events: Vec<ScheduledEvent>,

    event_cursor: usize,
}

impl Timeline {
    pub fn new(bpm: f64, sample_rate: f64) -> Self {
        Self {
            title: "Untitled".to_string(),
            bpm: if bpm > 10.0 { bpm } else { 120.0 },
            sample_rate: if sample_rate > 0.0 { sample_rate } else { 44100.0 },
            current_sample: 0,
            current_tick: 0,
            total_ticks: 4 * 4 * PPQ, // 4 bars default
            loop_enabled: true,
            loop_start_tick: 0,
            loop_end_tick: 4 * 4 * PPQ,
            is_playing: false,
            sections: Vec::new(),
            events: Vec::new(),
            event_cursor: 0,
        }
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
        self.total_ticks = arrangement.total_bars as u64 * 4 * PPQ;
        self.loop_enabled = arrangement.loop_enabled;
        self.loop_start_tick = 0;
        self.loop_end_tick = self.total_ticks;
        self.sections = arrangement.sections;

        let mut events = arrangement.events;
        // Sort chronologically
        events.sort_by_key(|e| e.tick);
        self.events = events;
        self.reset();
    }

    /// Export the current live state into a TimelineArrangement for saving
    pub fn to_arrangement(&self) -> TimelineArrangement {
        let total_bars = (self.total_ticks / (4 * PPQ)).max(1) as u32;
        TimelineArrangement {
            title: self.title.clone(),
            bpm: self.bpm,
            total_bars,
            loop_enabled: self.loop_enabled,
            sections: self.sections.clone(),
            events: self.events.clone(),
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

    pub fn get_section_at_tick(&self, tick: u64) -> Option<&SongSection> {
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
        self.sections.sort_by_key(|s| s.start_tick());
    }

    pub fn remove_section(&mut self, section_id: &str) -> bool {
        let orig_len = self.sections.len();
        self.sections.retain(|s| s.id != section_id);
        self.sections.len() < orig_len
    }

    pub fn update_section(&mut self, section: SongSection) -> bool {
        if let Some(pos) = self.sections.iter().position(|s| s.id == section.id) {
            let sec_end = section.end_tick();
            if sec_end > self.total_ticks {
                self.total_ticks = sec_end;
                self.loop_end_tick = sec_end;
            }
            self.sections[pos] = section;
            self.sections.sort_by_key(|s| s.start_tick());
            true
        } else {
            false
        }
    }

    pub fn play(&mut self) {
        self.is_playing = true;
    }

    pub fn pause(&mut self) {
        self.is_playing = false;
    }

    pub fn reset(&mut self) {
        self.current_sample = 0;
        self.current_tick = 0;
        self.event_cursor = 0;
    }

    pub fn seek_tick(&mut self, tick: u64) {
        self.current_tick = tick;
        self.current_sample = self.ticks_to_samples(tick);
        // Find new cursor position
        self.event_cursor = self.events.partition_point(|e| e.tick < tick);
    }

    /// Advance the timeline by `num_samples` and return all scheduled events occurring in this window.
    pub fn advance_samples(&mut self, num_samples: usize) -> Vec<ScheduledEvent> {
        if !self.is_playing || self.events.is_empty() {
            return Vec::new();
        }

        self.current_sample += num_samples as u64;
        let end_tick = self.samples_to_ticks(self.current_sample);

        let mut due_events = Vec::new();

        // Check if we hit the loop boundary
        if self.loop_enabled && end_tick >= self.loop_end_tick {
            // Collect events up to the loop end
            while self.event_cursor < self.events.len() {
                let ev = &self.events[self.event_cursor];
                if ev.tick <= self.loop_end_tick {
                    due_events.push(ev.clone());
                    self.event_cursor += 1;
                } else {
                    break;
                }
            }

            // Loop back to start
            self.current_sample = self.ticks_to_samples(self.loop_start_tick);
            self.current_tick = self.loop_start_tick;
            self.event_cursor = self.events.partition_point(|e| e.tick < self.loop_start_tick);
        } else {
            while self.event_cursor < self.events.len() {
                let ev = &self.events[self.event_cursor];
                if ev.tick <= end_tick {
                    due_events.push(ev.clone());
                    self.event_cursor += 1;
                } else {
                    break;
                }
            }
            self.current_tick = end_tick;
        }

        due_events
    }
}
