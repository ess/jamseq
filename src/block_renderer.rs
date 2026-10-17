use anyhow::Result;

/// Offline render a single block directly through its track's instrument and DSP chain.
/// Employs dynamic silence detection (-80 dBFS threshold, 200ms window, 10s ceiling).
pub fn render_block(
    track: &mut crate::track::Track,
    block: &crate::timeline::TrackBlock,
    eff_bpm: f64,
    sample_rate: f32,
) -> Result<crate::sampler::SampleBuffer> {
    // Save target track fader/pan
    let orig_track_vol = track.volume;
    let orig_track_pan = track.pan;
    track.volume = 1.0;
    track.pan = 0.0;

    let block_dur_ticks = block.effective_length_ticks().max(crate::timeline::TICKS_PER_16TH);
    let ticks_per_beat = crate::timeline::PPQ as f64;
    let beats = block_dur_ticks as f64 / ticks_per_beat;
    let seconds = beats * (60.0 / eff_bpm);
    let total_samples = (seconds * sample_rate as f64).round() as usize;

    let mut samples_left = Vec::with_capacity(total_samples);
    let mut samples_right = Vec::with_capacity(total_samples);

    let block_size = 512;
    let mut left = vec![0.0f32; block_size];
    let mut right = vec![0.0f32; block_size];
    let mut scratch_left = vec![0.0f32; block_size];
    let mut scratch_right = vec![0.0f32; block_size];

    // Sort block events by tick offset relative to block start (0..block_dur_ticks)
    let mut sorted_events = block.events.clone();
    sorted_events.sort_by_key(|e| e.tick);
    let mut next_ev_idx = 0;

    let samples_per_tick = (60.0 / eff_bpm) * (sample_rate as f64) / ticks_per_beat;

    // Ensure track is quiet before starting block render
    track.all_notes_off();

    // Phase 1: Render block duration, dispatching events at the appropriate tick offsets
    let mut current_sample = 0;
    while current_sample < total_samples {
        let chunk_size = (total_samples - current_sample).min(block_size);
        let chunk_end_sample = current_sample + chunk_size;

        let chunk_end_tick = if samples_per_tick > 0.0 {
            (chunk_end_sample as f64 / samples_per_tick).round() as u64
        } else {
            0
        };

        // Dispatch any events occurring up to chunk_end_tick
        while next_ev_idx < sorted_events.len() && sorted_events[next_ev_idx].tick < chunk_end_tick {
            let ev = &sorted_events[next_ev_idx];
            match &ev.action {
                crate::timeline::ScheduledAction::NoteOn { note, velocity } => {
                    track.note_on(*note, *velocity);
                }
                crate::timeline::ScheduledAction::NoteOff { note } => {
                    track.note_off(*note);
                }
                crate::timeline::ScheduledAction::ControlChange { cc, value } => {
                    track.handle_cc(*cc, *value);
                }
                crate::timeline::ScheduledAction::SetParam { param_name, value } => {
                    track.set_parameter(param_name, *value);
                }
                crate::timeline::ScheduledAction::LoadPatchJson { json } => {
                    match &mut track.instrument {
                        crate::track::TrackInstrument::Clap(clap) => {
                            if let Ok(patch) = jamsynth::JamSynthPatch::from_json(json) {
                                clap.set_parameter("cutoff", patch.filter.cutoff_hz);
                                clap.set_parameter("resonance", patch.filter.resonance);
                                clap.set_parameter("glide", patch.glide_ms);
                                clap.set_parameter("drive", patch.drive.amount);
                                clap.set_parameter("attack", patch.amp_env.attack_ms);
                                clap.set_parameter("decay", patch.amp_env.decay_ms);
                                clap.set_parameter("sustain", patch.amp_env.sustain);
                                clap.set_parameter("release", patch.amp_env.release_ms);
                            }
                        }
                        crate::track::TrackInstrument::Synth(synth) => {
                            let _ = synth.load_patch_json(json);
                        }
                        _ => {}
                    }
                }
            }
            next_ev_idx += 1;
        }

        left[..chunk_size].fill(0.0);
        right[..chunk_size].fill(0.0);

        track.process_block(
            current_sample as u64,
            &mut left[..chunk_size],
            &mut right[..chunk_size],
            &mut scratch_left[..chunk_size],
            &mut scratch_right[..chunk_size],
            None,
            true, // bypass stems so instrument + FX process live
            true,
        );

        samples_left.extend_from_slice(&left[..chunk_size]);
        samples_right.extend_from_slice(&right[..chunk_size]);
        current_sample += chunk_size;
    }

    // Phase 2: Dynamic Tail Rendering with Silence Detection (-80 dBFS cutoff with 10s ceiling)
    let silence_threshold = 0.0001f32; // -80 dBFS
    let silence_window_samples = (0.2 * sample_rate) as usize; // 200 ms
    let four_bars_samples = (4.0 * 4.0 * (60.0 / eff_bpm) * sample_rate as f64).round() as usize;
    let max_tail_ceiling = (10.0 * sample_rate as f64) as usize;
    let max_tail_samples = max_tail_ceiling.max(four_bars_samples);

    let mut silent_samples = 0;
    let mut tail_samples_rendered = 0;

    track.all_notes_off();

    while tail_samples_rendered < max_tail_samples {
        let chunk_size = block_size.min(max_tail_samples - tail_samples_rendered);
        left[..chunk_size].fill(0.0);
        right[..chunk_size].fill(0.0);

        let sim_sample = (total_samples + tail_samples_rendered) as u64;
        track.process_block(
            sim_sample,
            &mut left[..chunk_size],
            &mut right[..chunk_size],
            &mut scratch_left[..chunk_size],
            &mut scratch_right[..chunk_size],
            None,
            true, // bypass stems so we capture natural decay and reverb tails
            true,
        );

        samples_left.extend_from_slice(&left[..chunk_size]);
        samples_right.extend_from_slice(&right[..chunk_size]);
        tail_samples_rendered += chunk_size;

        let mut chunk_peak = 0.0f32;
        for i in 0..chunk_size {
            let mag = left[i].abs().max(right[i].abs());
            if mag > chunk_peak {
                chunk_peak = mag;
            }
        }

        if chunk_peak < silence_threshold {
            silent_samples += chunk_size;
            if silent_samples >= silence_window_samples {
                break;
            }
        } else {
            silent_samples = 0;
        }
    }

    track.all_notes_off();

    // Restore target track volume & pan
    track.volume = orig_track_vol;
    track.pan = orig_track_pan;

    Ok(crate::sampler::SampleBuffer {
        channels: 2,
        sample_rate: sample_rate as u32,
        samples_left,
        samples_right,
    })
}
