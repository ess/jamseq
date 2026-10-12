use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::host::HostError;
use clack_host::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc as StdArc, Mutex};

pub struct JamSeqClapMainThread {
    pub timers: StdArc<Mutex<Vec<TimerId>>>,
}

impl<'a> MainThreadHandler<'a> for JamSeqClapMainThread {}

impl HostTimerImpl for JamSeqClapMainThread {
    fn register_timer(&self, _period_ms: u32) -> Result<TimerId, HostError> {
        let mut timers = self.timers.lock().unwrap();
        let id = TimerId((timers.len() as u32) + 1);
        timers.push(id);
        Ok(id)
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        self.timers.lock().unwrap().retain(|&id| id != timer_id);
        Ok(())
    }
}

pub struct JamSeqClapHost;
impl HostHandlers for JamSeqClapHost {
    type Shared<'a> = ();
    type MainThread<'a> = JamSeqClapMainThread;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<'_, Self>, _shared: &()) {
        builder.register::<HostTimer>();
    }
}

#[derive(Clone, Debug)]
pub struct ClapParam {
    pub id: ClapId,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
}

pub struct ClapTrack {
    pub path: String,
    pub processor: StdArc<Mutex<StartedPluginAudioProcessor<JamSeqClapHost>>>,
    pub pending_events: StdArc<Mutex<Vec<(u8, f32, bool)>>>, // (note, vel, is_note_on)
    pub pending_params: StdArc<Mutex<Vec<(ClapId, f64)>>>,   // (param_id, value)
    pub params: StdArc<Mutex<Vec<ClapParam>>>,
    pub param_index: StdArc<Mutex<HashMap<String, usize>>>,
    pub out_l: Vec<f32>,
    pub out_r: Vec<f32>,
}

impl ClapTrack {
    pub fn load(path: &str, sample_rate: f32) -> anyhow::Result<Self> {
        println!("[jamseq:clap] Loading CLAP plugin (headless DSP): {}", path);

        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let path_clone = path.to_string();

        let params_arc = StdArc::new(Mutex::new(Vec::new()));
        let param_index_arc = StdArc::new(Mutex::new(HashMap::new()));
        let params_init = params_arc.clone();
        let param_index_init = param_index_arc.clone();

        std::thread::Builder::new()
            .name("clap-host-thread".to_string())
            .spawn(move || {
                let entry = match unsafe { PluginEntry::load(&path_clone) } {
                    Ok(e) => Box::leak(Box::new(e)),
                    Err(e) => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("Load error: {}", e)));
                        return;
                    }
                };

