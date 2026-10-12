use anyhow::{Context, Result};
use jamsynth::patch::JamSynthPatch;
use jamsynth::JamSynthEngine;
use jamseq::audio::AudioOutput;
use jamseq::engine::JamSeqHost;
use jamseq::timeline::{ScheduledAction, ScheduledEvent, TimelineArrangement, TICKS_PER_16TH};
use jamseq::track::Track;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    println!("================================================================================");
    println!("JamSeq v1.0 - Standalone Real-Time Multi-Track Sequencer & Host");
    println!("================================================================================");

    match command {
        "demo" => run_demo()?,
        "server" => {
            let socket_path = args.get(2).map(|s| s.as_str()).unwrap_or("/tmp/jamseq.sock");
            run_server(socket_path)?;
        }
        "render-demo" => {
            let wav_path = args.get(2).map(|s| s.as_str()).unwrap_or("export/the_white_flame_preview.wav");
            fs::create_dir_all("export")?;
            let mut host = build_white_flame_demo_host()?;
            println!("[jamseq] Bouncing 4 bars of The White Flame Verse 1 to: {}", wav_path);
            host.render_to_wav(Path::new(wav_path), 4)?;
            println!("[jamseq] ✓ Successfully rendered: {}", wav_path);
        }
        "render" => {
            let json_path = args.get(2).context("Usage: jamseq render <song.json> <output.wav> [bars]")?;
            let wav_path = args.get(3).context("Usage: jamseq render <song.json> <output.wav> [bars]")?;
            let bars: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(4);
            run_render(json_path, wav_path, bars)?;
        }
        "play" => {
            let json_path = args.get(2).context("Usage: jamseq play <song.json>")?;
            run_play_json(json_path)?;
        }
        _ => {
            println!("Usage:");
            println!("  jamseq server [socket_path]  Run headless audio host with Unix socket IPC server");
            println!("  jamseq demo                  Run a live demo of The White Flame groove (PipeWire/ALSA)");
            println!("  jamseq play <song.json>      Play an arrangement in real time");
            println!("  jamseq render <in> <out> [N] Offline render N bars to a 24-bit WAV file");
        }
    }

    Ok(())
}

fn run_server(socket_path: &str) -> Result<()> {
    println!("[jamseq:server] Starting JamSeq audio engine and IPC server on {}", socket_path);
    let host = Arc::new(Mutex::new(build_white_flame_demo_host()?));

    let _audio = AudioOutput::start(Arc::clone(&host))?;
    println!("[jamseq:server] ✓ Audio engine active (PipeWire / ALSA)");

    let server = jamseq::ipc::IpcServer::new(socket_path);
    server.start(Arc::clone(&host))?;
    println!("[jamseq:server] ✓ IPC server listening on {}", socket_path);
    println!("[jamseq:server] Ready for JamOut GUI client connections. Press Ctrl+C to stop.");

    loop {
        thread::sleep(Duration::from_millis(500));
    }
}

