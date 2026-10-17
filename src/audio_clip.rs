use crate::sampler::SampleBuffer;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AudioClip {
    pub path: PathBuf,
    pub start_sample: u64,
    pub buffer: Option<SampleBuffer>,
}

impl AudioClip {
    pub fn new(path: &Path, start_sample: u64) -> Self {
        let buffer = SampleBuffer::load_from_file(path).ok();
        Self {
            path: path.to_path_buf(),
            start_sample,
            buffer,
        }
    }

    pub fn from_buffer(path: PathBuf, start_sample: u64, buffer: SampleBuffer) -> Self {
        Self {
            path,
            start_sample,
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

            let clip_len = buf.samples_left.len() as u64;
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
                    if offset < buf.samples_left.len() {
                        left_out[i] += buf.samples_left[offset];
                        right_out[i] += buf.samples_right[offset];
                    }
                }
            }
        }
    }
}
