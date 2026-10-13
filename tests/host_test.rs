use hound::{SampleFormat, WavSpec, WavWriter};
use jamsynth::patch::JamSynthPatch;
use jamsynth::JamSynthEngine;
use jamseq::engine::JamSeqHost;
use jamseq::sampler::{SamplerConfig, SamplerInstrument};
use jamseq::timeline::{ScheduledAction, ScheduledEvent, TimelineArrangement, TICKS_PER_16TH};
use jamseq::track::Track;
use std::fs;
use std::path::Path;

fn create_test_wav(path: &Path) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    // Write 0.1 seconds of a 220Hz test sine wave
    for i in 0..4410 {
        let t = i as f32 / 44100.0;
        let s = (t * 220.0 * 2.0 * std::f32::consts::PI).sin();
        let val = (s * 30000.0) as i16;
        writer.write_sample(val).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn test_sampler_chromatic_pitching() {
    let temp_dir = std::env::temp_dir();
    let wav_path = temp_dir.join("jamseq_test_sample.wav");
    create_test_wav(&wav_path);

    let mut config = SamplerConfig::default();
    config.sample_path = wav_path.clone();
    config.root_note = 60; // C4

    let mut sampler = SamplerInstrument::new(config, 44100.0);
    assert!(sampler.buffer.is_some(), "Sampler failed to load WAV buffer");

    // Play C4 (root pitch)
    sampler.note_on(60, 1.0);
    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    sampler.process_block(&mut left, &mut right);

    let max_c4: f32 = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(max_c4 > 0.05, "Expected audio at root pitch C4");

    // Play C5 (pitched up 1 octave)
    sampler.all_notes_off();
    sampler.note_on(72, 1.0);
    left.fill(0.0);
    right.fill(0.0);
    sampler.process_block(&mut left, &mut right);
    let max_c5: f32 = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(max_c5 > 0.05, "Expected audio at pitched C5");

    let _ = fs::remove_file(wav_path);
}

#[test]
fn test_offline_render_to_wav() {
    let temp_dir = std::env::temp_dir();
    let out_wav = temp_dir.join("jamseq_test_render.wav");

    let mut host = JamSeqHost::new(68.0, 44100.0);

    // Track 1: Sub 808
    let mut bass = JamSynthEngine::new(44100.0);
    bass.load_patch(JamSynthPatch::preset_808_sub());
    host.add_track(Track::new_synth("bass", "Sub 808", bass));

    // Track 2: Cold Pad
    let mut pad = JamSynthEngine::new(44100.0);
    pad.load_patch(JamSynthPatch::preset_cold_pad());
    host.add_track(Track::new_synth("pad", "Pad", pad));

    let events = vec![
        ScheduledEvent::new(1, 0, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 }),
        ScheduledEvent::new(2, 8 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 50 }),
        ScheduledEvent::new(3, 4 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOn { note: 62, velocity: 0.8 }),
        ScheduledEvent::new(4, 12 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOff { note: 62 }),
    ];

    let arrangement = TimelineArrangement {
        title: "Test Render".into(),
        bpm: 68.0,
        time_signature: jamseq::timeline::TimeSignature::default(),
        total_bars: 1,
        loop_enabled: false,
        tracks: Vec::new(),
        master_bus: None,
        aux_buses: Vec::new(),
        sections: vec![jamseq::SongSection::new("test_verse", "Test Verse", 1, 1)],
        events,
    };

    host.load_arrangement(arrangement);

    // Render 1 bar
    host.render_to_wav(&out_wav, 1).expect("Failed to render WAV");

    assert!(out_wav.exists(), "Rendered WAV does not exist!");
    let reader = hound::WavReader::open(&out_wav).expect("Rendered WAV is invalid!");
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 44100);
    assert!(reader.len() > 0, "Rendered WAV is empty!");

    let _ = fs::remove_file(out_wav);
}

#[test]
fn test_audio_recorder_and_clip_playback() {
    let temp_dir = std::env::temp_dir();
    let rec_wav = temp_dir.join("jamseq_test_recorded_take.wav");
    let _ = fs::remove_file(&rec_wav);

    let recorder = jamseq::recorder::AudioRecorder::new();
    recorder.start_recording(0, &rec_wav);
    assert!(recorder.is_recording());

    // Push 4410 frames of 440Hz test audio (stereo)
    let mut fake_input = Vec::new();
    for i in 0..4410 {
        let t = i as f32 / 44100.0;
        let s = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.7;
        fake_input.push(s);
        fake_input.push(s);
    }
    recorder.push_input_frames(&fake_input, 2);

    let clip_info = recorder.stop_recording(44100).expect("Failed to stop recording and write WAV");
    assert!(!recorder.is_recording());
    assert_eq!(clip_info.total_samples, 44100 / 10);
    assert!(rec_wav.exists());

    // Now test playing that recorded take via AudioClipTrack
    let mut clip_track = jamseq::audio_clip::AudioClipTrack::new();
    let clip = jamseq::audio_clip::AudioClip::new(&rec_wav, 0);
    assert!(clip.buffer.is_some(), "Recorded WAV buffer could not be loaded");
    clip_track.add_clip(clip);

    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    clip_track.process_block(0, &mut left, &mut right);

    let max_amp: f32 = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(max_amp > 0.1, "AudioClipTrack did not output recorded audio");

    let _ = fs::remove_file(rec_wav);
}

#[test]
fn test_ipc_socket_server() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    let temp_dir = std::env::temp_dir();
    let sock_path = temp_dir.join("test_jamseq.sock");

    let host = Arc::new(Mutex::new(JamSeqHost::new(120.0, 44100.0)));
    {
        let mut h = host.lock().unwrap();
        h.add_track(Track::new_audio_clips("vox", "Vocals", jamseq::audio_clip::AudioClipTrack::new()));
    }

    let server = jamseq::ipc::IpcServer::new(&sock_path);
    server.start(Arc::clone(&host)).expect("Failed to start IPC server");

    // Give server a split second to bind
    thread::sleep(Duration::from_millis(50));

    // Connect client stream
    let mut stream = UnixStream::connect(&sock_path).expect("Failed to connect to IPC socket");
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    // 1. Send GetStatus
    writeln!(stream, "{{\"cmd\":\"GetStatus\"}}").unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).expect("Invalid JSON response");
    assert!(resp.ok);
    let status = resp.status.unwrap();
    assert!(!status.is_playing);
    assert_eq!(status.tracks.len(), 1);
    assert_eq!(status.tracks[0].id, "vox");

    // 2. Send Play
    writeln!(stream, "{{\"cmd\":\"Play\"}}").unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.status.unwrap().is_playing);

    // 3. Send SetVolume
    writeln!(stream, "{{\"cmd\":\"SetVolume\",\"args\":{{\"track_id\":\"vox\",\"volume\":0.75}}}}").unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert_eq!(resp.status.unwrap().tracks[0].volume, 0.75);

    // 4. Send AddSection and ReplaceSectionEvents (Section-scoped edit)
    writeln!(
        stream,
        "{{\"cmd\":\"AddSection\",\"args\":{{\"id\":\"intro\",\"name\":\"Intro\",\"start_tick\":0,\"length_ticks\":7680}}}}"
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);
    assert_eq!(resp.status.unwrap().sections.len(), 1);

    writeln!(
        stream,
        "{{\"cmd\":\"ReplaceSectionEvents\",\"args\":{{\"section_id\":\"intro\",\"track_id\":\"vox\",\"events\":[{{\"id\":10,\"tick\":0,\"track_id\":\"vox\",\"action\":{{\"NoteOn\":{{\"note\":60,\"velocity\":0.8}}}}}}]}}}}"
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);
    assert_eq!(resp.status.unwrap().sections.len(), 1);

    // 5. Send granular AddNote
    writeln!(
        stream,
        "{{\"cmd\":\"AddNote\",\"args\":{{\"note_on_id\":101,\"note_off_id\":102,\"track_id\":\"vox\",\"tick\":960,\"duration_ticks\":480,\"note\":64,\"velocity\":0.9}}}}"
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);

    // 6. Send granular DeleteNoteAt
    writeln!(
        stream,
        "{{\"cmd\":\"DeleteNoteAt\",\"args\":{{\"track_id\":\"vox\",\"tick\":960,\"note\":64}}}}"
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);

    // 6b. Test AddTrackInsert & SetTrackInsertParam over IPC
    let jamsynth_path = if std::path::Path::new("target/debug/libjamsynth.so").exists() {
        "target/debug/libjamsynth.so"
    } else {
        "../target/debug/libjamsynth.so"
    };
    writeln!(
        stream,
        "{{\"cmd\":\"AddTrackInsert\",\"args\":{{\"track_id\":\"vox\",\"plugin_uri_or_path\":\"{}\",\"plugin_name\":\"JamSynth Insert\"}}}}",
        jamsynth_path
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);
    assert_eq!(resp.status.as_ref().unwrap().tracks[0].inserts.len(), 1);

    writeln!(
        stream,
        "{{\"cmd\":\"SetTrackInsertParam\",\"args\":{{\"track_id\":\"vox\",\"insert_index\":0,\"param_name\":\"cutoff\",\"value\":0.72}}}}"
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);
    let insert_params = &resp.status.as_ref().unwrap().tracks[0].inserts[0].params;
    let cutoff_param = insert_params.iter().find(|p| p.name.to_lowercase() == "cutoff").unwrap();
    assert!((cutoff_param.value - 0.72).abs() < 1e-3);

    // 7. Test SaveProject & LoadProject (.jam.json convention)
    let test_proj_path = temp_dir.join("test_saved_project.jam.json");
    let test_proj_str = test_proj_path.to_str().unwrap().replace("\\", "/");
    writeln!(
        stream,
        "{{\"cmd\":\"SaveProject\",\"args\":{{\"path\":\"{}\"}}}}",
        test_proj_str
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);
    assert!(test_proj_path.exists());

    // Switch/Reload Project
    writeln!(
        stream,
        "{{\"cmd\":\"LoadProject\",\"args\":{{\"path\":\"{}\"}}}}",
        test_proj_str
    ).unwrap();
    resp_line.clear();
    reader.read_line(&mut resp_line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&resp_line).unwrap();
    assert!(resp.ok);
    let _ = fs::remove_file(test_proj_path);
    server.stop();
}

