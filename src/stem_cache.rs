use anyhow::Result;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::sampler::SampleBuffer;
use crate::timeline::TrackBlock;

/// Content-addressed in-memory stem chunk cache manager.
/// Caches rendered audio chunks per (track, section, block hash) in RAM to avoid redundant DSP rendering
/// without performing any disk I/O during normal playback.
#[derive(Debug, Clone, Default)]
pub struct StemCacheManager {
    pub cached_chunks: HashMap<String, SampleBuffer>,
}

impl StemCacheManager {
    pub fn new() -> Self {
        Self {
            cached_chunks: HashMap::new(),
        }
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

    /// Computes a deterministic content-addressed hash key for an ad-hoc timeline block.
    pub fn compute_timeline_block_key(
        &self,
        track_id: &str,
        block: &TrackBlock,
        dsp_patch: Option<&str>,
        effective_bpm: f64,
    ) -> String {
        let mut hasher = DefaultHasher::new();
        track_id.hash(&mut hasher);
        "timeline".hash(&mut hasher);
        block.id.hash(&mut hasher);
        block.start_offset_ticks.hash(&mut hasher);
        block.base_length_ticks.hash(&mut hasher);
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

        format!("timeline_{}_{}_{:016x}", track_id, block.id, hasher.finish())
    }

    /// Check if a cached chunk exists in memory
    pub fn get_cached_chunk(&self, cache_key: &str) -> Option<&SampleBuffer> {
        self.cached_chunks.get(cache_key)
    }

    /// Load sample buffer for a cached key
    pub fn load_chunk(&self, cache_key: &str) -> Result<SampleBuffer> {
        self.cached_chunks
            .get(cache_key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Chunk '{}' not found in stem cache", cache_key))
    }

    /// Store rendered audio block samples into in-memory cache
    pub fn store_chunk(
        &mut self,
        cache_key: &str,
        samples_left: &[f32],
        samples_right: &[f32],
        sample_rate: u32,
    ) -> Result<()> {
        let buf = SampleBuffer {
            channels: 2,
            sample_rate,
            samples_left: samples_left.to_vec(),
            samples_right: samples_right.to_vec(),
        };
        self.cached_chunks.insert(cache_key.to_string(), buf);
        Ok(())
    }

    /// Invalidate all cached entries for a specific track
    pub fn invalidate_track(&mut self, track_id: &str) {
        let needle = format!("_{}_", track_id);
        self.cached_chunks.retain(|k, _| {
            !(k.contains(&needle) || k.ends_with(&format!("_{}", track_id)))
        });
    }

    /// Clear all cached chunks from memory
    pub fn clear(&mut self) -> Result<()> {
        self.cached_chunks.clear();
        Ok(())
    }
}
