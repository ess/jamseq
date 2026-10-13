use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use vst3_host::audio::AudioBuffers;
use vst3_host::host::Vst3Host;
use vst3_host::midi::MidiChannel;
use vst3_host::plugin::Plugin;

#[derive(Clone, Debug)]
pub struct Vst3Param {
    pub id: u32,
    pub name: String,
    pub group: String,
    pub param_type: crate::ipc::ParamType,
    pub display_value: Option<String>,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
}

pub struct Vst3Track {
    pub path: String,
    pub plugin_name: String,
    plugin: Arc<Mutex<Plugin>>,
    buffers: Arc<Mutex<AudioBuffers>>,
    params: Arc<Mutex<Vec<Vst3Param>>>,
    param_index: Arc<Mutex<HashMap<String, usize>>>,
    pending_events: Arc<Mutex<Vec<(u8, f32, bool)>>>, // (note, velocity, is_note_on)
    sample_rate: f64,
}

impl Vst3Track {
    pub fn load(path: &str, sample_rate: f32) -> Result<Self> {
        let sr = sample_rate as f64;
        let mut host = Vst3Host::builder()
            .sample_rate(sr)
            .block_size(512)
            .with_process_isolation(false)
            .build()
            .context("Failed to build Vst3Host")?;

        let mut plugin = host
            .load_plugin(path)
            .with_context(|| format!("Failed to load VST3 plugin from: {}", path))?;

        let plugin_name = plugin.info().name.clone();

        plugin
            .start_processing()
            .with_context(|| format!("Failed to start processing for VST3 plugin: {}", plugin_name))?;

        let raw_params = plugin.get_parameters().unwrap_or_default();
        let mut params = Vec::new();
        let mut param_index = HashMap::new();

        for (i, p) in raw_params.into_iter().enumerate() {
            let (group, display_name) = if let Some(slash_idx) = p.name.rfind('/') {
                let g = p.name[..slash_idx].trim().to_string();
                let n = p.name[slash_idx + 1..].trim().to_string();
                (
                    if g.is_empty() { "General".to_string() } else { g.replace('/', " > ") },
                    if n.is_empty() { p.name.clone() } else { n },
                )
            } else if !p.unit.is_empty() && (p.name.contains(':') || p.name.contains(" - ")) {
                if let Some(colon_idx) = p.name.find(':') {
                    let g = p.name[..colon_idx].trim().to_string();
                    let n = p.name[colon_idx + 1..].trim().to_string();
                    (g, n)
                } else {
                    ("General".to_string(), p.name.clone())
                }
            } else {
                ("General".to_string(), p.name.clone())
            };

            let group = if group == "General" {
                let n = display_name.to_lowercase();
                if n.contains("cutoff") || n.contains("reso") || n.contains("drive") || n.contains("filter") {
                    "Filter".to_string()
                } else if n.contains("attack") || n.contains("decay") || n.contains("sustain") || n.contains("release") || n.contains("env") {
                    "Amp Envelope".to_string()
                } else if n.contains("glide") || n.contains("voice") || n.contains("portamento") || n.contains("tuning") {
                    "Voice".to_string()
                } else if n.contains("gain") || n.contains("volume") || n.contains("master") || n.contains("pan") {
                    "Output".to_string()
                } else {
                    "General".to_string()
                }
            } else {
                group
            };

            let is_bypass = p.is_bypass;
            let step_count = p.step_count;
            let param_type = if is_bypass || step_count == 1 {
                crate::ipc::ParamType::Bool
            } else if step_count > 1 && step_count <= 64 {
                let mut labels = Vec::new();
                for step in 0..=step_count {
                    let norm = step as f64 / step_count as f64;
                    let label = plugin.format_parameter(p.id, norm).unwrap_or_default().trim().to_string();
                    labels.push(label);
                }
                let has_named_labels = labels.iter().any(|l| !l.is_empty() && l.parse::<i64>().is_err());
                if has_named_labels {
                    for (step_i, l) in labels.iter_mut().enumerate() {
                        if l.is_empty() {
                            *l = format!("{}", step_i);
                        }
                    }
                    crate::ipc::ParamType::Enum { labels }
                } else {
                    crate::ipc::ParamType::Int
                }
            } else if step_count > 64 {
                crate::ipc::ParamType::Int
            } else {
                // step_count == 0: Plugins like Surge XT may report step_count = 0 for discrete mode/type parameters
                let n = p.name.to_lowercase();
                let is_likely_enum = n.contains("type") || n.contains("mode") || n.contains("model") || n.contains("waveform") || n.contains("algorithm") || n.contains("shape");
                let mut probed_enum = None;
                if is_likely_enum {
                    let num_probes = 48;
                    let mut probe_labels = Vec::new();
                    for step in 0..=num_probes {
                        let norm = step as f64 / num_probes as f64;
                        if let Ok(label) = plugin.format_parameter(p.id, norm) {
                            let trimmed = label.trim().to_string();
                            if !trimmed.is_empty() && !probe_labels.contains(&trimmed) {
                                probe_labels.push(trimmed);
                            }
                        }
                    }
                    let has_named = probe_labels.iter().any(|l| l.parse::<f64>().is_err());
                    if probe_labels.len() >= 2 && probe_labels.len() <= 64 && has_named {
                        probed_enum = Some(crate::ipc::ParamType::Enum { labels: probe_labels });
                    }
                }

                probed_enum.unwrap_or(crate::ipc::ParamType::Float)
            };

            let display_value = plugin.format_parameter(p.id, p.value).ok().and_then(|s| {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() { Some(trimmed) } else { None }
            });

            let param = Vst3Param {
                id: p.id,
                name: display_name,
                group,
                param_type,
                display_value,
                min: p.min as f32,
                max: p.max as f32,
                default: p.default as f32,
                value: p.value as f32,
            };
            param_index.insert(param.name.clone(), i);
            param_index.insert(param.name.to_lowercase(), i);
            param_index.insert(param.name.to_lowercase().replace(' ', "_"), i);
            param_index.insert(param.name.to_lowercase().replace('_', " "), i);
            if p.name != param.name {
                param_index.insert(p.name.clone(), i);
                param_index.insert(p.name.to_lowercase(), i);
            }
            params.push(param);
        }

        println!(
            "[jamseq:vst3] Loaded VST3 plugin: {} ({} parameters) from {}",
            plugin_name,
            params.len(),
            path
        );

        let buffers = AudioBuffers::new(2, 2, 512, sr);

        Ok(Self {
            path: path.to_string(),
            plugin_name,
            plugin: Arc::new(Mutex::new(plugin)),
            buffers: Arc::new(Mutex::new(buffers)),
            params: Arc::new(Mutex::new(params)),
            param_index: Arc::new(Mutex::new(param_index)),
            pending_events: Arc::new(Mutex::new(Vec::new())),
            sample_rate: sr,
        })
    }