#[test]
fn test_clap_and_lv2_plugin_hosting() {
    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.add_track(Track::new_synth("track1", "Test Track", JamSynthEngine::new(48000.0)));

    // 1. Switch to Surge XT CLAP
    let surge_path = "/usr/lib/clap/Surge XT.clap";
    if std::path::Path::new(surge_path).exists() {
        host.switch_track_instrument("track1", surge_path, "Surge XT");
        assert_eq!(host.tracks[0].current_plugin.as_deref(), Some("Surge XT"));

        // Verify native CLAP parameter introspection
        let params = host.tracks[0].get_parameters();
        println!("Introspected {} parameters for Surge XT CLAP", params.len());
        assert!(!params.is_empty(), "Surge XT should expose parameters natively via clack PluginParams");

        // Test updating a parameter
        let (ref name, _val, min, max) = params[0];
        let test_val = (min + max) / 2.0;
        host.tracks[0].set_parameter(name, test_val);

        // Trigger note on and process audio block
        host.tracks[0].note_on(60, 1.0);
        let mut left = vec![0.0f32; 512];
        let mut right = vec![0.0f32; 512];
        host.process_block(&mut left, &mut right);

        let max_l: f32 = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
        let max_r: f32 = right.iter().map(|s| s.abs()).fold(0.0, f32::max);
        println!("CLAP Surge XT audio output: max_l={}, max_r={}", max_l, max_r);
        assert!(max_l > 0.0001 || max_r > 0.0001, "Expected audio output from Surge XT CLAP");
    }

    // 2. Switch to AVL Drums LV2
    let avl_uri = "http://gareus.org/oss/lv2/avldrums#BlackPearl";
    host.switch_track_instrument("track1", avl_uri, "Black Pearl Drumkit");
    assert_eq!(host.tracks[0].current_plugin.as_deref(), Some("Black Pearl Drumkit"));

    // Verify native LV2 parameter introspection on AVL Drums (pure sampler with 0 control ports)
    let avl_params = host.tracks[0].get_parameters();
    println!("Introspected {} parameters for AVL Drums LV2", avl_params.len());

    // Switch to Surge XT LV2 to verify LV2 ControlPort introspection
    let surge_lv2_uri = "https://surge-synthesizer.github.io/lv2/surge-xt";
    host.switch_track_instrument("track1", surge_lv2_uri, "Surge XT LV2");
    let surge_lv2_params = host.tracks[0].get_parameters();
    println!("Introspected {} parameters for Surge XT LV2", surge_lv2_params.len());
    assert!(!surge_lv2_params.is_empty(), "Surge XT LV2 should expose parameters natively via Lilv ControlPorts");

    let (ref name, _val, min, max) = surge_lv2_params[0];
    let test_val = (min + max) / 2.0;
    host.tracks[0].set_parameter(name, test_val);

    // Switch back to Black Pearl for drum audio verification
    host.switch_track_instrument("track1", avl_uri, "Black Pearl Drumkit");

    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];

    // Process a warmup block to allow worker schedule to finish loading samples
    host.process_block(&mut left, &mut right);

    // Trigger note on (Kick note 36) and process audio blocks
    host.tracks[0].note_on(36, 1.0);
    let mut max_l = 0.0f32;
    let mut max_r = 0.0f32;
    for _ in 0..16 {
        host.process_block(&mut left, &mut right);
        max_l = max_l.max(left.iter().map(|s| s.abs()).fold(0.0, f32::max));
        max_r = max_r.max(right.iter().map(|s| s.abs()).fold(0.0, f32::max));
    }
    println!("LV2 AVL Drums audio output: max_l={}, max_r={}", max_l, max_r);
    assert!(max_l > 0.0001 || max_r > 0.0001, "Expected audio output from AVL Drums LV2");
}

#[test]
fn test_jamsynth_clap_hosting() {
    let jamsynth_path = if std::path::Path::new("target/debug/libjamsynth.so").exists() {
        "target/debug/libjamsynth.so"
    } else if std::path::Path::new("../target/debug/libjamsynth.so").exists() {
        "../target/debug/libjamsynth.so"
    } else {
        panic!("libjamsynth.so not found");
    };

    let mut clap_track = jamseq::clap_host::ClapTrack::load(jamsynth_path, 48000.0)
        .expect("Failed to load libjamsynth.so via ClapTrack");

    let params = clap_track.get_parameters();
    println!("Introspected {} parameters for JamSynth CLAP:", params.len());
    for p in &params {
        println!("  - {}: val={}, min={}, max={}", p.0, p.1, p.2, p.3);
    }
    assert!(params.len() >= 9, "JamSynth should expose at least 9 parameters via CLAP");

    // Test parameter tweak
    clap_track.set_parameter("Cutoff", 500.0);
    clap_track.set_parameter("Resonance", 0.7);

    // Play a note and process audio block
    clap_track.note_on(60, 1.0);
    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    clap_track.process_block(&mut left, &mut right);

    let max_l: f32 = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
    let max_r: f32 = right.iter().map(|s| s.abs()).fold(0.0, f32::max);
    println!("JamSynth CLAP audio output: max_l={}, max_r={}", max_l, max_r);
    assert!(max_l > 0.0001 || max_r > 0.0001, "Expected audio output from JamSynth CLAP");
}

#[test]
fn test_jamsynth_clap_track_lifecycle_and_presets() {
    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.add_custom_track("synth1", "JamSynth Track", "synth:reese");

    assert_eq!(host.tracks.len(), 1);
    assert_eq!(host.tracks[0].current_plugin.as_deref(), Some("JamSynth (Dark Reese)"));

    // Check parameters
    let params = host.tracks[0].get_parameters();
    assert!(params.len() >= 9, "Expected 9 parameters on JamSynth track");

    // Test saving and loading preset
    let temp_preset = std::env::temp_dir().join("test_reese_preset.json");
    host.tracks[0].save_patch_to_file(&temp_preset).expect("Failed to save preset");
    assert!(temp_preset.exists());

    // Modify parameter live
    host.tracks[0].set_parameter("Resonance", 0.9);

    // Reload preset from file
    host.tracks[0].load_patch_from_file(&temp_preset).expect("Failed to load preset");

    // Trigger note and render
    host.tracks[0].note_on(48, 1.0);
    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    host.process_block(&mut left, &mut right);

    let max_l: f32 = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
    let max_r: f32 = right.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(max_l > 0.0001 || max_r > 0.0001, "Expected audio from JamSynth CLAP track");

    let _ = std::fs::remove_file(temp_preset);
}

#[test]
fn test_robust_jamseq_features_suite() {
    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.add_custom_track("track_a", "Track A", "synth:sub808");
    host.add_custom_track("track_b", "Track B", "synth:reese");
    host.add_custom_track("track_c", "Track C", "audio");

    assert_eq!(host.tracks.len(), 3);
    assert_eq!(host.track_map.get("track_a"), Some(&0));
    assert_eq!(host.track_map.get("track_b"), Some(&1));
    assert_eq!(host.track_map.get("track_c"), Some(&2));

    // Test Reordering tracks: move track_c (idx 2) to beginning (idx 0)
    assert!(host.reorder_tracks(2, 0));
    assert_eq!(host.tracks[0].id, "track_c");
    assert_eq!(host.tracks[1].id, "track_a");
    assert_eq!(host.tracks[2].id, "track_b");
    assert_eq!(host.track_map.get("track_c"), Some(&0));
    assert_eq!(host.track_map.get("track_a"), Some(&1));
    assert_eq!(host.track_map.get("track_b"), Some(&2));

    // Test Deleting a track
    assert!(host.delete_track("track_a"));
    assert_eq!(host.tracks.len(), 2);
    assert!(host.track_map.get("track_a").is_none());
    assert_eq!(host.track_map.get("track_c"), Some(&0));
    assert_eq!(host.track_map.get("track_b"), Some(&1));

    // Test AutomationPoint dispatching on timeline
    host.timeline.add_event(jamseq::ScheduledEvent::new(
        999,
        0,
        "track_b",
        jamseq::ScheduledAction::SetParam {
            param_name: "cutoff".to_string(),
            value: 800.0,
        },
    ));

    host.play();
    let mut out_l = vec![0.0f32; 256];
    let mut out_r = vec![0.0f32; 256];
    host.process_block(&mut out_l, &mut out_r);

    // Verify time signature calculation
    let sig_3_4 = jamseq::TimeSignature { numerator: 3, denominator: 4 };
    assert_eq!(sig_3_4.ticks_per_bar(), 3 * jamseq::PPQ);

    let sig_7_8 = jamseq::TimeSignature { numerator: 7, denominator: 8 };
    assert_eq!(sig_7_8.ticks_per_bar(), (7 * jamseq::PPQ * 4) / 8);
}

#[test]
fn test_master_inserts_and_aux_send_routing() {
    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.add_custom_track("track_lead", "Lead Synth", "synth:reese");
    host.add_custom_track("track_bass", "Sub Bass", "synth:sub808");

    // 1. Create Aux Bus
    host.add_aux_bus("aux_reverb", "Space Reverb");
    assert_eq!(host.aux_buses.len(), 1);
    assert_eq!(host.aux_map.get("aux_reverb"), Some(&0));

    // 2. Set post-fader send from Lead Synth to Aux Reverb (gain 0.5)
    assert!(host.set_track_aux_send("track_lead", "aux_reverb", 0.5, false));
    let lead_idx = host.track_map["track_lead"];
    assert_eq!(host.tracks[lead_idx].sends.len(), 1);
    assert_eq!(host.tracks[lead_idx].sends[0].gain, 0.5);
    assert!(!host.tracks[lead_idx].sends[0].is_pre_fader);

    // 3. Play note on Lead Synth and process block
    host.tracks[lead_idx].note_on(60, 1.0);
    let mut out_l = vec![0.0f32; 256];
    let mut out_r = vec![0.0f32; 256];
    host.process_block(&mut out_l, &mut out_r);

    // Verify audio arrived in both Master Output and Aux Bus buffer
    let aux_peak: f32 = host.aux_left[0].iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
    let master_peak: f32 = out_l.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
    assert!(aux_peak > 0.001, "Expected audio sent into aux_reverb buffer, got {}", aux_peak);
    assert!(master_peak > 0.001, "Expected audio in Master output, got {}", master_peak);

    // 4. Test pre-fader send: Mute track or set volume to 0.0
    // Pre-fader send should STILL feed audio into the aux bus even when track volume is 0!
    host.tracks[lead_idx].volume = 0.0;
    host.set_track_aux_send("track_lead", "aux_reverb", 0.8, true); // pre-fader

    let mut out_l2 = vec![0.0f32; 256];
    let mut out_r2 = vec![0.0f32; 256];
    host.process_block(&mut out_l2, &mut out_r2);

    let aux_peak_pre: f32 = host.aux_left[0].iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
    assert!(aux_peak_pre > 0.001, "Pre-fader send must route audio to aux bus when track volume is 0");

    // 5. Test Master Bus controls
    host.master_bus.pan = 1.0; // Hard right
    let mut out_l3 = vec![0.0f32; 256];
    let mut out_r3 = vec![0.0f32; 256];
    host.process_block(&mut out_l3, &mut out_r3);
    let peak_l: f32 = out_l3.iter().map(|s| s.abs()).fold(0.0, f32::max);
    let peak_r: f32 = out_r3.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(peak_l < 0.0001, "Hard right master pan should silence left channel");
    assert!(peak_r > 0.001, "Hard right master pan should output to right channel");

    // 6. Test removing Aux Bus
    host.remove_aux_bus("aux_reverb");
    assert_eq!(host.aux_buses.len(), 0);
    assert!(host.aux_map.get("aux_reverb").is_none());
    assert_eq!(host.tracks[lead_idx].sends.len(), 0, "Removing aux bus must purge track send entries");
}

