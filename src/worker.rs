use crate::sampler::SampleBuffer;
use crate::timeline::TrackBlock;
use crate::track::{InsertEffect, Track, TrackInstrument};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub enum InstrumentSnapshot {
    Synth { patch_json: String },
    Sampler(crate::sampler::SamplerInstrument),
    AudioClips(crate::audio_clip::AudioClipTrack),
    Clap { path: String, params: Vec<(String, f32)> },
    Lv2 { uri: String, params: Vec<(String, f32)> },
    Vst3 { path: String, params: Vec<(String, f32)> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum InsertKind {
    Clap,
    Lv2,
    Vst3,
}

#[derive(Clone, Debug)]
pub struct InsertSnapshot {
    pub name: String,
    pub kind: InsertKind,
    pub uri_or_path: String,
    pub params: Vec<(String, f32)>,
}

#[derive(Clone, Debug)]
pub struct TrackDslSnapshot {
    pub track_id: String,
    pub track_name: String,
    pub instrument: InstrumentSnapshot,
    pub inserts: Vec<InsertSnapshot>,
}

pub struct BakeJob {
    pub job_id: u64,
    pub track_id: String,
    pub block_id: String,
    pub section_id: Option<String>,
    pub abs_start_tick: u64,
    pub start_sample: usize,
    pub eff_bpm: f64,
    pub sample_rate: f32,
    pub block: TrackBlock,
    pub track_snapshot: TrackDslSnapshot,
}

pub struct BakeResult {
    pub job_id: u64,
    pub track_id: String,
    pub block_id: String,
    pub start_sample: usize,
    pub end_sample: usize,
    pub buffer: SampleBuffer,
}

pub struct StemBakeWorker {
    receiver: Receiver<BakeJob>,
    sender: Sender<BakeResult>,
    latest_jobs: Arc<Mutex<HashMap<(String, String), u64>>>,
    worker_tracks: HashMap<String, Track>,
}

impl StemBakeWorker {
    pub fn spawn(
        receiver: Receiver<BakeJob>,
        sender: Sender<BakeResult>,
        latest_jobs: Arc<Mutex<HashMap<(String, String), u64>>>,
    ) {
        std::thread::Builder::new()
            .name("stem-bake-worker".to_string())
            .spawn(move || {
                let mut worker = StemBakeWorker {
                    receiver,
                    sender,
                    latest_jobs,
                    worker_tracks: HashMap::new(),
                };
                worker.run();
            })
            .expect("Failed to spawn stem-bake-worker thread");
    }

    fn run(&mut self) {
        while let Ok(job) = self.receiver.recv() {
            // Coalesce / debounce: drain any queued jobs that supersede this one
            let mut current_job = job;
            let mut other_jobs = Vec::new();

            while let Ok(queued_job) = self.receiver.try_recv() {
                if queued_job.track_id == current_job.track_id
                    && queued_job.block_id == current_job.block_id
                {
                    // Superseded by a newer parameter update or edit for the same block
                    current_job = queued_job;
                } else {
                    other_jobs.push(queued_job);
                }
            }

            self.process_single_job(current_job);

            for queued in other_jobs {
                self.process_single_job(queued);
            }
        }
    }

    fn process_single_job(&mut self, job: BakeJob) {
        let key = (job.track_id.clone(), job.block_id.clone());
        {
            let map = self.latest_jobs.lock().unwrap();
            if let Some(&latest_id) = map.get(&key) {
                if job.job_id < latest_id {
                    // Stale job superseded by a newer job
                    return;
                }
            }
        }

        let track = match self.prepare_track(&job.track_snapshot, job.sample_rate) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[jamseq:worker] Error preparing track {}: {}", job.track_id, e);
                return;
            }
        };

        match crate::block_renderer::render_block(track, &job.block, job.eff_bpm, job.sample_rate) {
            Ok(buf) => {
                {
                    let map = self.latest_jobs.lock().unwrap();
                    if let Some(&latest_id) = map.get(&key) {
                        if job.job_id < latest_id {
                            // Superseded while rendering
                            return;
                        }
                    }
                }

                let end_sample = job.start_sample + buf.samples_left.len();
                let result = BakeResult {
                    job_id: job.job_id,
                    track_id: job.track_id,
                    block_id: job.block_id,
                    start_sample: job.start_sample,
                    end_sample,
                    buffer: buf,
                };
                let _ = self.sender.send(result);
            }
            Err(e) => {
                eprintln!("[jamseq:worker] Error rendering block {}: {}", job.block_id, e);
            }
        }
    }

    fn prepare_track<'a>(
        &'a mut self,
        snapshot: &TrackDslSnapshot,
        sr: f32,
    ) -> Result<&'a mut Track> {
        let tid = &snapshot.track_id;

        let needs_new_instrument = match self.worker_tracks.get(tid) {
            None => true,
            Some(t) => !is_instrument_matching(&t.instrument, &snapshot.instrument),
        };

        if needs_new_instrument {
            let new_track = create_track_from_snapshot(snapshot, sr)?;
            self.worker_tracks.insert(tid.clone(), new_track);
        }

        let track = self.worker_tracks.get_mut(tid).unwrap();
        apply_instrument_snapshot(&mut track.instrument, &snapshot.instrument)?;

        let inserts_match = track.inserts.len() == snapshot.inserts.len()
            && track.inserts.iter().zip(&snapshot.inserts).all(|(existing, snap)| {
                existing.uri() == snap.uri_or_path
            });

        if !inserts_match {
            track.inserts.clear();
            for ins_snap in &snapshot.inserts {
                if let Ok(ins) = create_insert_from_snapshot(ins_snap, sr) {
                    track.inserts.push(ins);
                }
            }
        } else {
            for (ins, ins_snap) in track.inserts.iter_mut().zip(&snapshot.inserts) {
                for (pname, pval) in &ins_snap.params {
                    ins.set_parameter(pname, *pval);
                }
            }
        }

        Ok(track)
    }
}