                let factory = match entry.get_plugin_factory() {
                    Some(f) => f,
                    None => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("No plugin factory found")));
                        return;
                    }
                };

                let desc = match factory.plugin_descriptors().next() {
                    Some(d) => d,
                    None => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("No plugin descriptors found")));
                        return;
                    }
                };

                let plugin_id = match desc.id() {
                    Some(id) => id,
                    None => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("No plugin ID found")));
                        return;
                    }
                };

                let plugin_name = desc
                    .name()
                    .map(|c| c.to_string_lossy().to_string())
                    .unwrap_or_else(|| "CLAP Plugin".to_string());

                let host_info = match HostInfo::new("JamSeq", "JamBand", "https://codeberg.org/ess/jamband", "1.0.0") {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("HostInfo error: {}", e)));
                        return;
                    }
                };

                let timers = StdArc::new(Mutex::new(Vec::new()));
                let timers_clone = timers.clone();

                let mut instance = match PluginInstance::<JamSeqClapHost>::new(
                    |_| (),
                    |_| JamSeqClapMainThread { timers },
                    entry,
                    plugin_id,
                    &host_info,
                ) {
                    Ok(inst) => inst,
                    Err(e) => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("Instantiation error: {}", e)));
                        return;
                    }
                };

                // Query and introspect all tweakable parameters via PluginParams extension
                {
                    let handle = instance.plugin_handle();
                    if let Some(params_ext) = handle.get_extension::<PluginParams>() {
                        let count = params_ext.count(&handle);
                        let mut buf = ParamInfoBuffer::new();
                        let mut p_list = params_init.lock().unwrap();
                        let mut p_map = param_index_init.lock().unwrap();

                        for i in 0..count {
                            if let Some(info) = params_ext.get_info(&handle, i, &mut buf) {
                                let name = String::from_utf8_lossy(info.name).to_string();
                                let min = info.min_value as f32;
                                let max = info.max_value as f32;
                                let def = info.default_value as f32;
                                let val = params_ext.get_value(&handle, info.id).map(|v| v as f32).unwrap_or(def);

                                let idx = p_list.len();
                                p_map.insert(name.clone(), idx);
                                p_map.insert(name.to_lowercase(), idx);
                                p_map.insert(name.to_lowercase().replace(' ', "_"), idx);
                                p_map.insert(name.to_lowercase().replace('_', " "), idx);
                                let stripped_ms = name.to_lowercase().replace(" (ms)", "").replace("_ms", "").replace(" ms", "");
                                p_map.insert(stripped_ms.clone(), idx);
                                p_map.insert(format!("{}_ms", stripped_ms), idx);
                                p_map.insert(format!("{} (ms)", stripped_ms), idx);

                                p_list.push(ClapParam {
                                    id: info.id,
                                    name,
                                    min,
                                    max,
                                    default: def,
                                    value: val,
                                });
                            }
                        }
                        println!("[jamseq:clap] Introspected {} parameters for {}", p_list.len(), plugin_name);
                    }
                }

                let audio_config = PluginAudioConfiguration {
                    sample_rate: sample_rate as f64,
                    min_frames_count: 32,
                    max_frames_count: 4096,
                };

                let audio_processor = match instance.activate(|_, _| (), audio_config) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("Activation error: {}", e)));
                        return;
                    }
                };

                let started_processor = match audio_processor.start_processing() {
                    Ok(p) => p,
                    Err(_) => {
                        let _ = init_tx.send(Err(anyhow::anyhow!("Failed to start CLAP audio processor")));
                        return;
                    }
                };

                let _ = init_tx.send(Ok(started_processor));
                println!("[jamseq:clap] ✓ Started CLAP DSP processor for: {}", plugin_name);

                // Main thread timer pump (no GUI/X11 loop needed!)
                let timer_ext = instance.plugin_handle().get_extension::<PluginTimer>();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    if let Some(ref timer) = timer_ext {
                        let active_timers = timers_clone.lock().unwrap().clone();
                        for timer_id in active_timers {
                            timer.on_timer(&instance.plugin_handle(), timer_id);
                        }
                    }
                }
            })?;

        let started_processor = init_rx.recv()??;

        Ok(Self {
            path: path.to_string(),
            processor: StdArc::new(Mutex::new(started_processor)),
            pending_events: StdArc::new(Mutex::new(Vec::new())),
            pending_params: StdArc::new(Mutex::new(Vec::new())),
            params: params_arc,
            param_index: param_index_arc,
            out_l: vec![0.0f32; 1024],
            out_r: vec![0.0f32; 1024],
        })
    }

    pub fn note_on(&self, note: u8, velocity: f32) {
        self.pending_events.lock().unwrap().push((note, velocity, true));
    }

    pub fn note_off(&self, note: u8) {
        self.pending_events.lock().unwrap().push((note, 0.0, false));
    }

    pub fn all_notes_off(&self) {
        for note in 0..128 {
            self.note_off(note);
        }
    }

    pub fn get_parameters(&self) -> Vec<(String, f32, f32, f32)> {
        let params = self.params.lock().unwrap();
        params.iter().map(|p| (p.name.clone(), p.value, p.min, p.max)).collect()
    }

    pub fn set_parameter(&self, name: &str, value: f32) {
        let mut params = self.params.lock().unwrap();
        let index_map = self.param_index.lock().unwrap();
        let idx = index_map.get(name)
            .or_else(|| index_map.get(&name.to_lowercase()))
            .or_else(|| index_map.get(&name.to_lowercase().replace(' ', "_")))
            .or_else(|| index_map.get(&name.to_lowercase().replace('_', " ")));
        if let Some(&i) = idx {
            if let Some(param) = params.get_mut(i) {
                let clamped = if param.min >= 0.0 && param.max <= 1.0 && value > 1.0 {
                    // Plugin parameter expects 0.0..1.0 normalized value, but caller passed an unnormalized plain value
                    let n = name.to_lowercase();
                    if n.contains("cutoff") {
                        // 20..20000 Hz with skew -2.0 (log-like)
                        let hz = value.clamp(20.0, 20000.0);
                        ((hz / 20.0).ln() / (20000.0 / 20.0f32).ln()).clamp(0.0, 1.0)
                    } else if n.contains("glide") {
                        (value / 1000.0).clamp(0.0, 1.0)
                    } else if n.contains("attack") || n.contains("decay") || n.contains("release") {
                        (value / 5000.0).clamp(0.0, 1.0)
                    } else {
                        value.clamp(param.min, param.max)
                    }
                } else {
                    value.clamp(param.min, param.max)
                };

                param.value = clamped;
                let id = param.id;
                self.pending_params.lock().unwrap().push((id, clamped as f64));
            }
        }
    }

    pub fn process_block(&mut self, scratch_l: &mut [f32], scratch_r: &mut [f32]) {
        let num_samples = scratch_l.len().min(scratch_r.len());
        if self.out_l.len() < num_samples {
            self.out_l.resize(num_samples, 0.0);
            self.out_r.resize(num_samples, 0.0);
        }

        self.out_l[..num_samples].fill(0.0);
        self.out_r[..num_samples].fill(0.0);

        let mut input_event_buffer = EventBuffer::new();
        {
            let mut pending_notes = self.pending_events.lock().unwrap();
            for (note, vel, is_on) in pending_notes.drain(..) {
                if is_on {
                    let ev = NoteOnEvent::new(0, Pckn::new(0u16, 0u16, note as u16, 0u32), vel as f64);
                    input_event_buffer.push(&ev);
                } else {
                    let ev = NoteOffEvent::new(0, Pckn::new(0u16, 0u16, note as u16, 0u32), 0.0);
                    input_event_buffer.push(&ev);
                }
            }
        }
        {
            let mut pending_params = self.pending_params.lock().unwrap();
            for (id, val) in pending_params.drain(..) {
                let ev = ParamValueEvent::new(0, id, Pckn::match_all(), val);
                input_event_buffer.push(&ev);
            }
        }

        let input_events = input_event_buffer.as_input();
        let mut output_events_buf = EventBuffer::new();
        let mut output_events = output_events_buf.as_output();
        let input_audio = InputAudioBuffers::empty();

        let mut output_ports = AudioPorts::with_capacity(2, 1);
        let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only([
                &mut self.out_l[..num_samples],
                &mut self.out_r[..num_samples],
            ]),
        }]);

        if let Ok(mut proc) = self.processor.lock() {
            let _ = proc.process(
                &input_audio,
                &mut output_audio,
                &input_events,
                &mut output_events,
                None,
                None,
            );
        }

        for i in 0..num_samples {
            scratch_l[i] += self.out_l[i];
            scratch_r[i] += self.out_r[i];
        }
    }
}
