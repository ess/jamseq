use crate::engine::JamSeqHost;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, Stream, StreamConfig};
use std::sync::{Arc, Mutex};

pub struct AudioOutput {
    _stream: Stream,
    _input_stream: Option<Stream>,
    pub host: Arc<Mutex<JamSeqHost>>,
}

impl AudioOutput {
    pub fn start(host: Arc<Mutex<JamSeqHost>>) -> Result<Self> {
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

        // Note: Do not unconditionally open default microphone stream on startup.
        // Holding an input stream open causes BlueZ / PipeWire to force Bluetooth earbuds
        // out of high-fidelity stereo A2DP mode into telephone mono headset (mSBC/SCO) mode,
        // which completely mutes or breaks audio playback on most Bluetooth earbuds.
        let input_stream: Option<Stream> = None;

        Ok(Self {
            _stream: stream,
            _input_stream: input_stream,
            host,
        })
    }
}