#[test]
fn test_hardware_midi_input_dispatch() {
    let mut host = JamSeqHost::new(120.0, 44100.0);
    host.add_custom_track("hw_lead", "Hardware Lead", "synth:reese");
    let track_id = "hw_lead".to_string();

    // Initially unarmed: simulated MIDI events should not trigger notes
    host.midi_input.send_event(jamseq::midi_input::RawMidiEvent {
        status: 0x90, // Note On Ch 1
        data1: 60,   // Middle C
        data2: 100,  // Velocity
    });

    let mut out_l = vec![0.0f32; 256];
    let mut out_r = vec![0.0f32; 256];
    host.process_block(&mut out_l, &mut out_r);
    let peak_unarmed: f32 = out_l.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert_eq!(peak_unarmed, 0.0, "Unarmed track should not receive MIDI notes");

    // Arm the track and send Note On
    host.armed_track_id = Some(track_id.clone());
    host.midi_input.send_event(jamseq::midi_input::RawMidiEvent {
        status: 0x90, // Note On Ch 1
        data1: 60,   // Middle C
        data2: 100,  // Velocity
    });

    let mut out_l2 = vec![0.0f32; 256];
    let mut out_r2 = vec![0.0f32; 256];
    host.process_block(&mut out_l2, &mut out_r2);
    let peak_armed: f32 = out_l2.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(peak_armed > 0.001, "Armed track must receive and render incoming live MIDI Note On");

    // Send Note Off
    host.midi_input.send_event(jamseq::midi_input::RawMidiEvent {
        status: 0x80, // Note Off Ch 1
        data1: 60,
        data2: 0,
    });
    // Send CC 7 (Volume)
    host.midi_input.send_event(jamseq::midi_input::RawMidiEvent {
        status: 0xB0, // CC
        data1: 7,
        data2: 64,
    });
    host.process_block(&mut out_l2, &mut out_r2);
}

#[test]
fn test_inter_track_sidechain_ducking() {
    let mut host = JamSeqHost::new(120.0, 44100.0);

    // Track 1: Bass
    let mut bass = JamSynthEngine::new(44100.0);
    bass.load_patch(JamSynthPatch::preset_808_sub());
    host.add_track(Track::new_synth("bass", "Sub Bass", bass));

    // Track 2: Kick trigger
    let mut kick = JamSynthEngine::new(44100.0);
    kick.load_patch(JamSynthPatch::preset_808_sub());
    host.add_track(Track::new_synth("kick", "Kick Drum", kick));

    // When kick sends sidechain to bass, kick MUST run first to feed bass's sidechain!
    let configured = host.set_insert_sidechain("bass", 0, "kick", 1.0);
    assert!(configured, "Failed to configure sidechain routing");

    let order = host.get_track_processing_order();
    let kick_idx = *host.track_map.get("kick").unwrap();
    let bass_idx = *host.track_map.get("bass").unwrap();
    let kick_pos = order.iter().position(|&x| x == kick_idx).unwrap();
    let bass_pos = order.iter().position(|&x| x == bass_idx).unwrap();
    assert!(kick_pos < bass_pos, "Sidechain source (kick) must be processed BEFORE target (bass)");

    // 1. Play bass alone (kick is quiet)
    host.tracks[bass_idx].note_on(36, 1.0);
    let mut out_l_solo = vec![0.0f32; 256];
    let mut out_r_solo = vec![0.0f32; 256];
    host.process_block(&mut out_l_solo, &mut out_r_solo);
    let peak_bass_unducked: f32 = out_l_solo.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(peak_bass_unducked > 0.05, "Bass note should produce audio");

    // 2. Play kick simultaneously with sidechain routing
    // Set kick track fader output to near zero so master level reflects ducked bass
    host.tracks[kick_idx].volume = 0.001;
    host.tracks[kick_idx].note_on(36, 1.0);
    let mut out_l_sc = vec![0.0f32; 256];
    let mut out_r_sc = vec![0.0f32; 256];
    host.process_block(&mut out_l_sc, &mut out_r_sc);
    let peak_bass_ducked: f32 = out_l_sc.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(peak_bass_ducked < peak_bass_unducked, "Bass should be ducked by incoming kick sidechain signal");

    // 3. Remove sidechain and verify removal
    let removed = host.remove_insert_sidechain("bass", 0);
    assert!(removed);
    assert_eq!(host.tracks[kick_idx].sidechains.len(), 0);
}

#[test]
fn test_track_freeze_and_unfreeze() {
    let mut host = JamSeqHost::new(120.0, 44100.0);
    let mut pad = JamSynthEngine::new(44100.0);
    pad.load_patch(JamSynthPatch::preset_cold_pad());
    host.add_track(Track::new_synth("pad", "Pad", pad));

    // Schedule events
    host.timeline.events.push(ScheduledEvent::new(1, 0, "pad", ScheduledAction::NoteOn { note: 60, velocity: 1.0 }));
    host.timeline.events.push(ScheduledEvent::new(2, 4 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOff { note: 60 }));
    host.timeline.seek_tick(0);

    // Freeze track
    let wav_path_str = host.freeze_track("pad").expect("Failed to freeze track");
    let wav_path = std::path::Path::new(&wav_path_str);
    assert!(wav_path.exists(), "Frozen WAV file must exist on disk");

    let pad_idx = *host.track_map.get("pad").unwrap();
    assert!(host.tracks[pad_idx].is_frozen, "Track must be marked as frozen");
    assert!(matches!(host.tracks[pad_idx].instrument, jamseq::track::TrackInstrument::AudioClips(_)), "Instrument must be replaced with AudioClip player");

    // Test playback of frozen track
    host.timeline.seek_tick(0);
    host.timeline.play();
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    host.process_block(&mut out_l, &mut out_r);
    let peak_frozen: f32 = out_l.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(peak_frozen > 0.001, "Frozen track must play audio from audio clip");

    // Unfreeze track
    host.unfreeze_track("pad").expect("Failed to unfreeze track");
    assert!(!host.tracks[pad_idx].is_frozen, "Track must not be marked frozen");
    assert!(matches!(host.tracks[pad_idx].instrument, jamseq::track::TrackInstrument::Synth(_)), "Original synth instrument must be restored");
}

#[test]
fn test_track_bouncing() {
    let mut host = JamSeqHost::new(120.0, 44100.0);
    let mut lead = JamSynthEngine::new(44100.0);
    lead.load_patch(JamSynthPatch::preset_reese_bass());
    host.add_track(Track::new_synth("lead", "Reese Lead", lead));

    // Schedule event
    host.timeline.events.push(ScheduledEvent::new(1, 0, "lead", ScheduledAction::NoteOn { note: 48, velocity: 1.0 }));
    host.timeline.events.push(ScheduledEvent::new(2, 4 * TICKS_PER_16TH, "lead", ScheduledAction::NoteOff { note: 48 }));

    let bounce_id = host.bounce_track("lead", "Lead Stem").expect("Failed to bounce track");
    assert!(host.track_map.contains_key(&bounce_id), "New bounced track must exist in host track map");

    let bounce_idx = *host.track_map.get(&bounce_id).unwrap();
    assert_eq!(host.tracks[bounce_idx].name, "Lead Stem");
    assert!(matches!(host.tracks[bounce_idx].instrument, jamseq::track::TrackInstrument::AudioClips(_)));
}

#[test]
fn test_autostem_mid_event_pause_and_unpause() {
    let mut host = JamSeqHost::new(120.0, 44100.0);
    let mut pad = JamSynthEngine::new(44100.0);
    pad.load_patch(JamSynthPatch::preset_cold_pad());
    host.add_track(Track::new_synth("pad_long", "Long Ambient Pad", pad));

    // Pad note starts at tick 0 and sustains for an entire bar (1920 ticks)
    host.timeline.events.push(ScheduledEvent::new(1, 0, "pad_long", ScheduledAction::NoteOn { note: 60, velocity: 1.0 }));
    host.timeline.events.push(ScheduledEvent::new(2, 1920, "pad_long", ScheduledAction::NoteOff { note: 60 }));
    host.timeline.events.sort_by_key(|e| e.tick);

    // Initial state: track is dirty
    let pad_idx = *host.track_map.get("pad_long").unwrap();
    assert!(host.tracks[pad_idx].stem_dirty);
    assert!(host.tracks[pad_idx].cached_stem.is_none());

    // 1. Bake stems
    let count = host.generate_stems_for_dirty_tracks();
    assert_eq!(count, 1, "Should generate exactly 1 dirty stem");
    assert!(!host.tracks[pad_idx].stem_dirty);
    assert!(host.tracks[pad_idx].cached_stem.is_some(), "Stem clip must be attached to track");

    // 2. Play from tick 0 to halfway through the note (tick 960)
    host.timeline.seek_tick(0);
    host.timeline.play();
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    host.process_block(&mut out_l, &mut out_r);

    // 3. Pause playback mid-note (at tick 960)
    // In a naive MIDI engine, pause silences all notes. When resuming at tick 960 without stems,
    // the synth would produce 0 audio because NoteOn was at tick 0!
    host.timeline.seek_tick(960);
    host.pause();

    // 4. Unpause at tick 960
    host.timeline.play();
    out_l.fill(0.0);
    out_r.fill(0.0);
    host.process_block(&mut out_l, &mut out_r);

    let peak_mid_note: f32 = out_l.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(
        peak_mid_note > 0.001,
        "Unpausing mid-note must seamlessly continue playing the pad from the autogenerated stem"
    );

    // 5. Test dirty invalidation on parameter change
    host.mark_track_dirty("pad_long");
    assert!(host.tracks[pad_idx].stem_dirty);
    assert!(host.tracks[pad_idx].cached_stem.is_none(), "Editing must invalidate cached stem for live editing");

    // 6. Test pause automatically regenerates dirty stems
    host.pause();
    assert!(!host.tracks[pad_idx].stem_dirty);
    assert!(host.tracks[pad_idx].cached_stem.is_some(), "Pause must auto-bake dirty stems");
}

