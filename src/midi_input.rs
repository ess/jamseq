//! Real-time Hardware USB / ALSA MIDI Input Integration for jamseq.

use anyhow::{Context, Result};
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMidiEvent {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiDeviceStatus {
    pub index: usize,
    pub name: String,
    pub is_connected: bool,
    pub is_enabled: bool,
}

pub struct MidiInputManager {
    sender: Sender<RawMidiEvent>,
    pub receiver: Receiver<RawMidiEvent>,
    active_connection: Option<(usize, MidiInputConnection<()>)>,
    active_device_name: Option<String>,
}

impl MidiInputManager {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            active_connection: None,
            active_device_name: None,
        }
    }

    /// Send a simulated or test MIDI event into the receiver queue
    pub fn send_event(&self, event: RawMidiEvent) {
        let _ = self.sender.send(event);
    }

    /// List all currently discovered hardware and virtual ALSA/PipeWire MIDI input ports.
    pub fn list_devices(&self) -> Vec<MidiDeviceStatus> {
        let Ok(midi_in) = MidiInput::new("jamseq-probe") else {
            return Vec::new();
        };

        let ports = midi_in.ports();
        let active_idx = self.active_connection.as_ref().map(|(idx, _)| *idx);

        ports
            .into_iter()
            .enumerate()
            .map(|(idx, port)| {
                let name = midi_in
                    .port_name(&port)
                    .unwrap_or_else(|_| format!("MIDI Port {}", idx));
                MidiDeviceStatus {
                    index: idx,
                    name,
                    is_connected: true,
                    is_enabled: active_idx == Some(idx),
                }
            })
            .collect()
    }

    /// Connect to a hardware MIDI port by index and stream events into the receiver channel.
    pub fn connect_device(&mut self, port_index: usize) -> Result<String> {
        self.disconnect();

        let midi_in = MidiInput::new("jamseq-midi-client")
            .context("Failed to initialize midir client")?;
        let ports = midi_in.ports();

        let port: MidiInputPort = ports
            .get(port_index)
            .cloned()
            .with_context(|| format!("MIDI port index {} out of range", port_index))?;

        let device_name = midi_in
            .port_name(&port)
            .unwrap_or_else(|_| format!("MIDI Port {}", port_index));

        let tx = self.sender.clone();
        let conn = midi_in
            .connect(
                &port,
                "jamseq-input-handler",
                move |_timestamp_us, message, _| {
                    if message.is_empty() {
                        return;
                    }
                    let status = message[0];
                    let data1 = if message.len() > 1 { message[1] } else { 0 };
                    let data2 = if message.len() > 2 { message[2] } else { 0 };
                    let _ = tx.send(RawMidiEvent { status, data1, data2 });
                },
                (),
            )
            .map_err(|e| anyhow::anyhow!("Failed to connect to MIDI device '{}': {}", device_name, e))?;

        println!("[jamseq] ✓ Connected to hardware MIDI controller: {}", device_name);
        self.active_device_name = Some(device_name.clone());
        self.active_connection = Some((port_index, conn));

        Ok(device_name)
    }

    /// Disconnect from the currently active MIDI input controller.
    pub fn disconnect(&mut self) {
        if let Some((_, conn)) = self.active_connection.take() {
            conn.close();
            println!("[jamseq] Disconnected MIDI controller");
        }
        self.active_device_name = None;
    }

    pub fn active_device_name(&self) -> Option<&str> {
        self.active_device_name.as_deref()
    }
}