    pub fn name(&self) -> &str {
        &self.plugin_name
    }

    pub fn note_on(&self, note: u8, velocity: f32) {
        self.pending_events.lock().unwrap().push((note, velocity, true));
    }

    pub fn note_off(&self, note: u8) {
        self.pending_events.lock().unwrap().push((note, 0.0, false));
    }

    pub fn all_notes_off(&self) {
        if let Ok(mut plugin) = self.plugin.lock() {
            let _ = plugin.midi_panic();
        }
        for note in 0..128 {
            self.pending_events.lock().unwrap().push((note, 0.0, false));
        }
    }

    pub fn handle_cc(&self, cc: u8, val: u8) {
        if let Ok(mut plugin) = self.plugin.lock() {
            let _ = plugin.send_midi_cc(cc, val, MidiChannel::Ch1);
        }
    }

    pub fn get_parameters(&self) -> Vec<(String, f32, f32, f32)> {
        let params = self.params.lock().unwrap();
        params
            .iter()
            .map(|p| (p.name.clone(), p.value, p.min, p.max))
            .collect()
    }

    pub fn get_param_statuses(&self) -> Vec<crate::ipc::TrackParamStatus> {
        let params = self.params.lock().unwrap();
        params
            .iter()
            .map(|p| crate::ipc::TrackParamStatus {
                name: p.name.clone(),
                value: p.value,
                min: p.min,
                max: p.max,
                group: p.group.clone(),
                param_type: p.param_type.clone(),
                display_value: p.display_value.clone(),
            })
            .collect()
    }

