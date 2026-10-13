use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{PathBuf};

use crate::sampler::SampleBuffer;
use crate::timeline::TrackBlock;

/// Content-addressed stem chunk cache manager.
/// Caches rendered audio chunks per (track, section, block hash) on disk to avoid redundant DSP rendering.
#[derive(Debug, Clone)]
pub struct StemCacheManager {
    pub cache_dir: PathBuf,
    pub cached_chunks: HashMap<String, PathBuf>,
}

impl StemCacheManager {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = cache_dir.into();
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create stem cache directory at {:?}", dir))?;

        let mut mgr = Self {
            cache_dir: dir,
            cached_chunks: HashMap::new(),
        };
        mgr.rescan_disk()?;
        Ok(mgr)
    }

    /// Rescan existing cache files from disk
    pub fn rescan_disk(&mut self) -> Result<()> {
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        self.cached_chunks.insert(stem.to_string(), path);
                    }
                }
            }
        }
        Ok(())
    }

    /// Computes a deterministic content-addressed hash key for a track block within a section.
    pub fn compute_block_key(
        &self,
        track_id: &str,
        section_id: &str,
        block: &TrackBlock,
        dsp_patch: Option<&str>,
        effective_bpm: f64,
    ) -> String {
        let mut hasher = DefaultHasher::new();
        track_id.hash(&mut hasher);
        section_id.hash(&mut hasher);
        block.id.hash(&mut hasher);
        block.start_offset_ticks.hash(&mut hasher);
        block.base_length_ticks.hash(&mut hasher);
        match block.loop_mode {
            crate::timeline::LoopMode::OneShot => 1u8.hash(&mut hasher),
            crate::timeline::LoopMode::LoopToSection => 2u8.hash(&mut hasher),
            crate::timeline::LoopMode::FixedLoops(n) => (10u8 + n as u8).hash(&mut hasher),
        }
        if let Some(tempo) = block.tempo {
            match tempo {
                crate::timeline::TempoOverride::AbsoluteBpm(b) => b.to_bits().hash(&mut hasher),
                crate::timeline::TempoOverride::Ratio(n, d) => (n, d).hash(&mut hasher),
            }
        }
        // Hash serialized events
        if let Ok(events_json) = serde_json::to_string(&block.events) {
            events_json.hash(&mut hasher);
        }
        if let Some(ref audio) = block.audio_file {
            audio.hash(&mut hasher);
        }
        if let Some(patch) = dsp_patch {
            patch.hash(&mut hasher);
        }
        effective_bpm.to_bits().hash(&mut hasher);

        format!("{}_{}_{:016x}", section_id, track_id, hasher.finish())
    }

    /// Check if a cached chunk exists and is present on disk
    pub fn get_cached_chunk(&self, cache_key: &str) -> Option<PathBuf> {
        if let Some(path) = self.cached_chunks.get(cache_key) {
            if path.exists() {
                return Some(path.clone());
            }
        }
        let fallback = self.cache_dir.join(format!("{}.wav", cache_key));
        if fallback.exists() {
            Some(fallback)
        } else {
            None
        }
    }

    /// Load sample buffer for a cached key
    pub fn load_chunk(&self, cache_key: &str) -> Result<SampleBuffer> {
        let path = self.get_cached_chunk(cache_key)
            .ok_or_else(|| anyhow::anyhow!("Chunk '{}' not found in stem cache", cache_key))?;
        SampleBuffer::load_from_file(&path)
    }

    /// Write rendered audio block samples to a cached WAV file on disk
    pub fn store_chunk(
        &mut self,
        cache_key: &str,
        samples_left: &[f32],
        samples_right: &[f32],
        sample_rate: u32,
    ) -> Result<PathBuf> {
        let out_path = self.cache_dir.join(format!("{}.wav", cache_key));
        let spec = WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: 24,
            sample_format: SampleFormat::Int,
        };

        let mut writer = WavWriter::create(&out_path, spec)
            .with_context(|| format!("Failed to create stem cache WAV at {:?}", out_path))?;

        let num_samples = samples_left.len().min(samples_right.len());
        for i in 0..num_samples {
            let sample_i24_l = (samples_left[i] * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
            let sample_i24_r = (samples_right[i] * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
            writer.write_sample(sample_i24_l)?;
            writer.write_sample(sample_i24_r)?;
        }
        writer.finalize()?;

        self.cached_chunks.insert(cache_key.to_string(), out_path.clone());
        Ok(out_path)
    }

    /// Invalidate all cached entries for a specific track
    pub fn invalidate_track(&mut self, track_id: &str) {
        let needle = format!("_{}_", track_id);
        let matching_keys: Vec<String> = self.cached_chunks.keys()
            .filter(|k| k.contains(&needle) || k.ends_with(&format!("_{}", track_id)))
            .cloned()
            .collect();

        for key in matching_keys {
            if let Some(path) = self.cached_chunks.remove(&key) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Clear all cached chunks from disk and memory
    pub fn clear(&mut self) -> Result<()> {
        self.cached_chunks.clear();
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                    let _ = fs::remove_file(path);
                }
            }
        }
        Ok(())
    }
}
