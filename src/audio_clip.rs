use crate::sampler::SampleBuffer;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AudioClip {
    pub id: String,
    pub path: PathBuf,
    pub start_sample: u64,
    pub sample_offset: usize,
    pub duration_samples: Option<usize>,
    pub buffer: Option<SampleBuffer>,
}

impl AudioClip {
    pub fn new(path: &Path, start_sample: u64) -> Self {
        let buffer = SampleBuffer::load_from_file(path).ok();
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "clip".to_string());
        Self {
            id,
            path: path.to_path_buf(),
            start_sample,
            sample_offset: 0,
            duration_samples: None,
            buffer,
        }
    }

    pub fn with_id(id: &str, path: &Path, start_sample: u64) -> Self {
        let buffer = SampleBuffer::load_from_file(path).ok();
        Self {
            id: id.to_string(),
            path: path.to_path_buf(),
            start_sample,
            sample_offset: 0,
            duration_samples: None,
            buffer,
        }
    }

    pub fn from_buffer(id: &str, path: PathBuf, start_sample: u64, buffer: SampleBuffer) -> Self {
        Self {
            id: id.to_string(),
            path,
            start_sample,
            sample_offset: 0,
            duration_samples: None,
            buffer: Some(buffer),
        }
    }

    pub fn reload(&mut self) -> Result<()> {
        let buf = SampleBuffer::load_from_file(&self.path)?;
        self.buffer = Some(buf);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AudioClipTrack {
    pub clips: Vec<AudioClip>,
}

impl AudioClipTrack {
    pub fn new() -> Self {
        Self { clips: Vec::new() }
    }

    pub fn add_clip(&mut self, clip: AudioClip) {
        self.clips.push(clip);
    }

    /// Slice a clip into two non-destructive segments referencing the shared in-memory audio buffer
    pub fn slice_clip(&mut self, clip_id: &str, split_sample: u64) -> Option<(String, String)> {
        let pos = self
            .clips
            .iter()
            .position(|c| c.id == clip_id || c.path.to_string_lossy().contains(clip_id))?;
        let clip = &self.clips[pos];
        let buf_len = clip.buffer.as_ref()?.samples_left.len();
        let avail = buf_len.saturating_sub(clip.sample_offset);
        let clip_len = clip
            .duration_samples
            .map(|d| (d as u64).min(avail as u64))
            .unwrap_or(avail as u64);
        let clip_start = clip.start_sample;
        let clip_end = clip_start + clip_len;

        if split_sample <= clip_start || split_sample >= clip_end {
            return None;
        }

        let left_len = (split_sample - clip_start) as usize;
        let right_len = (clip_end - split_sample) as usize;
        let right_id = format!("{}_b", self.clips[pos].id);

        self.clips[pos].duration_samples = Some(left_len);

        let right_clip = AudioClip {
            id: right_id.clone(),
            path: self.clips[pos].path.clone(),
            start_sample: split_sample,
            sample_offset: self.clips[pos].sample_offset + left_len,
            duration_samples: Some(right_len),
            buffer: self.clips[pos].buffer.clone(),
        };

        let left_id = self.clips[pos].id.clone();
        self.clips.insert(pos + 1, right_clip);
        Some((left_id, right_id))
    }

    /// Non-destructive slip editing and trimming
    pub fn set_clip_offset(
        &mut self,
        clip_id: &str,
        start_sample: u64,
        duration_samples: usize,
    ) -> bool {
        let pos = match self
            .clips
            .iter()
            .position(|c| c.id == clip_id || c.path.to_string_lossy().contains(clip_id))
        {
            Some(p) => p,
            None => return false,
        };
        let clip = &mut self.clips[pos];
        if start_sample > clip.start_sample {
            let diff = (start_sample - clip.start_sample) as usize;
            clip.sample_offset += diff;
        } else if start_sample < clip.start_sample {
            let diff = (clip.start_sample - start_sample) as usize;
            clip.sample_offset = clip.sample_offset.saturating_sub(diff);
        }
        clip.start_sample = start_sample;
        clip.duration_samples = Some(duration_samples);
        true
    }

    /// Process audio block given the timeline's current start sample.
    pub fn process_block(
        &mut self,
        current_timeline_sample: u64,
        left_out: &mut [f32],
        right_out: &mut [f32],
    ) {
        let num_samples = left_out.len().min(right_out.len());
        let block_end = current_timeline_sample + num_samples as u64;

        for clip in &self.clips {
            let buf = match &clip.buffer {
                Some(b) => b,
                None => continue,
            };

            let avail = buf.samples_left.len().saturating_sub(clip.sample_offset);
            let clip_len = clip
                .duration_samples
                .map(|d| (d as u64).min(avail as u64))
                .unwrap_or(avail as u64);
            let clip_end = clip.start_sample + clip_len;

            // Check if this clip overlaps with the current block
            if block_end <= clip.start_sample || current_timeline_sample >= clip_end {
                continue;
            }

            // Calculate overlap window
            for i in 0..num_samples {
                let sample_pos = current_timeline_sample + i as u64;
                if sample_pos >= clip.start_sample && sample_pos < clip_end {
                    let offset = (sample_pos - clip.start_sample) as usize;
                    let buf_idx = clip.sample_offset + offset;
                    if buf_idx < buf.samples_left.len() {
                        left_out[i] += buf.samples_left[buf_idx];
                        right_out[i] += buf.samples_right[buf_idx];
                    }
                }
            }
        }
    }
}