    pub fn set_parameter(&self, name: &str, value: f32) {
        let mut params = self.params.lock().unwrap();
        let index_map = self.param_index.lock().unwrap();
        let idx = index_map
            .get(name)
            .or_else(|| index_map.get(&name.to_lowercase()))
            .or_else(|| index_map.get(&name.to_lowercase().replace(' ', "_")))
            .or_else(|| index_map.get(&name.to_lowercase().replace('_', " ")));

        if let Some(&i) = idx {
            if let Some(param) = params.get_mut(i) {
                let clamped = value.clamp(param.min, param.max);
                param.value = clamped;
                let id = param.id;
                if let Ok(mut plugin) = self.plugin.lock() {
                    let _ = plugin.set_parameter(id, clamped as f64);
                    if let Ok(formatted) = plugin.format_parameter(id, clamped as f64) {
                        let trimmed = formatted.trim().to_string();
                        if !trimmed.is_empty() {
                            param.display_value = Some(trimmed);
                        }
                    }
                }
            }
        }
    }

    pub fn process_block(&mut self, scratch_l: &mut [f32], scratch_r: &mut [f32]) {
        let num_samples = scratch_l.len().min(scratch_r.len());
        let mut buffers = self.buffers.lock().unwrap();
        if buffers.block_size < num_samples {
            *buffers = AudioBuffers::new(2, 2, num_samples, self.sample_rate);
        }

        buffers.clear();

        if let Ok(mut plugin) = self.plugin.lock() {
            let mut pending = self.pending_events.lock().unwrap();
            for (note, vel, is_on) in pending.drain(..) {
                if is_on {
                    let v_byte = (vel * 127.0).clamp(1.0, 127.0) as u8;
                    let _ = plugin.send_midi_note(note, v_byte, MidiChannel::Ch1);
                } else {
                    let _ = plugin.send_midi_note_off(note, MidiChannel::Ch1);
                }
            }

            let _ = plugin.process_audio(&mut buffers);
        }

        if buffers.outputs.len() >= 2 {
            for i in 0..num_samples {
                scratch_l[i] += buffers.outputs[0][i];
                scratch_r[i] += buffers.outputs[1][i];
            }
        } else if buffers.outputs.len() == 1 {
            for i in 0..num_samples {
                scratch_l[i] += buffers.outputs[0][i];
                scratch_r[i] += buffers.outputs[0][i];
            }
        }
    }

    pub fn process_effect_block(&mut self, buf_l: &mut [f32], buf_r: &mut [f32]) {
        let num_samples = buf_l.len().min(buf_r.len());
        let mut buffers = self.buffers.lock().unwrap();
        if buffers.block_size < num_samples {
            *buffers = AudioBuffers::new(2, 2, num_samples, self.sample_rate);
        }

        buffers.clear();
        if buffers.inputs.len() >= 2 {
            buffers.inputs[0][..num_samples].copy_from_slice(&buf_l[..num_samples]);
            buffers.inputs[1][..num_samples].copy_from_slice(&buf_r[..num_samples]);
        }

        if let Ok(mut plugin) = self.plugin.lock() {
            let _ = plugin.process_audio(&mut buffers);
        }

        if buffers.outputs.len() >= 2 {
            buf_l[..num_samples].copy_from_slice(&buffers.outputs[0][..num_samples]);
            buf_r[..num_samples].copy_from_slice(&buffers.outputs[1][..num_samples]);
        } else if buffers.outputs.len() == 1 {
            buf_l[..num_samples].copy_from_slice(&buffers.outputs[0][..num_samples]);
            buf_r[..num_samples].copy_from_slice(&buffers.outputs[0][..num_samples]);
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate as f64;
        if let Ok(mut plugin) = self.plugin.lock() {
            let _ = plugin.reconfigure(sample_rate as f64, 512);
        }
    }
}