#[test]
fn test_debug_playback_demo() {
    let mut host = JamSeqHost::new(68.0, 48000.0);
    let sr = 48000.0;
    if let Some(jamsynth_path) = jamseq::plugin_scanner::find_jamsynth_plugin() {
        if let Ok(bass_clap) = jamseq::clap_host::ClapTrack::load(&jamsynth_path.to_string_lossy(), sr) {
            host.add_track(Track::new_clap("bass", "Sub 808 (Drake Slide)", bass_clap, "JamSynth (Sub 808)"));
        } else {
            let mut bass_engine = JamSynthEngine::new(sr);
            bass_engine.load_patch(JamSynthPatch::preset_808_sub());
            host.add_track(Track::new_synth("bass", "Sub 808 (Drake Slide)", bass_engine));
        }

        if let Ok(pad_clap) = jamseq::clap_host::ClapTrack::load(&jamsynth_path.to_string_lossy(), sr) {
            host.add_track(Track::new_clap("pad", "Cold Pad (D-Minor 9th)", pad_clap, "JamSynth (Cold Pad)"));
        } else {
            let mut pad_engine = JamSynthEngine::new(sr);
            pad_engine.load_patch(JamSynthPatch::preset_cold_pad());
            host.add_track(Track::new_synth("pad", "Cold Pad (D-Minor 9th)", pad_engine));
        }
    } else {
        let mut bass_engine = JamSynthEngine::new(sr);
        bass_engine.load_patch(JamSynthPatch::preset_808_sub());
        host.add_track(Track::new_synth("bass", "Sub 808 (Drake Slide)", bass_engine));

        let mut pad_engine = JamSynthEngine::new(sr);
        pad_engine.load_patch(JamSynthPatch::preset_cold_pad());
        host.add_track(Track::new_synth("pad", "Cold Pad (D-Minor 9th)", pad_engine));
    }

    let avl_uri = "http://gareus.org/oss/lv2/avldrums#BlackPearl";
    if let Ok(drums_lv2) = jamseq::lv2_host::Lv2Track::load(avl_uri, sr) {
        host.add_track(Track::new_lv2("drums", "Drums (Black Pearl)", drums_lv2, "Black Pearl Drumkit"));
    }

    // Schedule 4 bars identical to main.rs
    let mut events = Vec::new();
    let mut next_id = 1u64;
    let mut add_ev = |events: &mut Vec<ScheduledEvent>, tick: u64, track_id: &str, action: ScheduledAction| {
        events.push(ScheduledEvent::new(next_id, tick, track_id, action));
        next_id += 1;
    };
    add_ev(&mut events, 0, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 });
    add_ev(&mut events, 6 * 240, "bass", ScheduledAction::NoteOff { note: 50 });
    add_ev(&mut events, 0, "drums", ScheduledAction::NoteOn { note: 36, velocity: 1.0 });
    add_ev(&mut events, 120, "drums", ScheduledAction::NoteOff { note: 36 });
    for &n in &[50, 53, 57, 60, 64] {
        add_ev(&mut events, 4 * 240, "pad", ScheduledAction::NoteOn { note: n, velocity: 0.7 });
        add_ev(&mut events, 12 * 240, "pad", ScheduledAction::NoteOff { note: n });
    }

    let arr = jamseq::timeline::TimelineArrangement {
        title: "Test".to_string(),
        bpm: 68.0,
        time_signature: jamseq::timeline::TimeSignature::default(),
        total_bars: 4,
        loop_enabled: true,
        tracks: Vec::new(),
        master_bus: None,
        aux_buses: Vec::new(),
        sections: vec![jamseq::SongSection::new("verse", "Verse", 1, 4)],
        events,
    };
    host.load_arrangement(arr);

    for (i, t) in host.tracks.iter().enumerate() {
        if let Some(ref stem) = t.cached_stem {
            if let Some(ref buf) = stem.buffer {
                let max_l: f32 = buf.samples_left.iter().map(|s| s.abs()).fold(0.0, f32::max);
                println!("Track {} ({}) stem len={}, max_l={}", i, t.id, buf.samples_left.len(), max_l);
            }
        } else {
            println!("Track {} ({}) has NO cached stem", i, t.id);
        }
    }

    host.set_sample_rate(44100.0);

    // Verify that while stopped, there is ZERO audio output (zero hum)
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    host.process_block(&mut out_l, &mut out_r);
    let stopped_peak: f32 = out_l.iter().chain(out_r.iter()).map(|s| s.abs()).fold(0.0, f32::max);
    assert!(stopped_peak < 1e-6, "Stopped engine must produce zero hum or audible audio (got {})", stopped_peak);

    host.play();
    let mut out_r = vec![0.0f32; 512];
    let mut total_peak = 0.0f32;
    for block in 0..30 {
        host.process_block(&mut out_l, &mut out_r);
        let peak: f32 = out_l.iter().chain(out_r.iter()).map(|s| s.abs()).fold(0.0, f32::max);
        if peak > total_peak {
            total_peak = peak;
        }
        if block == 0 || block == 5 || block == 15 {
            println!("Block {}: peak = {}", block, peak);
        }
    }
    println!("Total peak over 30 blocks: {}", total_peak);
    assert!(total_peak > 0.001, "Playback must produce audible output!");
}

#[test]
fn test_server_playback_over_ipc() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    let temp_dir = std::env::temp_dir();
    let sock_path = temp_dir.join("test_server_playback.sock");
    let _ = std::fs::remove_file(&sock_path);

    let mut host = JamSeqHost::new(68.0, 48000.0);
    let mut bass = JamSynthEngine::new(48000.0);
    bass.load_patch(JamSynthPatch::preset_808_sub());
    host.add_track(Track::new_synth("bass", "Sub 808", bass));

    let mut events = Vec::new();
    events.push(ScheduledEvent::new(1, 0, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 }));
    events.push(ScheduledEvent::new(2, 6 * 240, "bass", ScheduledAction::NoteOff { note: 50 }));

    let arr = jamseq::timeline::TimelineArrangement {
        title: "Test".to_string(),
        bpm: 68.0,
        time_signature: jamseq::timeline::TimeSignature::default(),
        total_bars: 4,
        loop_enabled: true,
        tracks: Vec::new(),
        master_bus: None,
        aux_buses: Vec::new(),
        sections: vec![jamseq::SongSection::new("verse", "Verse", 1, 4)],
        events,
    };
    host.load_arrangement(arr);

    let host_arc = Arc::new(Mutex::new(host));

    let server = jamseq::ipc::IpcServer::new(&sock_path);
    server.start(Arc::clone(&host_arc)).expect("Failed to start IPC server");
    thread::sleep(Duration::from_millis(50));

    // Connect client
    let mut stream = UnixStream::connect(&sock_path).expect("Failed to connect");
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    // 1. Get status before play
    writeln!(stream, "{{\"cmd\":\"GetStatus\"}}").unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&line).unwrap();
    assert!(!resp.status.unwrap().is_playing);

    // 2. Send Play command
    writeln!(stream, "{{\"cmd\":\"Play\"}}").unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let resp: jamseq::ipc::IpcResponse = serde_json::from_str(&line).unwrap();
    let status = resp.status.unwrap();
    println!("Status after Play: is_playing={}", status.is_playing);
    assert!(status.is_playing);

    // 3. Process blocks as CPAL would
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    let mut total_peak = 0.0f32;
    for _ in 0..10 {
        let mut h = host_arc.lock().unwrap();
        h.process_block(&mut out_l, &mut out_r);
        let peak: f32 = out_l.iter().chain(out_r.iter()).map(|s| s.abs()).fold(0.0, f32::max);
        if peak > total_peak {
            total_peak = peak;
        }
    }
    println!("Total peak in IPC play test: {}", total_peak);
    assert!(total_peak > 0.001);
}



#[test]
fn test_vst3_scanner_discovery() {
    let surge_path = "/usr/lib/vst3/Surge XT.vst3";
    if !std::path::Path::new(surge_path).exists() {
        println!("Skipping VST3 scanner test: {} not found", surge_path);
        return;
    }

    let plugins = jamseq::plugin_scanner::PluginScanner::scan_all();
    let found = plugins.iter().any(|p| p.plugin_format == "VST3" && p.name.contains("Surge XT"));
    assert!(found, "PluginScanner failed to discover Surge XT VST3 plugin in /usr/lib/vst3");
}

#[test]
fn test_vst3_track_in_engine_and_host() {
    let surge_path = "/usr/lib/vst3/Surge XT.vst3";
    if !std::path::Path::new(surge_path).exists() {
        println!("Skipping VST3 engine test: {} not found", surge_path);
        return;
    }

    let mut host = jamseq::JamSeqHost::new(120.0, 44100.0);
    // Switch or add track
    host.add_custom_track("vst_test", "Surge Track", surge_path);
    assert_eq!(host.tracks.len(), 1);

    let track_idx = *host.track_map.get("vst_test").expect("Track not found");
    assert!(matches!(host.tracks[track_idx].instrument, jamseq::track::TrackInstrument::Vst3(_)));

    // Send NoteOn
    host.tracks[track_idx].note_on(60, 0.9);

    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    host.process_block(&mut out_l, &mut out_r);

    let peak: f32 = out_l.iter().chain(out_r.iter()).map(|s| s.abs()).fold(0.0, f32::max);
    println!("VST3 Engine track output peak: {}", peak);
    assert!(peak > 0.0001, "VST3 track did not produce audio in engine process_block");

    // Test parameter retrieval and preset saving/loading
    let params = host.tracks[track_idx].get_parameters();
    assert!(!params.is_empty(), "VST3 track should report parameters");

    let temp_preset = std::env::temp_dir().join("test_surge_preset.json");
    host.tracks[track_idx].save_patch_to_file(&temp_preset).expect("Failed to save patch");
    assert!(temp_preset.exists());

    host.tracks[track_idx].load_patch_from_file(&temp_preset).expect("Failed to load patch");
    let _ = std::fs::remove_file(temp_preset);
}

