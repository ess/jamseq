use crate::engine::JamSeqHost;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, Stream, StreamConfig};
use std::sync::{Arc, Mutex};

/// Global audio subsystem managing active cpal output and dynamic input streams.
#[allow(dead_code)]
pub struct AudioSystem {
    output_stream: Stream,
    input_stream: Option<Stream>,
    host: Arc<Mutex<JamSeqHost>>,
    sample_rate: u32,
    channels: usize,
}

impl AudioSystem {
    pub fn start(host: Arc<Mutex<JamSeqHost>>) -> Result<Arc<Mutex<Self>>> {
        let cpal_host = cpal::default_host();
        let device = cpal_host
            .default_output_device()
            .context("Failed to find default audio output device (PipeWire / ALSA)")?;

        let default_config = device
            .default_output_config()
            .context("Failed to get default output config")?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels() as usize;

        // Update host sample rate to match audio interface
        {
            let mut h = host.lock().unwrap();
            h.set_sample_rate(sample_rate as f32);
        }

        let config = StreamConfig {
            channels: channels as u16,
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let host_clone = Arc::clone(&host);

        let mut left_buf = vec![0.0f32; 1024];
        let mut right_buf = vec![0.0f32; 1024];

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let num_frames = data.len() / channels;

                if left_buf.len() < num_frames {
                    left_buf.resize(num_frames, 0.0);
                    right_buf.resize(num_frames, 0.0);
                }

                if let Ok(mut h) = host_clone.lock() {
                    h.process_block(&mut left_buf[..num_frames], &mut right_buf[..num_frames]);
                } else {
                    left_buf[..num_frames].fill(0.0);
                    right_buf[..num_frames].fill(0.0);
                }

                // Interleave into hardware output buffer
                for frame_idx in 0..num_frames {
                    let out_idx = frame_idx * channels;
                    data[out_idx] = left_buf[frame_idx];
                    if channels > 1 {
                        data[out_idx + 1] = right_buf[frame_idx];
                    }
                }
            },
            |err| {
                eprintln!("[jamseq] Audio output stream error: {:?}", err);
            },
            None,
        )?;

        stream.play().context("Failed to start audio output stream")?;

        let audio_system = Arc::new(Mutex::new(Self {
            output_stream: stream,
            input_stream: None,
            host,
            sample_rate,
            channels,
        }));

        Ok(audio_system)
    }

    /// Dynamically open the default microphone input stream on demand.
    /// This prevents Bluetooth earbuds from dropping into low-quality mono SCO mode
    /// during ordinary playback, and only activates microphone input when requested.
    pub fn start_input_capture(&mut self) -> Result<()> {
        if self.input_stream.is_some() {
            return Ok(()); // Already active
        }

        let cpal_host = cpal::default_host();
        let input_device = match cpal_host.default_input_device() {
            Some(dev) => dev,
            None => {
                eprintln!("[jamseq] Warning: No default input device found for recording");
                return Ok(());
            }
        };

        let input_config = match input_device.default_input_config() {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("[jamseq] Warning: Could not retrieve default input config: {:?}", e);
                return Ok(());
            }
        };

        let in_channels = input_config.channels() as usize;
        let in_sample_rate = input_config.sample_rate().0;
        let config = StreamConfig {
            channels: in_channels as u16,
            sample_rate: SampleRate(in_sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let host_clone = Arc::clone(&self.host);
        let in_stream = input_device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(h) = host_clone.lock() {
                    h.recorder.push_input_frames(data, in_channels);
                }
            },
            |err| {
                eprintln!("[jamseq] Audio input stream error: {:?}", err);
            },
            None,
        )?;

        in_stream.play().context("Failed to play input stream")?;
        self.input_stream = Some(in_stream);
        println!("[jamseq] ✓ Microphone input stream opened dynamically ({in_channels} ch @ {in_sample_rate} Hz)");
        Ok(())
    }

    /// Stop and drop the microphone input stream, returning Bluetooth headphones to A2DP stereo mode.
    pub fn stop_input_capture(&mut self) {
        if let Some(stream) = self.input_stream.take() {
            drop(stream);
            println!("[jamseq] ✓ Microphone input stream closed (Bluetooth A2DP preserved)");
        }
    }

    /// Check if recording/monitoring was requested, dynamically opening/closing input stream accordingly.
    pub fn poll_capture_state(&mut self) {
        let requested = if let Ok(h) = self.host.lock() {
            h.recorder.is_capture_requested()
        } else {
            false
        };

        if requested && self.input_stream.is_none() {
            let _ = self.start_input_capture();
        } else if !requested && self.input_stream.is_some() {
            self.stop_input_capture();
        }
    }
}

// Retain AudioOutput type alias for backward compatibility
pub type AudioOutput = AudioSystem;