fn build_white_flame_demo_host() -> Result<JamSeqHost> {
    let mut host = JamSeqHost::new(68.0, 48000.0);

    // Track 1: Sub 808 (JamSynth)
    let mut bass_engine = JamSynthEngine::new(48000.0);
    bass_engine.load_patch(JamSynthPatch::preset_808_sub());
    host.add_track(Track::new_synth("bass", "Sub 808 (Drake Slide)", bass_engine));

    // Track 2: Cold Gothic Pad (JamSynth)
    let mut pad_engine = JamSynthEngine::new(48000.0);
    pad_engine.load_patch(JamSynthPatch::preset_cold_pad());
    host.add_track(Track::new_synth("pad", "Cold Pad (D-Minor 9th)", pad_engine));

    // Schedule 4 bars of The White Flame Verse 1
    let mut events = Vec::new();
    let mut next_id = 1u64;
    let mut add_ev = |events: &mut Vec<ScheduledEvent>, tick: u64, track_id: &str, action: ScheduledAction| {
        events.push(ScheduledEvent::new(next_id, tick, track_id, action));
        next_id += 1;
    };

    // --- BASS LINE (Locks to Kick + Drake 808 Slide on Bar 2) ---
    // Bar 1: D3 on beat 1 (tick 0), pickup on step 13 (tick 12*240 = 2880)
    add_ev(&mut events, 0, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 });
    add_ev(&mut events, 6 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 50 });

    add_ev(&mut events, 12 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 50, velocity: 0.9 });
    add_ev(&mut events, 15 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 50 });

    // Bar 2: Downbeat D3 (tick 3840), then DRAKE SLIDE up to D4 (62) on tick 4800!
    add_ev(&mut events, 16 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 });
    add_ev(&mut events, 20 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 62, velocity: 1.0 }); // Legato slide!
    add_ev(&mut events, 28 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 62 });

    // Bar 3: Return to D3
    add_ev(&mut events, 32 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 });
    add_ev(&mut events, 38 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 50 });

    add_ev(&mut events, 44 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 50, velocity: 0.9 });
    add_ev(&mut events, 47 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 50 });

    // Bar 4: Turnaround to F3 (53), then D3
    add_ev(&mut events, 48 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 53, velocity: 0.95 });
    add_ev(&mut events, 52 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 53 });

    add_ev(&mut events, 56 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOn { note: 50, velocity: 1.0 });
    add_ev(&mut events, 62 * TICKS_PER_16TH, "bass", ScheduledAction::NoteOff { note: 50 });

    // --- COLD PAD CHORDS (D Minor 9th & Bbmaj7 Swells) ---
    // Bar 1: Dm9 (D3, F3, A3, C4, E4) on Step 5 (Beat 2, tick 4*240=960) held for 8 steps
    for &n in &[50, 53, 57, 60, 64] {
        add_ev(&mut events, 4 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOn { note: n, velocity: 0.7 });
        add_ev(&mut events, 12 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOff { note: n });
    }

    // Bar 2: Dm7 on Step 21 (tick 20*240=4800)
    for &n in &[50, 53, 57, 60] {
        add_ev(&mut events, 20 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOn { note: n, velocity: 0.65 });
        add_ev(&mut events, 28 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOff { note: n });
    }

    // Bar 3: Dm9 on Step 37 (tick 36*240)
    for &n in &[50, 53, 57, 60, 64] {
        add_ev(&mut events, 36 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOn { note: n, velocity: 0.7 });
        add_ev(&mut events, 44 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOff { note: n });
    }

    // Bar 4: Bbmaj7 turnaround (Bb2, D3, F3, A3) on Step 53 sustaining through end of Bar 4
    for &n in &[46, 50, 53, 57] {
        add_ev(&mut events, 52 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOn { note: n, velocity: 0.7 });
        add_ev(&mut events, 64 * TICKS_PER_16TH, "pad", ScheduledAction::NoteOff { note: n });
    }

    let arrangement = TimelineArrangement {
        title: "The White Flame (Verse 1 Demo)".to_string(),
        bpm: 68.0,
        total_bars: 4,
        loop_enabled: true,
        sections: vec![jamseq::SongSection::new("verse_1", "Verse 1", 1, 4, 3)],
        events,
    };

    host.load_arrangement(arrangement);
    host.project_path = Some("doc/songs/the_white_flame.json".to_string());
    Ok(host)
}

fn run_demo() -> Result<()> {
    println!("[jamseq] Initializing real-time PipeWire / ALSA audio engine...");
    let host = Arc::new(Mutex::new(build_white_flame_demo_host()?));

    let audio = AudioOutput::start(Arc::clone(&host))
        .context("Could not start audio stream")?;

    {
        let mut h = host.lock().unwrap();
        h.play();
    }

    println!("[jamseq] ▶ PLAYING: 'The White Flame' (Verse 1 Demo @ 68 BPM)");
    println!("         • Track 1: Sub 808 (JamSynth with Drake slide D3 -> D4)");
    println!("         • Track 2: Cold Gothic Pad (JamSynth 8-voice polyphonic Dm9)");
    println!("         • Engine: Zero DAW, real-time PipeWire audio output");
    println!("         (Playing 1 loop iteration for 14 seconds...)");

    // Play 1 complete 4-bar loop at 68 BPM (4 bars @ 68 BPM = ~14.1 seconds)
    thread::sleep(Duration::from_millis(14200));

    {
        let mut h = host.lock().unwrap();
        h.pause();
    }

    println!("[jamseq] ■ Demo playback complete.");
    drop(audio);
    Ok(())
}

fn run_render(json_path: &str, wav_path: &str, bars: u32) -> Result<()> {
    println!("[jamseq] Reading arrangement: {}", json_path);
    let json_data = fs::read_to_string(json_path)?;
    let arrangement: TimelineArrangement = serde_json::from_str(&json_data)?;

    let mut host = build_white_flame_demo_host()?;
    host.load_arrangement(arrangement);

    println!("[jamseq] Offline bouncing {} bars to: {}", bars, wav_path);
    host.render_to_wav(Path::new(wav_path), bars)?;
    println!("[jamseq] ✓ Successfully rendered 24-bit stereo WAV!");
    Ok(())
}

fn run_play_json(json_path: &str) -> Result<()> {
    println!("[jamseq] Reading arrangement: {}", json_path);
    let json_data = fs::read_to_string(json_path)?;
    let arrangement: TimelineArrangement = serde_json::from_str(&json_data)?;

    let host = Arc::new(Mutex::new(build_white_flame_demo_host()?));
    {
        let mut h = host.lock().unwrap();
        h.load_arrangement(arrangement);
    }

    let _audio = AudioOutput::start(Arc::clone(&host))?;
    {
        let mut h = host.lock().unwrap();
        h.play();
    }

    println!("[jamseq] ▶ PLAYING. Press Ctrl+C to stop.");
    loop {
        thread::sleep(Duration::from_millis(100));
    }
}