#[test]
fn test_vst3_insert_effect_on_track() {
    let surge_fx_path = "/usr/lib/vst3/Surge XT Effects.vst3";
    if !std::path::Path::new(surge_fx_path).exists() {
        println!("Skipping VST3 effect test: {} not found", surge_fx_path);
        return;
    }

    let mut host = jamseq::JamSeqHost::new(120.0, 44100.0);
    // Add a synth track first
    let mut bass = jamsynth::JamSynthEngine::new(44100.0);
    bass.load_patch(jamsynth::JamSynthPatch::preset_808_sub());
    host.add_track(jamseq::track::Track::new_synth("track_0", "Sub Bass", bass));

    // Add VST3 effect to track 0
    let success = host.add_track_insert("track_0", surge_fx_path, "Surge FX Insert");
    assert!(success, "Failed to add VST3 insert to track");

    let track_0 = &host.tracks[0];
    assert_eq!(track_0.inserts.len(), 1);

    // Play note on track 0
    host.tracks[0].note_on(60, 0.8);

    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];
    host.process_block(&mut out_l, &mut out_r);

    let peak: f32 = out_l.iter().chain(out_r.iter()).map(|s| s.abs()).fold(0.0, f32::max);
    println!("Track with VST3 insert audio output peak: {}", peak);
    assert!(peak > 0.0001, "Expected audio output from track through VST3 insert");
}

#[test]
fn test_track_insert_param_update() {
    let jamsynth_path = if std::path::Path::new("target/debug/libjamsynth.so").exists() {
        "target/debug/libjamsynth.so"
    } else if std::path::Path::new("../target/debug/libjamsynth.so").exists() {
        "../target/debug/libjamsynth.so"
    } else {
        panic!("libjamsynth.so not found");
    };

    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.add_custom_track("track_0", "Sub Bass", "synth:sub808");

    // Add insert effect to track 0
    let success = host.add_track_insert("track_0", jamsynth_path, "JamSynth Insert");
    assert!(success, "Failed to add JamSynth insert to track");
    assert_eq!(host.tracks[0].inserts.len(), 1);

    // Initial parameters
    let initial_params = host.tracks[0].inserts[0].get_parameters();
    let _initial_cutoff = initial_params.iter().find(|p| p.0.to_lowercase() == "cutoff").map(|p| p.1).unwrap_or(0.0);

    // Change parameter via set_track_insert_param
    let target_cutoff = 0.75;
    assert!(host.set_track_insert_param("track_0", 0, "cutoff", target_cutoff));

    // Verify parameter changed and persisted on the insert
    let updated_params = host.tracks[0].inserts[0].get_parameters();
    let updated_cutoff = updated_params.iter().find(|p| p.0.to_lowercase() == "cutoff").map(|p| p.1).unwrap_or(0.0);
    assert!((updated_cutoff - target_cutoff).abs() < 1e-3, "Expected cutoff to be updated to {}, got {}", target_cutoff, updated_cutoff);

    // Verify track stem is marked dirty
    assert!(host.tracks[0].stem_dirty);

    // Verify project persistence saves the updated insert parameters
    let tmp_path = std::env::temp_dir().join("test_insert_params.jam.json");
    host.save_project_to_file(&tmp_path).expect("Failed to save project");

    let mut new_host = JamSeqHost::new(120.0, 48000.0);
    new_host.load_project_from_file(&tmp_path).expect("Failed to load project");

    assert_eq!(new_host.tracks[0].inserts.len(), 1);
    let loaded_params = new_host.tracks[0].inserts[0].get_parameters();
    let loaded_cutoff = loaded_params.iter().find(|p| p.0.to_lowercase() == "cutoff").map(|p| p.1).unwrap_or(0.0);
    assert!((loaded_cutoff - target_cutoff).abs() < 1e-3, "Loaded insert parameter must match saved parameter");

    let _ = std::fs::remove_file(tmp_path);
}

#[test]
fn test_blank_slate_and_section_management() {
    let mut host = jamseq::JamSeqHost::new(120.0, 44100.0);
    assert_eq!(host.tracks.len(), 0, "Blank slate should start with 0 tracks");
    assert!(host.project_path.is_none(), "Blank slate should have no project path");
    assert_eq!(host.timeline.title, "Untitled");

    // Add a sub-bar section (e.g. 2-beat pickup = 960 ticks)
    let pickup = jamseq::timeline::SongSection::from_ticks("pickup", "Pickup", 0, 960);
    assert_eq!(pickup.start_tick(), 0);
    assert_eq!(pickup.length_ticks(), 960);
    assert_eq!(pickup.end_tick(), 960);

    host.add_section(pickup);
    assert_eq!(host.timeline.sections.len(), 1);

    // Add a 4-bar section starting at 960 ticks (4 bars * 1920 ticks = 7680 ticks)
    let verse = jamseq::timeline::SongSection::from_ticks("verse", "Verse 1", 960, 7680);
    host.add_section(verse);
    assert_eq!(host.timeline.sections.len(), 2);
    // Derived total_ticks is the sum of pickup (960) and verse (7680) = 8640 ticks!
    assert_eq!(host.timeline.total_ticks, 8640);

    // Add an outro section of length 5000
    let outro = jamseq::timeline::SongSection::from_ticks("outro", "Outro", 8640, 5000);
    host.add_section(outro);
    assert_eq!(host.timeline.total_ticks, 8640 + 5000);

    // Test bar/beat normalization
    let mut custom = jamseq::timeline::SongSection::from_ticks("custom", "Bridge", 123, 456);
    custom.normalize_to_beat();
    assert_eq!(custom.start_tick() % jamseq::timeline::PPQ, 0);
    assert_eq!(custom.length_ticks() % jamseq::timeline::PPQ, 0);

    custom.normalize_to_bar();
    let tpb = custom.ticks_per_bar();
    assert_eq!(custom.start_tick() % tpb, 0);
    assert_eq!(custom.length_ticks() % tpb, 0);

    // Test removing a section ripples the timeline derived length
    let removed = host.remove_section("pickup");
    assert!(removed);
    assert_eq!(host.timeline.sections.len(), 2);
    assert_eq!(host.timeline.total_ticks, 7680 + 5000);

    // Test new_project clears tracks and sections to blank slate
    let synth = jamsynth::JamSynthEngine::new(44100.0);
    host.add_track(jamseq::track::Track::new_synth("t1", "Synth", synth));
    assert_eq!(host.tracks.len(), 1);

    host.new_project("Fresh Song", 130.0);
    assert_eq!(host.tracks.len(), 0, "new_project must clear tracks to blank slate");
    assert_eq!(host.timeline.sections.len(), 0, "new_project must start with 0 sections");
    assert_eq!(host.timeline.total_ticks, 0, "new_project must start with 0 total_ticks");
    assert_eq!(host.timeline.title, "Fresh Song");
    assert_eq!(host.timeline.bpm, 130.0);
}

#[test]
fn test_block_hierarchy_and_tempo_inheritance() {
    use jamseq::timeline::{SongSection, TrackBlock, TempoOverride};

    // 1. An empty section has length 0 and no stored length
    let mut sec = SongSection::new_empty("v1", "Verse 1");
    assert_eq!(sec.length_ticks(), 0);

    // 2. Add a 4-bar drum beat (start offset 0, length 3840 ticks)
    let drum_block = TrackBlock::new("tb_drums", "drums", 3840);
    sec.set_block(drum_block);
    assert_eq!(sec.length_ticks(), 3840);

    // 3. Add an 8-bar synth block (start offset 0, length 7680 ticks) -> Section dynamically stretches!
    let synth_block = TrackBlock::new("tb_synth", "synth", 7680);
    sec.set_block(synth_block);
    assert_eq!(sec.length_ticks(), 7680);

    // 4. Add a lead guitar block starting at offset 4 bars (3840) with length 6 bars (5760)
    // Furthest-reaching extent is 3840 + 5760 = 9600 ticks!
    let lead_block = TrackBlock::new("tb_lead", "lead", 5760).with_offset(3840);
    sec.set_block(lead_block);
    assert_eq!(sec.length_ticks(), 9600);

    // 5. Test Hierarchical Tempo Inheritance
    let project_bpm = 120.0;
    // Section without override inherits project BPM
    assert_eq!(sec.effective_bpm(project_bpm), 120.0);

    // Section with AbsoluteBpm override
    sec.tempo = Some(TempoOverride::AbsoluteBpm(90.0));
    assert_eq!(sec.effective_bpm(project_bpm), 90.0);

    // Block with Ratio override (3:2 metric modulation)
    let mut poly_block = TrackBlock::new("tb_poly", "poly", 1920)
        .with_tempo(TempoOverride::Ratio(3, 2));
    assert_eq!(poly_block.effective_bpm(sec.effective_bpm(project_bpm)), 135.0);

    // Block with Ratio override (1:2 half-time)
    poly_block.tempo = Some(TempoOverride::Ratio(1, 2));
    assert_eq!(poly_block.effective_bpm(sec.effective_bpm(project_bpm)), 45.0);

    // Block without override inherits section BPM
    poly_block.tempo = None;
    assert_eq!(poly_block.effective_bpm(sec.effective_bpm(project_bpm)), 90.0);

    // 6. Test Timeline Derived Geometry and Dynamic Rippling
    let mut timeline = jamseq::Timeline::new(120.0, 48000.0);
    assert_eq!(timeline.total_ticks, 0);

    // Section 1: Intro (8 bars = 7680 ticks via synth)
    let mut intro = SongSection::new_empty("intro", "Intro");
    intro.set_block(TrackBlock::new("b1", "synth", 7680));
    timeline.add_section(intro);
    assert_eq!(timeline.section_start_tick(0), 0);
    assert_eq!(timeline.total_ticks, 7680);

    // Section 2: Verse 1 (4 bars = 3840 ticks via drums)
    let mut verse = SongSection::new_empty("verse", "Verse");
    verse.set_block(TrackBlock::new("b2", "drums", 3840));
    timeline.add_section(verse);
    assert_eq!(timeline.section_start_tick(1), 7680);
    assert_eq!(timeline.total_ticks, 7680 + 3840);

    // Section 3: Chorus 1 (8 bars = 7680 ticks)
    let mut chorus = SongSection::new_empty("chorus", "Chorus");
    chorus.set_block(TrackBlock::new("b3", "full", 7680));
    timeline.add_section(chorus);
    assert_eq!(timeline.section_start_tick(2), 7680 + 3840);
    assert_eq!(timeline.total_ticks, 7680 + 3840 + 7680);

    // Dynamic Rippling: Extend Verse 1 by adding an 8-bar guitar track block (7680 ticks)
    timeline.set_track_block("verse", TrackBlock::new("b4", "guitar", 7680));
    // Verse 1 length expanded to 7680!
    // Downstream Chorus 1 automatically ripples from 11520 to 15360!
    assert_eq!(timeline.section_start_tick(2), 7680 + 7680);
    assert_eq!(timeline.total_ticks, 7680 + 7680 + 7680);
}

