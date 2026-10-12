use clack_extensions::gui::{GuiApiType, GuiConfiguration, PluginGui, Window};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent};
use clack_host::host::HostError;
use clack_host::prelude::*;
use std::sync::{Arc as StdArc, Mutex};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

pub struct JamSeqClapMainThread {
    pub timers: StdArc<Mutex<Vec<TimerId>>>,
}

impl<'a> MainThreadHandler<'a> for JamSeqClapMainThread {}

impl HostTimerImpl for JamSeqClapMainThread {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        let mut timers = self.timers.lock().unwrap();
        let id = TimerId((timers.len() as u32) + 1);
        timers.push(id);
        println!("[jamseq:clap] Plugin registered timer #{} ({} ms)", id.0, period_ms);
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

pub struct ClapTrack {
    pub path: String,
    pub processor: StdArc<Mutex<StartedPluginAudioProcessor<JamSeqClapHost>>>,
    pub pending_events: StdArc<Mutex<Vec<(u8, f32, bool)>>>, // (note, vel, is_note_on)
    pub gui_open_tx: StdArc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    pub out_l: Vec<f32>,
    pub out_r: Vec<f32>,
}

impl ClapTrack {
    pub fn load(path: &str, sample_rate: f32) -> anyhow::Result<Self> {
        println!("[jamseq:clap] Spawning CLAP host thread for: {}", path);

        let (init_tx, init_rx) = std::sync::mpsc::channel();
        let (gui_tx, gui_rx) = std::sync::mpsc::channel::<()>();
        let gui_open_tx = StdArc::new(Mutex::new(Some(gui_tx)));

        let path_clone = path.to_string();

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

                let audio_config = PluginAudioConfiguration {
                    sample_rate: sample_rate as f64,
                    min_frames_count: 64,
                    max_frames_count: 1024,
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

                let window_title = format!("{} - Native CLAP GUI", plugin_name);
                let handle = instance.plugin_handle();
                let gui_ext = handle.get_extension::<PluginGui>();
                let timer_ext = handle.get_extension::<PluginTimer>();

                if gui_ext.is_none() {
                    println!("[jamseq:clap] Plugin does not support PluginGui");
                    return;
                }
                let gui = gui_ext.unwrap();

                while let Ok(()) = gui_rx.recv() {
                    println!("[jamseq:clap] Spawning native X11 window on display :0...");
                    if let Ok((conn, screen_num)) = x11rb::connect(None) {
                        let screen = &conn.setup().roots[screen_num];
                        let win_id = match conn.generate_id() {
                            Ok(id) => id,
                            Err(_) => continue,
                        };

                        let default_w = 800u16;
                        let default_h = 600u16;
                        let _ = conn.create_window(
                            x11rb::COPY_DEPTH_FROM_PARENT,
                            win_id,
                            screen.root,
                            100, 100,
                            default_w, default_h,
                            0,
                            WindowClass::INPUT_OUTPUT,
                            screen.root_visual,
                            &CreateWindowAux::new()
                                .background_pixel(screen.black_pixel)
                                .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
                        );

                        let _ = conn.change_property8(
                            PropMode::REPLACE,
                            win_id,
                            AtomEnum::WM_NAME,
                            AtomEnum::STRING,
                            window_title.as_bytes(),
                        );

                        let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS").ok().and_then(|r| r.reply().ok()).map(|r| r.atom).unwrap_or(0);
                        let wm_delete_window = conn.intern_atom(false, b"WM_DELETE_WINDOW").ok().and_then(|r| r.reply().ok()).map(|r| r.atom).unwrap_or(0);
                        if wm_protocols != 0 && wm_delete_window != 0 {
                            let _ = conn.change_property32(
                                PropMode::REPLACE,
                                win_id,
                                wm_protocols,
                                AtomEnum::ATOM,
                                &[wm_delete_window],
                            );
                        }

                        let _ = conn.map_window(win_id);
                        let _ = conn.flush();

                        let _ = gui.create(&handle, GuiConfiguration { api_type: GuiApiType::X11, is_floating: false });
                        if let Some(size) = gui.get_size(&handle) {
                            let _ = conn.configure_window(win_id, &ConfigureWindowAux::new().width(size.width).height(size.height));
                            let _ = conn.flush();
                        }
                        unsafe {
                            let _ = gui.set_parent(&handle, Window::from_x11_handle(win_id as _));
                        }
                        let _ = gui.show(&handle);

                        println!("[jamseq:clap] Native GUI is displayed and active.");

                        'event_loop: loop {
                            while let Ok(Some(event)) = conn.poll_for_event() {
                                match event {
                                    x11rb::protocol::Event::ClientMessage(msg) => {
                                        if msg.data.as_data32()[0] == wm_delete_window {
                                            println!("[jamseq:clap] User closed plugin window.");
                                            break 'event_loop;
                                        }
                                    }
                                    x11rb::protocol::Event::DestroyNotify(_) => break 'event_loop,
                                    _ => {}
                                }
                            }

                            if let Some(ref timer) = timer_ext {
                                let active_timers = timers_clone.lock().unwrap().clone();
                                for timer_id in active_timers {
                                    timer.on_timer(&handle, timer_id);
                                }
                            }

                            let _ = conn.flush();
                            std::thread::sleep(std::time::Duration::from_millis(16));
                        }

                        gui.destroy(&handle);
                        let _ = conn.destroy_window(win_id);
                        let _ = conn.flush();
                    }
                }
            })?;

        let started_processor = init_rx.recv()??;

        Ok(Self {
            path: path.to_string(),
            processor: StdArc::new(Mutex::new(started_processor)),
            pending_events: StdArc::new(Mutex::new(Vec::new())),
            gui_open_tx,
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

    pub fn open_gui(&self) {
        if let Some(tx) = self.gui_open_tx.lock().unwrap().as_ref() {
            let _ = tx.send(());
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
            let mut pending = self.pending_events.lock().unwrap();
            for (note, vel, is_on) in pending.drain(..) {
                if is_on {
                    let ev = NoteOnEvent::new(0, Pckn::new(0u16, 0u16, note as u16, 0u32), vel as f64);
                    input_event_buffer.push(&ev);
                } else {
                    let ev = NoteOffEvent::new(0, Pckn::new(0u16, 0u16, note as u16, 0u32), 0.0);
                    input_event_buffer.push(&ev);
                }
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