fn is_instrument_matching(existing: &TrackInstrument, snap: &InstrumentSnapshot) -> bool {
    match (existing, snap) {
        (TrackInstrument::Synth(_), InstrumentSnapshot::Synth { .. }) => true,
        (TrackInstrument::Sampler(_), InstrumentSnapshot::Sampler(_)) => true,
        (TrackInstrument::AudioClips(_), InstrumentSnapshot::AudioClips(_)) => true,
        (TrackInstrument::Clap(clap), InstrumentSnapshot::Clap { path, .. }) => clap.path == *path,
        (TrackInstrument::Lv2(lv2), InstrumentSnapshot::Lv2 { uri, .. }) => lv2.uri == *uri,
        (TrackInstrument::Vst3(vst3), InstrumentSnapshot::Vst3 { path, .. }) => vst3.path == *path,
        _ => false,
    }
}

fn create_track_from_snapshot(snapshot: &TrackDslSnapshot, sr: f32) -> Result<Track> {
    match &snapshot.instrument {
        InstrumentSnapshot::Synth { patch_json } => {
            let mut engine = jamsynth::JamSynthEngine::new(sr);
            if !patch_json.is_empty() {
                let _ = engine.load_patch_json(patch_json);
            }
            Ok(Track::new_synth(&snapshot.track_id, &snapshot.track_name, engine))
        }
        InstrumentSnapshot::Sampler(sampler) => {
            Ok(Track::new_sampler(&snapshot.track_id, &snapshot.track_name, sampler.clone()))
        }
        InstrumentSnapshot::AudioClips(clips) => {
            Ok(Track::new_audio_clips(&snapshot.track_id, &snapshot.track_name, clips.clone()))
        }
        InstrumentSnapshot::Clap { path, .. } => {
            match crate::clap_host::ClapTrack::load(path, sr) {
                Ok(inst) => Ok(Track::new_clap(&snapshot.track_id, &snapshot.track_name, inst, &snapshot.track_name)),
                Err(e) => {
                    eprintln!("[jamseq:worker] Fallback to JamSynth: failed to load CLAP {}: {}", path, e);
                    Ok(Track::new_synth(&snapshot.track_id, &snapshot.track_name, jamsynth::JamSynthEngine::new(sr)))
                }
            }
        }
        InstrumentSnapshot::Lv2 { uri, .. } => {
            match crate::lv2_host::Lv2Track::load(uri, sr) {
                Ok(inst) => Ok(Track::new_lv2(&snapshot.track_id, &snapshot.track_name, inst, &snapshot.track_name)),
                Err(e) => {
                    eprintln!("[jamseq:worker] Fallback to JamSynth: failed to load LV2 {}: {}", uri, e);
                    Ok(Track::new_synth(&snapshot.track_id, &snapshot.track_name, jamsynth::JamSynthEngine::new(sr)))
                }
            }
        }
        InstrumentSnapshot::Vst3 { path, .. } => {
            match crate::vst3_host::Vst3Track::load(path, sr) {
                Ok(inst) => Ok(Track::new_vst3(&snapshot.track_id, &snapshot.track_name, inst, &snapshot.track_name)),
                Err(e) => {
                    eprintln!("[jamseq:worker] Fallback to JamSynth: failed to load VST3 {}: {}", path, e);
                    Ok(Track::new_synth(&snapshot.track_id, &snapshot.track_name, jamsynth::JamSynthEngine::new(sr)))
                }
            }
        }
    }
}