#[test]
fn test_polytempo_block_playback_and_loop_modes() {
    use jamseq::timeline::{LoopMode, SongSection, TempoOverride, TrackBlock};

    let mut timeline = jamseq::Timeline::new(120.0, 48000.0);

    // Section A (starts at 0)
    let mut sec_a = SongSection::new_empty("sec_a", "Section A");

    // Track 1: Synth (standard 120 BPM, repeats 2 times via FixedLoops)
    let mut synth_block = TrackBlock::new("tb_synth", "synth", 1920)
        .with_loop_mode(LoopMode::FixedLoops(2));
    synth_block.add_note(101, 102, 0, 480, 60, 0.8);
    sec_a.set_block(synth_block);

    // Track 2: Polytempo Lead (Ratio 3:2 = 180 BPM, starts at offset 960 ticks)
    let mut lead_block = TrackBlock::new("tb_lead", "lead", 1920)
        .with_offset(960)
        .with_tempo(TempoOverride::Ratio(3, 2))
        .with_loop_mode(LoopMode::OneShot);
    // Note in lead local time: duration 480 ticks
    lead_block.add_note(201, 202, 0, 480, 72, 0.9);
    sec_a.set_block(lead_block);

    // Track 3: Drum Loop (1 bar = 1920 ticks, LoopToSection to fill 3840 ticks)
    let mut drum_block = TrackBlock::new("tb_drums", "drums", 1920)
        .with_loop_mode(LoopMode::LoopToSection);
    drum_block.add_note(301, 302, 0, 240, 36, 1.0);
    sec_a.set_block(drum_block);

    timeline.add_section(sec_a);

    // Section extent: synth is 1920*2 = 3840 ticks. drums loop to 3840 ticks.
    // lead is 960 + 1920 = 2880 ticks.
    assert_eq!(timeline.total_ticks, 3840);

    // Verify materialized active events
    let events = timeline.materialize_all_events();

    // Synth: Loop 0 (NoteOn @ 0, NoteOff @ 480), Loop 1 (NoteOn @ 1920, NoteOff @ 2400)
    let synth_notes: Vec<&ScheduledEvent> = events.iter().filter(|e| e.track_id == "synth").collect();
    assert_eq!(synth_notes.len(), 4);
    assert_eq!(synth_notes[0].tick, 0);
    assert_eq!(synth_notes[1].tick, 480);
    assert_eq!(synth_notes[2].tick, 1920);
    assert_eq!(synth_notes[3].tick, 2400);

    // Lead (Ratio 3:2): NoteOn @ 960 (960 + 0).
    // Note duration scaled by 120/180 = 2/3: 480 * 2/3 = 320 ticks!
    // NoteOff @ 960 + 320 = 1280 ticks!
    let lead_notes: Vec<&ScheduledEvent> = events.iter().filter(|e| e.track_id == "lead").collect();
    assert_eq!(lead_notes.len(), 2);
    assert_eq!(lead_notes[0].tick, 960);
    assert_eq!(lead_notes[1].tick, 1280);

    // Drums (LoopToSection over 3840 ticks with base 1920): 2 iterations (0 and 1920)
    let drum_notes: Vec<&ScheduledEvent> = events.iter().filter(|e| e.track_id == "drums").collect();
    assert_eq!(drum_notes.len(), 4);
    assert_eq!(drum_notes[0].tick, 0);
    assert_eq!(drum_notes[1].tick, 240);
    assert_eq!(drum_notes[2].tick, 1920);
    assert_eq!(drum_notes[3].tick, 2160);

    // Test Audio Advance & Playback Cursors
    timeline.seek_tick(0);
    timeline.play();

    // Advance 480 samples at 48000 Hz = 0.01 seconds
    // At 120 BPM: 0.01 * 2 beats/sec * 960 PPQ = 19.2 ticks
    // Synth cursor advances at 120 BPM (19.2 ticks)
    // Lead cursor advances at 180 BPM = 0.01 * 3 beats/sec * 960 PPQ = 28.8 ticks
    let due = timeline.advance_samples(480);
    assert!(!due.is_empty(), "Events at tick 0 must fire on first advance");
    assert_eq!(due[0].tick, 0);

    let synth_cursor = timeline.track_cursors.get("synth").unwrap();
    let lead_cursor = timeline.track_cursors.get("lead").unwrap();

    assert!((synth_cursor.current_tick - 19.2).abs() < 0.1, "Synth cursor must advance at 120 BPM");
    assert!((lead_cursor.current_tick - 28.8).abs() < 0.1, "Lead cursor must advance at 180 BPM polytempo");

    // Test Host Integration with JamSeqHost audio block
    let mut host = JamSeqHost::new(120.0, 48000.0);
    let mut engine = JamSynthEngine::new(48000.0);
    engine.load_patch(JamSynthPatch::preset_reese_bass());
    host.add_track(Track::new_synth("poly_bass", "Poly Bass", engine));

    let mut bass_sec = SongSection::new_empty("b_sec", "Bass Section");
    let mut bass_block = TrackBlock::new("tb_bass", "poly_bass", 1920)
        .with_tempo(TempoOverride::Ratio(3, 2))
        .with_loop_mode(LoopMode::OneShot);
    bass_block.add_note(401, 402, 0, 480, 48, 0.9);
    bass_sec.set_block(bass_block);
    host.add_section(bass_sec);

    host.timeline.seek_tick(0);
    host.play();

    let mut left = vec![0.0f32; 512];
    let mut right = vec![0.0f32; 512];
    host.process_block(&mut left, &mut right);

    let peak = left.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(peak > 0.001, "Polytempo block must produce audio in host process_block");
}

