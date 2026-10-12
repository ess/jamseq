use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct RecordedClipInfo {
    pub path: PathBuf,
    pub start_sample: u64,
    pub total_samples: u64,
    pub sample_rate: u32,
}

pub struct RecorderState {
    pub is_recording: bool,
    pub start_sample: u64,
    pub out_path: PathBuf,
    pub recorded_left: Vec<f32>,
    pub recorded_right: Vec<f32>,
    pub peak_input: f32, // Live mic input meter level (0.0 to 1.0)
}

#[derive(Clone)]
pub struct AudioRecorder {
    state: Arc<Mutex<RecorderState>>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecorderState {
                is_recording: false,
                start_sample: 0,
                out_path: PathBuf::new(),
                recorded_left: Vec::new(),
                recorded_right: Vec::new(),
                peak_input: 0.0,
            })),
        }
    }

    pub fn start_recording(&self, start_sample: u64, out_path: &Path) {
        if let Some(parent) = out_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut st = self.state.lock().unwrap();
        st.is_recording = true;
        st.start_sample = start_sample;
        st.out_path = out_path.to_path_buf();
        st.recorded_left.clear();
        st.recorded_right.clear();
        st.peak_input = 0.0;
    }

    pub fn is_recording(&self) -> bool {
        self.state.lock().unwrap().is_recording
    }

    pub fn get_peak_input(&self) -> f32 {
        self.state.lock().unwrap().peak_input
    }

    /// Process incoming microphone samples from cpal input stream
    pub fn push_input_frames(&self, data: &[f32], channels: usize) {
        let mut st = self.state.lock().unwrap();
        if channels == 0 || data.is_empty() {
            return;
        }

        let num_frames = data.len() / channels;
        let mut peak = 0.0f32;

        for frame_idx in 0..num_frames {
            let in_idx = frame_idx * channels;
            let l = data[in_idx];
            let r = if channels > 1 { data[in_idx + 1] } else { l };

            let abs_val = l.abs().max(r.abs());
            if abs_val > peak {
                peak = abs_val;
            }

            if st.is_recording {
                st.recorded_left.push(l);
                st.recorded_right.push(r);
            }
        }

        st.peak_input = peak;
    }

    /// Stop recording and finalize writing to 24-bit WAV file on disk
    pub fn stop_recording(&self, sample_rate: u32) -> Result<RecordedClipInfo> {
        let mut st = self.state.lock().unwrap();
        if !st.is_recording {
            anyhow::bail!("Recorder is not currently recording");
        }

        st.is_recording = false;
        let out_path = st.out_path.clone();
        let start_sample = st.start_sample;
        let total_samples = st.recorded_left.len() as u64;

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let spec = WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: 24,
            sample_format: SampleFormat::Int,
        };

        let mut writer = WavWriter::create(&out_path, spec)
            .with_context(|| format!("Failed to create WAV writer at {:?}", out_path))?;

        for i in 0..st.recorded_left.len() {
            let l = st.recorded_left[i];
            let r = st.recorded_right[i];
            let val_l = (l * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
            let val_r = (r * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
            writer.write_sample(val_l)?;
            writer.write_sample(val_r)?;
        }

        writer.finalize()?;

        Ok(RecordedClipInfo {
            path: out_path,
            start_sample,
            total_samples,
            sample_rate,
        })
    }
}