fn create_insert_from_snapshot(snap: &InsertSnapshot, sr: f32) -> Result<InsertEffect> {
    match snap.kind {
        InsertKind::Clap => {
            let track = crate::clap_host::ClapTrack::load(&snap.uri_or_path, sr)?;
            let mut fx = InsertEffect::Clap {
                name: snap.name.clone(),
                uri: snap.uri_or_path.clone(),
                track: Box::new(track),
            };
            for (pname, pval) in &snap.params {
                fx.set_parameter(pname, *pval);
            }
            Ok(fx)
        }
        InsertKind::Lv2 => {
            let track = crate::lv2_host::Lv2Track::load(&snap.uri_or_path, sr)?;
            let mut fx = InsertEffect::Lv2 {
                name: snap.name.clone(),
                uri: snap.uri_or_path.clone(),
                track: Box::new(track),
            };
            for (pname, pval) in &snap.params {
                fx.set_parameter(pname, *pval);
            }
            Ok(fx)
        }
        InsertKind::Vst3 => {
            let track = crate::vst3_host::Vst3Track::load(&snap.uri_or_path, sr)?;
            let mut fx = InsertEffect::Vst3 {
                name: snap.name.clone(),
                uri: snap.uri_or_path.clone(),
                track: Box::new(track),
            };
            for (pname, pval) in &snap.params {
                fx.set_parameter(pname, *pval);
            }
            Ok(fx)
        }
    }
}

fn apply_instrument_snapshot(inst: &mut TrackInstrument, snap: &InstrumentSnapshot) -> Result<()> {
    match (inst, snap) {
        (TrackInstrument::Synth(synth), InstrumentSnapshot::Synth { patch_json }) => {
            if !patch_json.is_empty() {
                let _ = synth.load_patch_json(patch_json);
            }
        }
        (TrackInstrument::Sampler(sampler), InstrumentSnapshot::Sampler(new_samp)) => {
            *sampler = new_samp.clone();
        }
        (TrackInstrument::AudioClips(clips), InstrumentSnapshot::AudioClips(new_clips)) => {
            *clips = new_clips.clone();
        }
        (TrackInstrument::Clap(clap), InstrumentSnapshot::Clap { params, .. }) => {
            for (n, v) in params {
                clap.set_parameter(n, *v);
            }
        }
        (TrackInstrument::Lv2(lv2), InstrumentSnapshot::Lv2 { params, .. }) => {
            for (n, v) in params {
                lv2.set_parameter(n, *v);
            }
        }
        (TrackInstrument::Vst3(vst3), InstrumentSnapshot::Vst3 { params, .. }) => {
            for (n, v) in params {
                vst3.set_parameter(n, *v);
            }
        }
        _ => {}
    }
    Ok(())
}