#[test]
fn test_stem_cache_manager_and_granular_block_stems() {
    use jamseq::stem_cache::StemCacheManager;
    use jamseq::timeline::{LoopMode, SongSection, TempoOverride, TrackBlock};

    let cache_dir = std::env::temp_dir().join("jamseq_test_stem_cache_dir");
    let mut cache_mgr = StemCacheManager::new(&cache_dir).expect("Failed to create StemCacheManager");
    cache_mgr.clear().expect("Failed to clear test cache dir");

    // 1. Verify Content-Addressed Hash Stability & Invalidation
    let mut block1 = TrackBlock::new("block_1", "bass_track", 1920)
        .with_tempo(TempoOverride::AbsoluteBpm(120.0))
        .with_loop_mode(LoopMode::OneShot);
    block1.add_note(1, 2, 0, 480, 40, 1.0);

    let key1 = cache_mgr.compute_block_key("bass_track", "verse", &block1, Some("patch_a"), 120.0);
    let key1_repeat = cache_mgr.compute_block_key("bass_track", "verse", &block1, Some("patch_a"), 120.0);
    assert_eq!(key1, key1_repeat, "Identical block parameters must produce identical content hash");

    // Changing note pitch or velocity changes key
    let mut block2 = block1.clone();
    block2.events[0].action = jamseq::timeline::ScheduledAction::NoteOn { note: 41, velocity: 1.0 };
    let key2 = cache_mgr.compute_block_key("bass_track", "verse", &block2, Some("patch_a"), 120.0);
    assert_ne!(key1, key2, "Mutating a note must yield a distinct cache key");

    // Changing tempo override changes key
    let block3 = block1.clone().with_tempo(TempoOverride::Ratio(3, 2));
    let key3 = cache_mgr.compute_block_key("bass_track", "verse", &block3, Some("patch_a"), 180.0);
    assert_ne!(key1, key3, "Metric modulation tempo override must yield a distinct cache key");

    // 2. Store and Load Chunks
    let left_samples = vec![0.5f32; 512];
    let right_samples = vec![-0.5f32; 512];
    let stored_path = cache_mgr.store_chunk(&key1, &left_samples, &right_samples, 48000).expect("Store chunk failed");
    assert!(stored_path.exists(), "Stored chunk WAV must exist on disk");
    assert!(cache_mgr.get_cached_chunk(&key1).is_some());

    let loaded = cache_mgr.load_chunk(&key1).expect("Load chunk failed");
    assert_eq!(loaded.samples_left.len(), 512);
    assert_eq!(loaded.samples_right.len(), 512);
    assert!((loaded.samples_left[0] - 0.5).abs() < 0.01);
    assert!((loaded.samples_right[0] - (-0.5)).abs() < 0.01);

    // Invalidate track
    cache_mgr.invalidate_track("bass_track");
    assert!(cache_mgr.get_cached_chunk(&key1).is_none(), "Invalidated track chunks must be purged");

    // 3. Engine Granular Block Stem Rendering & Stitching
    let mut host = JamSeqHost::new(120.0, 48000.0);
    let mut bass = JamSynthEngine::new(48000.0);
    bass.load_patch(JamSynthPatch::preset_808_sub());
    host.add_track(Track::new_synth("sub808", "Sub 808", bass));

    // Section 1: Intro (1 bar = 3840 ticks)
    let mut sec_intro = SongSection::new_empty("sec_intro", "Intro");
    let mut b_intro = TrackBlock::new("tb_intro", "sub808", 3840);
    b_intro.add_note(10, 11, 0, 960, 36, 1.0);
    sec_intro.set_block(b_intro);
    host.add_section(sec_intro);

    // Section 2: Verse (1 bar = 3840 ticks)
    let mut sec_verse = SongSection::new_empty("sec_verse", "Verse");
    let mut b_verse = TrackBlock::new("tb_verse", "sub808", 3840);
    b_verse.add_note(20, 21, 0, 960, 38, 1.0);
    sec_verse.set_block(b_verse);
    host.add_section(sec_verse);

    // Initial stem generation for dirty tracks
    let generated = host.generate_stems_for_dirty_tracks();
    assert_eq!(generated, 1, "Must generate stem for sub808");

    let track_idx = *host.track_map.get("sub808").unwrap();
    assert!(!host.tracks[track_idx].stem_dirty, "Track stem must no longer be dirty");
    assert!(host.tracks[track_idx].cached_stem.is_some(), "Track must have stitched cached stem");

    // Stitched stem length should span both sections (Intro 3840 + Verse 3840 = 7680 ticks)
    let expected_samples = host.timeline.ticks_to_samples(7680) as usize;
    let actual_samples = host.tracks[track_idx].cached_stem.as_ref().unwrap().buffer.as_ref().unwrap().samples_left.len();
    assert_eq!(actual_samples, expected_samples, "Stitched stem length must match total arrangement ticks");

    // Calling again without modifications generates 0 stems
    let generated_again = host.generate_stems_for_dirty_tracks();
    assert_eq!(generated_again, 0, "Clean tracks must skip stem regeneration");

    // Verify individual block chunks are cached on disk
    let chunk_intro = host.render_track_block("sub808", "sec_intro").expect("Must load cached intro block");
    assert!(!chunk_intro.samples_left.is_empty(), "Cached intro block chunk must not be empty");

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_toast_json_block_ingestion() {
    let toast_path = std::path::Path::new("/home/dwalters/toast.json");
    if !toast_path.exists() {
        return;
    }

    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.load_project_from_file(toast_path).expect("Failed to load toast.json");

    assert_eq!(host.timeline.sections.len(), 3, "toast.json must have 3 sections");

    let intro = host.timeline.sections.iter().find(|s| s.id == "intro").unwrap();
    let verse = host.timeline.sections.iter().find(|s| s.id == "verse_1").unwrap();
    let chorus = host.timeline.sections.iter().find(|s| s.id == "chorus_1").unwrap();

    let intro_b = intro.track_blocks.get("track_1").expect("intro must have track_1 block");
    let verse_b = verse.track_blocks.get("track_1").expect("verse must have track_1 block");
    let chorus_b = chorus.track_blocks.get("track_1").expect("chorus must have track_1 block");

    // Verify each block has its own distinct notes
    assert!(!intro_b.events.is_empty(), "intro_b must have events migrated from toast.json");
    assert!(!verse_b.events.is_empty(), "verse_b must have events migrated from toast.json");
    assert!(!chorus_b.events.is_empty(), "chorus_b must have events migrated from toast.json");

    // Intro has note 56
    let intro_pitch = intro_b.events.iter().find_map(|e| match e.action {
        jamseq::timeline::ScheduledAction::NoteOn { note, .. } => Some(note),
        _ => None,
    });
    assert_eq!(intro_pitch, Some(56), "intro_b must contain pitch 56");

    // Verse has note 64
    let verse_pitch = verse_b.events.iter().find_map(|e| match e.action {
        jamseq::timeline::ScheduledAction::NoteOn { note, .. } => Some(note),
        _ => None,
    });
    assert_eq!(verse_pitch, Some(64), "verse_b must contain pitch 64");

    // Chorus has note 55
    let chorus_pitch = chorus_b.events.iter().find_map(|e| match e.action {
        jamseq::timeline::ScheduledAction::NoteOn { note, .. } => Some(note),
        _ => None,
    });
    assert_eq!(chorus_pitch, Some(55), "chorus_b must contain pitch 55");
}

#[test]
fn test_project_track_persistence_and_auto_discovery() {
    let temp_dir = std::env::temp_dir();
    let proj_file = temp_dir.join("test_track_discovery.jam.json");
    let _ = fs::remove_file(&proj_file);

    // 1. Simulate untitled.jam.json: JSON without a "tracks" field, referencing track_1 and track_2
    let raw_json = r#"{
        "title": "Untracked Project",
        "bpm": 120.0,
        "total_bars": 4,
        "loop_enabled": true,
        "sections": [
            {
                "id": "sec_intro",
                "name": "Intro",
                "time_signature": { "numerator": 4, "denominator": 4 },
                "tempo": null,
                "track_blocks": {
                    "track_1": {
                        "id": "intro_track_1",
                        "track_id": "track_1",
                        "start_offset_ticks": 0,
                        "base_length_ticks": 3840,
                        "loop_mode": "OneShot",
                        "tempo": null,
                        "events": [],
                        "audio_file": null
                    }
                },
                "start_tick": 0,
                "length_ticks": 3840,
                "start_bar": 1,
                "bars": 1
            },
            {
                "id": "sec_verse",
                "name": "Verse",
                "time_signature": { "numerator": 4, "denominator": 4 },
                "tempo": null,
                "track_blocks": {
                    "track_2": {
                        "id": "verse_track_2",
                        "track_id": "track_2",
                        "start_offset_ticks": 0,
                        "base_length_ticks": 3840,
                        "loop_mode": "OneShot",
                        "tempo": null,
                        "events": [],
                        "audio_file": null
                    }
                },
                "start_tick": 3840,
                "length_ticks": 3840,
                "start_bar": 2,
                "bars": 1
            }
        ],
        "events": [
            {
                "id": 1,
                "tick": 0,
                "track_id": "track_1",
                "action": { "NoteOn": { "note": 60, "velocity": 0.8 } }
            }
        ]
    }"#;
    fs::write(&proj_file, raw_json).expect("Failed to write test raw JSON");

    // 2. Load the project into a host
    let mut host = JamSeqHost::new(120.0, 44100.0);
    // Ensure host starts with 0 tracks
    host.tracks.clear();
    host.track_map.clear();
    assert_eq!(host.tracks.len(), 0);

    host.load_project_from_file(&proj_file).expect("Failed to load project from file");

    // Must have auto-discovered track_1 and track_2!
    assert_eq!(host.tracks.len(), 2, "Must discover exactly 2 tracks");
    assert!(host.track_map.contains_key("track_1"), "Must contain track_1");
    assert!(host.track_map.contains_key("track_2"), "Must contain track_2");
    assert_eq!(host.tracks[0].name, "MIDI Track 1");
    assert_eq!(host.tracks[1].name, "MIDI Track 2");

    // 3. Tweak parameters on track_1, master bus, and save
    host.tracks[0].name = "Lead Synth".to_string();
    host.tracks[0].volume = 0.75;
    host.tracks[0].set_parameter("cutoff", 0.72);
    host.tracks[0].set_parameter("resonance", 0.65);
    host.master_bus.volume = 0.88;

    let saved_file = temp_dir.join("test_track_saved.jam.json");
    let _ = fs::remove_file(&saved_file);
    host.save_project_to_file(&saved_file).expect("Failed to save project");

    // Verify saved JSON contains the "tracks" array, parameter values, and master_bus
    let saved_content = fs::read_to_string(&saved_file).expect("Failed to read saved file");
    assert!(saved_content.contains(r#""tracks""#), "Saved project must contain 'tracks' array");
    assert!(saved_content.contains("Lead Synth"), "Saved project must persist renamed track");
    assert!(saved_content.contains("parameters"), "Saved project must persist parameters");
    assert!(saved_content.contains("Cutoff") || saved_content.contains("cutoff"), "Saved project must persist cutoff parameter");
    assert!(saved_content.contains("master_bus"), "Saved project must persist master_bus");

    // 4. Reload into a new host and verify complete fidelity
    let mut reload_host = JamSeqHost::new(120.0, 44100.0);
    reload_host.load_project_from_file(&saved_file).expect("Failed to reload saved project");
    assert_eq!(reload_host.tracks.len(), 2);
    assert_eq!(reload_host.tracks[0].name, "Lead Synth");
    assert!((reload_host.tracks[0].volume - 0.75).abs() < 1e-4);
    assert!((reload_host.master_bus.volume - 0.88).abs() < 1e-4);

    let params = reload_host.tracks[0].get_parameters();
    let cutoff = params.iter().find(|(n, ..)| n == "Cutoff" || n == "cutoff").map(|(.., v, _, _)| *v).unwrap_or(0.0);
    let resonance = params.iter().find(|(n, ..)| n == "Resonance" || n == "resonance").map(|(.., v, _, _)| *v).unwrap_or(0.0);
    assert!((cutoff - 0.72).abs() < 1e-2, "Reloaded track must restore cutoff 0.72 (got {})", cutoff);
    assert!((resonance - 0.65).abs() < 1e-2, "Reloaded track must restore resonance 0.65 (got {})", resonance);

    let _ = fs::remove_file(&proj_file);
    let _ = fs::remove_file(&saved_file);
}

#[test]
fn test_cascading_meter_and_polymeter_hierarchy() {
    use jamseq::timeline::{SongSection, TimeSignature, TrackBlock, PPQ};

    let mut host = JamSeqHost::new(120.0, 44100.0);
    let synth1 = jamsynth::JamSynthEngine::new(44100.0);
    let synth2 = jamsynth::JamSynthEngine::new(44100.0);
    host.add_track(Track::new_synth("track_1", "Track 1", synth1));
    host.add_track(Track::new_synth("track_2", "Track 2", synth2));

    // 1. Initial State: Default Project Meter is 4/4
    assert_eq!(host.timeline.time_signature, TimeSignature { numerator: 4, denominator: 4 });
    assert_eq!(host.timeline.time_signature.ticks_per_bar(), 4 * PPQ);
    assert_eq!(host.timeline.time_signature.ticks_per_beat(), PPQ);

    // 2. Set Project Meter to 3/4
    host.set_project_time_signature(TimeSignature::new(3, 4));
    assert_eq!(host.timeline.time_signature, TimeSignature { numerator: 3, denominator: 4 });
    assert_eq!(host.timeline.time_signature.ticks_per_bar(), 3 * PPQ);
    assert_eq!(host.timeline.time_signature.ticks_per_beat(), PPQ);

    // 3. Section 1: Inherits project meter (time_signature = None)
    let sec1 = SongSection::from_ticks("sec_1", "Intro", 0, 3 * PPQ * 2); // 2 bars of 3/4
    assert_eq!(sec1.time_signature, None);
    assert_eq!(host.timeline.get_section_effective_time_signature(&sec1), TimeSignature { numerator: 3, denominator: 4 });

    // TrackBlock on track_1 inheriting Section 1 (which inherits Project 3/4)
    let blk1 = TrackBlock::new("blk_1", "track_1", 3 * PPQ * 2);
    assert_eq!(blk1.time_signature, None);
    assert_eq!(host.timeline.get_block_effective_time_signature(&sec1, "track_1"), TimeSignature { numerator: 3, denominator: 4 });

    host.add_section(sec1.with_block(blk1));

    // 4. Section 2: Overrides meter to 6/8
    let sec2 = SongSection::from_ticks("sec_2", "Verse", 3 * PPQ * 2, 2880 * 2)
        .with_time_signature(TimeSignature::new(6, 8));
    assert_eq!(sec2.time_signature, Some(TimeSignature { numerator: 6, denominator: 8 }));
    assert_eq!(host.timeline.get_section_effective_time_signature(&sec2), TimeSignature { numerator: 6, denominator: 8 });
    assert_eq!(sec2.ticks_per_bar(), (6 * PPQ * 4) / 8); // 2880 ticks per bar
    assert_eq!(sec2.time_signature().ticks_per_beat(), (PPQ * 4) / 8); // 480 ticks per beat

    // Block on Track 1 inherits Section 2 (6/8)
    let blk2_t1 = TrackBlock::new("blk_2_t1", "track_1", 2880 * 2);
    assert_eq!(blk2_t1.time_signature, None);
    assert_eq!(host.timeline.get_block_effective_time_signature(&sec2, "track_1"), TimeSignature { numerator: 6, denominator: 8 });

    // Block on Track 2 defines a TRUE POLYMETER OVERRIDE: 5/4 against the 6/8 section!
    let blk2_t2 = TrackBlock::new("blk_2_t2", "track_2", 4800)
        .with_time_signature(TimeSignature::new(5, 4));
    assert_eq!(blk2_t2.time_signature, Some(TimeSignature { numerator: 5, denominator: 4 }));
    assert_eq!(blk2_t2.effective_time_signature(sec2.time_signature()), TimeSignature { numerator: 5, denominator: 4 });
    assert_eq!(blk2_t2.time_signature.unwrap().ticks_per_bar(), 5 * PPQ); // 4800 ticks per bar
    assert_eq!(blk2_t2.time_signature.unwrap().ticks_per_beat(), PPQ); // 960 ticks per beat

    let sec2 = sec2.with_block(blk2_t1).with_block(blk2_t2);
    host.add_section(sec2);

    // 5. Query effective meter at different timeline positions
    // In Section 1 (tick 100): Project 3/4
    assert_eq!(host.timeline.get_time_signature_at_tick(100), TimeSignature { numerator: 3, denominator: 4 });
    assert_eq!(host.timeline.get_track_effective_time_signature("track_1", 100), TimeSignature { numerator: 3, denominator: 4 });
    assert_eq!(host.timeline.get_track_effective_time_signature("track_2", 100), TimeSignature { numerator: 3, denominator: 4 });

    // In Section 2 (tick 7000): Track 1 is 6/8, Track 2 is polymetric 5/4!
    let sec2_start = host.timeline.sections[0].length_ticks();
    assert_eq!(host.timeline.get_time_signature_at_tick(sec2_start + 100), TimeSignature { numerator: 6, denominator: 8 });
    assert_eq!(host.timeline.get_track_effective_time_signature("track_1", sec2_start + 100), TimeSignature { numerator: 6, denominator: 8 });
    assert_eq!(host.timeline.get_track_effective_time_signature("track_2", sec2_start + 100), TimeSignature { numerator: 5, denominator: 4 });

    // 6. Test serialization & persistence to file (.jam.json)
    let temp_dir = std::env::temp_dir();
    let meter_project_path = temp_dir.join("test_cascading_meter.jam.json");
    host.save_project_to_file(&meter_project_path).expect("Failed to save project with meter");

    // Inspect serialized JSON
    let content = std::fs::read_to_string(&meter_project_path).expect("Failed to read file");
    assert!(content.contains("\"numerator\": 3"), "Serialized arrangement must contain project numerator 3");
    assert!(content.contains("\"numerator\": 6"), "Serialized arrangement must contain section numerator 6");
    assert!(content.contains("\"numerator\": 5"), "Serialized arrangement must contain polymeter block numerator 5");

    // 7. Reload into a fresh host
    let mut reload_host = JamSeqHost::new(140.0, 44100.0);
    reload_host.load_project_from_file(&meter_project_path).expect("Failed to reload project");

    assert_eq!(reload_host.timeline.time_signature, TimeSignature { numerator: 3, denominator: 4 });
    assert_eq!(reload_host.timeline.sections.len(), 2);
    assert_eq!(reload_host.timeline.sections[0].time_signature, None); // Section 1 inherits 3/4
    assert_eq!(reload_host.timeline.sections[1].time_signature, Some(TimeSignature { numerator: 6, denominator: 8 })); // Section 2 is 6/8

    let reloaded_sec2 = &reload_host.timeline.sections[1];
    let reloaded_t1_block = reloaded_sec2.track_blocks.get("track_1").unwrap();
    let reloaded_t2_block = reloaded_sec2.track_blocks.get("track_2").unwrap();
    assert_eq!(reloaded_t1_block.time_signature, None); // Inherits 6/8
    assert_eq!(reloaded_t2_block.time_signature, Some(TimeSignature { numerator: 5, denominator: 4 })); // Polymeter 5/4 preserved!

    let _ = std::fs::remove_file(&meter_project_path);
}

#[test]
fn test_param_grouping_and_typed_controls() {
    let host = std::sync::Arc::new(std::sync::Mutex::new(JamSeqHost::new(120.0, 48000.0)));
    {
        let mut h = host.lock().unwrap();
        h.add_custom_track("track_lead", "Lead Synth", "synth:reese");

        let lead_idx = h.track_map["track_lead"];
        let statuses = h.tracks[lead_idx].get_param_statuses();
        assert!(!statuses.is_empty(), "Expected param statuses on synth track");

        // Verify JamSynth parameters are categorized into Filter, Amp Envelope, and Voice
        let filter_params: Vec<_> = statuses.iter().filter(|p| p.group == "Filter").collect();
        let env_params: Vec<_> = statuses.iter().filter(|p| p.group == "Amp Envelope").collect();
        let voice_params: Vec<_> = statuses.iter().filter(|p| p.group == "Voice").collect();

        assert_eq!(filter_params.len(), 3); // cutoff, resonance, drive
        assert_eq!(env_params.len(), 4); // attack_ms, decay_ms, sustain, release_ms
        assert_eq!(voice_params.len(), 1); // glide_ms

        let cutoff = statuses.iter().find(|p| p.name.to_lowercase() == "cutoff").unwrap();
        assert!(cutoff.display_value.is_some());
        assert_eq!(cutoff.param_type, jamseq::ipc::ParamType::Float);
    }

    // Verify status struct over IPC contains the groups and types
    let resp = jamseq::ipc::execute_command(jamseq::ipc::IpcCommand::GetStatus, &host);
    assert!(resp.ok);
    let status_json = serde_json::to_string(&resp.status).unwrap();
    assert!(status_json.contains("\"group\":\"Filter\""));
    assert!(status_json.contains("\"group\":\"Amp Envelope\""));
    assert!(status_json.contains("\"param_type\":\"Float\""));

    // Verify project persistence of parameters
    let temp_path = std::env::temp_dir().join("test_param_persistence.jam.json");
    let saved_val = {
        let mut h = host.lock().unwrap();
        // Modify a parameter value
        h.tracks[0].set_parameter("cutoff", 880.0);
        let val = h.tracks[0].get_parameters().iter().find(|(n, _, _, _)| n.to_lowercase() == "cutoff").unwrap().1;
        h.save_project_to_file(&temp_path).expect("Failed to save project");
        val
    };

    let mut reload_host = JamSeqHost::new(120.0, 48000.0);
    reload_host.load_project_from_file(&temp_path).expect("Failed to load project");
    assert_eq!(reload_host.tracks.len(), 1);

    let reloaded_statuses = reload_host.tracks[0].get_param_statuses();
    let reloaded_cutoff = reloaded_statuses.iter().find(|p| p.name.to_lowercase() == "cutoff").unwrap();
    assert!((reloaded_cutoff.value - saved_val).abs() < 1e-4, "Cutoff value must be restored from project file");

    let _ = std::fs::remove_file(&temp_path);
}

#[test]
fn test_surge_xt_effects_enum_type() {
    let surge_clap = std::path::Path::new("/usr/lib/clap/Surge XT Effects.clap");
    if !surge_clap.exists() {
        return;
    }

    let mut host = JamSeqHost::new(120.0, 48000.0);
    host.add_custom_track("track_1", "Synth Track", "synth:reese");
    let added = host.add_track_insert("track_1", "/usr/lib/clap/Surge XT Effects.clap", "Surge XT Effects (CLAP)");
    assert!(added, "Surge XT Effects CLAP must load successfully");

    let lead_idx = host.track_map["track_1"];
    let statuses = host.tracks[lead_idx].inserts[0].get_param_statuses();
    let fx_type = statuses.iter().find(|p| p.name == "FX Type").expect("FX Type parameter must exist");

    match &fx_type.param_type {
        jamseq::ipc::ParamType::Enum { labels } => {
            assert!(labels.len() >= 20, "Surge XT Effects should discover at least 20 effect labels, discovered {}", labels.len());
            assert!(labels.iter().any(|l: &String| l.contains("Delay") || l.contains("Reverb")), "Discovered labels should contain Delay or Reverb");
        }
        other => panic!("Expected ParamType::Enum for 'FX Type', got {:?}", other),
    }

    // Test setting parameter to an enum index value and verifying display_value
    host.tracks[lead_idx].inserts[0].set_parameter("FX Type", 0.05);
    let updated_statuses = host.tracks[lead_idx].inserts[0].get_param_statuses();
    let updated_fx_type = updated_statuses.iter().find(|p| p.name == "FX Type").unwrap();
    assert!(updated_fx_type.display_value.as_ref().map(|s| s.contains("Reverb")).unwrap_or(false),
        "Setting FX Type to 0.05 should update display_value to Reverb, got: {:?}", updated_fx_type.display_value);
}

