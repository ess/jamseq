use anyhow::{Context, Result};
use midly::num::{u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::collections::HashMap;
use std::path::Path;

use crate::engine::JamSeqHost;
use crate::timeline::{PPQ, ScheduledAction, TimeSignature, TrackArrangerMode, TrackBlock};
use crate::track::Track;
use jamsynth::patch::JamSynthPatch;
use jamsynth::JamSynthEngine;

#[derive(Debug, Clone)]
pub struct ParsedMidiNote {
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub pitch: u8,
    pub velocity: f32,
}

#[derive(Debug, Clone)]
pub struct MidiTrackData {
    pub name: String,
    pub notes: Vec<ParsedMidiNote>,
}

#[derive(Debug, Clone)]
pub struct ImportedMidiSong {
    pub bpm: Option<f64>,
    pub time_signature: Option<(u32, u32)>,
    pub tracks: Vec<MidiTrackData>,
}

/// Parse a Standard MIDI File into structured notes, tempo, and meter
pub fn parse_midi_file(path: &Path) -> Result<ImportedMidiSong> {
    let bytes = std::fs::read(path).with_context(|| format!("Failed to read MIDI file: {:?}", path))?;
    let smf = Smf::parse(&bytes).with_context(|| "Failed to parse Standard MIDI File (SMF)")?;

    // Determine tick scaling factor relative to Jamband 960 PPQ
    let scale: f64 = match smf.header.timing {
        Timing::Metrical(ticks_per_beat) => {
            let tpb = ticks_per_beat.as_int() as f64;
            if tpb > 0.0 {
                PPQ as f64 / tpb
            } else {
                1.0
            }
        }
        Timing::Timecode(fps, subframes) => {
            // Fallback for SMPTE timecode (rare for musical SMF)
            let fps_val = fps.as_f32() as f64;
            let sub = subframes as f64;
            let ticks_per_sec = fps_val * sub;
            (PPQ as f64 * 2.0) / ticks_per_sec.max(1.0)
        }
    };

    let mut detected_bpm: Option<f64> = None;
    let mut detected_ts: Option<(u32, u32)> = None;
    let mut tracks_data: Vec<MidiTrackData> = Vec::new();

    for track in &smf.tracks {
        let mut curr_raw_tick: u64 = 0;
        let mut track_name = String::new();
        let mut raw_notes: Vec<ParsedMidiNote> = Vec::new();
        let mut active_notes: HashMap<u8, (u64, f32)> = HashMap::new();

        for ev in track {
            curr_raw_tick += ev.delta.as_int() as u64;
            let curr_tick = (curr_raw_tick as f64 * scale).round() as u64;

            match ev.kind {
                TrackEventKind::Meta(meta) => match meta {
                    MetaMessage::TrackName(bytes) => {
                        let name = String::from_utf8_lossy(bytes).trim().to_string();
                        if !name.is_empty() {
                            track_name = name;
                        }
                    }
                    MetaMessage::Tempo(micros) => {
                        let micros_val = micros.as_int() as f64;
                        if micros_val > 0.0 && detected_bpm.is_none() {
                            detected_bpm = Some(60_000_000.0 / micros_val);
                        }
                    }
                    MetaMessage::TimeSignature(num, den_power, _, _) => {
                        if detected_ts.is_none() {
                            let den = 2u32.pow(den_power as u32);
                            detected_ts = Some((num as u32, den));
                        }
                    }
                    _ => {}
                },
                TrackEventKind::Midi { message, .. } => match message {
                    MidiMessage::NoteOn { key, vel } => {
                        let pitch = key.as_int();
                        let velocity = vel.as_int();
                        if velocity > 0 {
                            // If a previous note on this pitch was still held, close it
                            if let Some((start_tick, prev_vel)) = active_notes.remove(&pitch) {
                                let dur = curr_tick.saturating_sub(start_tick).max(30);
                                raw_notes.push(ParsedMidiNote {
                                    start_tick,
                                    duration_ticks: dur,
                                    pitch,
                                    velocity: prev_vel,
                                });
                            }
                            active_notes.insert(pitch, (curr_tick, velocity as f32 / 127.0));
                        } else {
                            // NoteOn with velocity 0 is NoteOff
                            if let Some((start_tick, prev_vel)) = active_notes.remove(&pitch) {
                                let dur = curr_tick.saturating_sub(start_tick).max(30);
                                raw_notes.push(ParsedMidiNote {
                                    start_tick,
                                    duration_ticks: dur,
                                    pitch,
                                    velocity: prev_vel,
                                });
                            }
                        }
                    }
                    MidiMessage::NoteOff { key, .. } => {
                        let pitch = key.as_int();
                        if let Some((start_tick, prev_vel)) = active_notes.remove(&pitch) {
                            let dur = curr_tick.saturating_sub(start_tick).max(30);
                            raw_notes.push(ParsedMidiNote {
                                start_tick,
                                duration_ticks: dur,
                                pitch,
                                velocity: prev_vel,
                            });
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        // Close any dangling notes at track end
        for (pitch, (start_tick, velocity)) in active_notes {
            let dur = 240; // Default 16th note
            raw_notes.push(ParsedMidiNote {
                start_tick,
                duration_ticks: dur,
                pitch,
                velocity,
            });
        }

        // Sort chronologically
        raw_notes.sort_by_key(|n| n.start_tick);

        // Enforce the Pitch Non-Overlap Invariant per track
        let mut clean_notes: Vec<ParsedMidiNote> = Vec::with_capacity(raw_notes.len());
        let mut pitch_last_end: HashMap<u8, u64> = HashMap::new();

        for note in raw_notes {
            if let Some(&last_end) = pitch_last_end.get(&note.pitch) {
                if note.start_tick < last_end {
                    // Truncate previous note to prevent overlap
                    if let Some(prev) = clean_notes.iter_mut().rfind(|n| n.pitch == note.pitch) {
                        prev.duration_ticks = note.start_tick.saturating_sub(prev.start_tick).max(10);
                    }
                }
            }
            pitch_last_end.insert(note.pitch, note.start_tick + note.duration_ticks);
            clean_notes.push(note);
        }

        if !clean_notes.is_empty() {
            tracks_data.push(MidiTrackData {
                name: track_name,
                notes: clean_notes,
            });
        }
    }

    Ok(ImportedMidiSong {
        bpm: detected_bpm,
        time_signature: detected_ts,
        tracks: tracks_data,
    })
}

impl JamSeqHost {
    /// Import a Standard MIDI File (.mid) into either a specific track block or as full continuous timeline tracks
    pub fn import_midi_file(
        &mut self,
        path: &Path,
        target_track_id: Option<&str>,
        target_section_id: Option<&str>,
    ) -> Result<String> {
        let imported = parse_midi_file(path)?;

        // Apply detected tempo & meter if importing full song or project is empty
        if target_track_id.is_none() {
            if let Some(bpm) = imported.bpm {
                self.timeline.bpm = bpm;
            }
            if let Some((num, den)) = imported.time_signature {
                self.timeline.set_project_time_signature(TimeSignature::new(num, den));
            }
        }

        // Route A: Targeted block import (e.g. drum loop or bass pattern into a section block or timeline block)
        if let Some(track_id) = target_track_id {
            let notes_to_import = imported
                .tracks
                .iter()
                .find(|t| !t.notes.is_empty())
                .map(|t| t.notes.clone())
                .unwrap_or_default();

            if notes_to_import.is_empty() {
                return Ok("No notes found in MIDI file to import".to_string());
            }

            let max_tick = notes_to_import.iter().map(|n| n.start_tick + n.duration_ticks).max().unwrap_or(3840);
            let bar_len = ((max_tick + 3839) / 3840) * 3840;

            if let Some(sec_id) = target_section_id {
                // Section matrix block
                let block_id = format!("{}_{}", sec_id, track_id);
                let mut block = TrackBlock::new_section(&block_id, track_id, sec_id, bar_len);
                let mut ev_id = 1u64;
                for note in notes_to_import {
                    block.add_note(ev_id, ev_id + 1, note.start_tick, note.duration_ticks, note.pitch, note.velocity);
                    ev_id += 2;
                }
                self.timeline.set_track_block(sec_id, block);
                self.rebake_block(track_id, &block_id);
                return Ok(format!("Imported MIDI to section '{}' track '{}'", sec_id, track_id));
            } else {
                // Check if block already exists on timeline
                let block_id = format!("{}_imported", track_id);
                let mut block = TrackBlock::new_timeline(&block_id, track_id, 0, bar_len);
                let mut ev_id = 1u64;
                for note in notes_to_import {
                    block.add_note(ev_id, ev_id + 1, note.start_tick, note.duration_ticks, note.pitch, note.velocity);
                    ev_id += 2;
                }
                self.set_timeline_block(block);
                return Ok(format!("Imported MIDI to timeline track '{}'", track_id));
            }
        }

        // Route B: Full Multi-Track Import to Continuous Timeline Tracks
        let mut imported_count = 0;
        let mut next_ev_id = (self.timeline.events.len() as u64 + 1) * 1000;

        for (idx, midi_trk) in imported.tracks.iter().enumerate() {
            if midi_trk.notes.is_empty() {
                continue;
            }

            let track_id = format!("midi_trk_{}", idx + 1);
            let track_name = if !midi_trk.name.is_empty() {
                midi_trk.name.clone()
            } else {
                format!("MIDI Track {}", idx + 1)
            };

            let mut synth = JamSynthEngine::new(self.sample_rate);
            synth.load_patch(JamSynthPatch::preset_808_sub());
            let mut trk = Track::new_synth(&track_id, &track_name, synth);
            trk.arranger_mode = TrackArrangerMode::Timeline;
            self.add_track(trk);

            let max_tick = midi_trk.notes.iter().map(|n| n.start_tick + n.duration_ticks).max().unwrap_or(3840);
            let bar_len = ((max_tick + 3839) / 3840) * 3840;

            let block_id = format!("{}_clip", track_id);
            let mut block = TrackBlock::new_timeline(&block_id, &track_id, 0, bar_len);

            for note in &midi_trk.notes {
                block.add_note(
                    next_ev_id,
                    next_ev_id + 1,
                    note.start_tick,
                    note.duration_ticks,
                    note.pitch,
                    note.velocity,
                );
                next_ev_id += 2;
            }

            self.timeline.timeline_blocks.push(block);
            self.rebake_block(&track_id, &block_id);
            imported_count += 1;
        }

        self.timeline.rebuild_active_events();
        Ok(format!("Imported {} tracks from MIDI file", imported_count))
    }

    /// Export project arrangement to a Standard MIDI File (SMF Type 1) at 960 PPQ
    pub fn export_midi_file(&self, path: &Path) -> Result<()> {
        let header = Header::new(Format::Parallel, Timing::Metrical(u15_from_u32(PPQ as u32)));
        let mut smf = Smf::new(header);

        // Track 0: Conductor (Tempo, Meter, Title)
        let mut tempo_track: Vec<TrackEvent> = Vec::new();

        // Project Title
        let title_bytes = self.timeline.title.as_bytes();
        tempo_track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::TrackName(title_bytes)),
        });

        // Time Signature
        let ts = self.timeline.time_signature;
        let den_power = (ts.denominator as f64).log2().round() as u8;
        tempo_track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::TimeSignature(
                ts.numerator as u8,
                den_power,
                24,
                8,
            )),
        });

        // Set Tempo
        let micros_per_beat = (60_000_000.0 / self.timeline.bpm.max(10.0)).round() as u32;
        tempo_track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::from(micros_per_beat))),
        });

        tempo_track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });

        smf.tracks.push(tempo_track);

        // Tracks 1..N: Musical tracks
        for track in &self.tracks {
            let mut track_events: Vec<(u64, MidiMessage)> = Vec::new();

            // Collect notes from sections:
            for (sec_idx, sec) in self.timeline.sections.iter().enumerate() {
                if let Some(block) = sec.track_blocks.get(&track.id) {
                    let sec_start = self.timeline.section_start_tick(sec_idx);
                    let block_start = sec_start + block.start_offset_ticks;

                    for (i, ev) in block.events.iter().enumerate() {
                        if let ScheduledAction::NoteOn { note, velocity } = ev.action {
                            let abs_start = block_start + ev.tick;
                            let end_tick = block
                                .events
                                .iter()
                                .skip(i + 1)
                                .find(|off_ev| {
                                    off_ev.tick >= ev.tick
                                        && match off_ev.action {
                                            ScheduledAction::NoteOff { note: off_n } => off_n == note,
                                            _ => false,
                                        }
                                })
                                .map(|off_ev| off_ev.tick)
                                .unwrap_or(ev.tick + 240);
                            let abs_end = block_start + end_tick;

                            let vel_byte = (velocity * 127.0).clamp(1.0, 127.0) as u8;
                            track_events.push((abs_start, MidiMessage::NoteOn {
                                key: u7::from(note),
                                vel: u7::from(vel_byte),
                            }));
                            track_events.push((abs_end, MidiMessage::NoteOff {
                                key: u7::from(note),
                                vel: u7::from(64u8),
                            }));
                        }
                    }
                }
            }

            // Collect notes from continuous timeline blocks:
            for block in self.timeline.timeline_blocks.iter().filter(|b| b.track_id == track.id) {
                let block_start = block.start_offset_ticks;
                for (i, ev) in block.events.iter().enumerate() {
                    if let ScheduledAction::NoteOn { note, velocity } = ev.action {
                        let abs_start = block_start + ev.tick;
                        let end_tick = block
                            .events
                            .iter()
                            .skip(i + 1)
                            .find(|off_ev| {
                                off_ev.tick >= ev.tick
                                    && match off_ev.action {
                                        ScheduledAction::NoteOff { note: off_n } => off_n == note,
                                        _ => false,
                                    }
                            })
                            .map(|off_ev| off_ev.tick)
                            .unwrap_or(ev.tick + 240);
                        let abs_end = block_start + end_tick;

                        let vel_byte = (velocity * 127.0).clamp(1.0, 127.0) as u8;
                        track_events.push((abs_start, MidiMessage::NoteOn {
                            key: u7::from(note),
                            vel: u7::from(vel_byte),
                        }));
                        track_events.push((abs_end, MidiMessage::NoteOff {
                            key: u7::from(note),
                            vel: u7::from(64u8),
                        }));
                    }
                }
            }

            if track_events.is_empty() {
                continue;
            }

            // Sort: NoteOff before NoteOn at identical ticks
            track_events.sort_by(|(tick_a, msg_a), (tick_b, msg_b)| {
                tick_a.cmp(tick_b).then_with(|| {
                    let prio_a = matches!(msg_a, MidiMessage::NoteOff { .. });
                    let prio_b = matches!(msg_b, MidiMessage::NoteOff { .. });
                    prio_b.cmp(&prio_a)
                })
            });

            let mut out_track: Vec<TrackEvent> = Vec::new();
            let name_bytes = track.name.as_bytes();
            out_track.push(TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::TrackName(name_bytes)),
            });

            let mut prev_tick: u64 = 0;
            for (tick, msg) in track_events {
                let delta = tick.saturating_sub(prev_tick) as u32;
                prev_tick = tick;
                out_track.push(TrackEvent {
                    delta: u28::from(delta.min(0x0FFF_FFFF)),
                    kind: TrackEventKind::Midi {
                        channel: u4::from(0u8),
                        message: msg,
                    },
                });
            }

            out_track.push(TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            });

            smf.tracks.push(out_track);
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        smf.save(path).with_context(|| format!("Failed to write MIDI file to {:?}", path))?;
        Ok(())
    }
}

fn u15_from_u32(val: u32) -> midly::num::u15 {
    midly::num::u15::from((val & 0x7FFF) as u16)
}
