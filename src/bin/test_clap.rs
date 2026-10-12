use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::events::event_types::NoteOnEvent;
use clack_host::host::HostError;
use clack_host::prelude::*;
use std::sync::{Arc as StdArc, Mutex};

struct MyHostMainThread {
    timers: StdArc<Mutex<Vec<TimerId>>>,
}

impl<'a> MainThreadHandler<'a> for MyHostMainThread {}

impl HostTimerImpl for MyHostMainThread {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, HostError> {
        println!("  -> Plugin registered timer with period {} ms!", period_ms);
        let id = TimerId(self.timers.lock().unwrap().len() as u32 + 1);
        self.timers.lock().unwrap().push(id);
        Ok(id)
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        self.timers.lock().unwrap().retain(|&id| id != timer_id);
        Ok(())
    }
}

struct MyHost;
impl HostHandlers for MyHost {
    type Shared<'a> = ();
    type MainThread<'a> = MyHostMainThread;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<'_, Self>, _shared: &()) {
        builder.register::<HostTimer>();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("[test_clap] Loading Surge XT from /usr/lib/clap/Surge XT.clap...");
    let entry = unsafe { PluginEntry::load("/usr/lib/clap/Surge XT.clap")? };
    let factory = entry.get_plugin_factory().expect("No plugin factory found");
    let desc = factory
        .plugin_descriptors()
        .next()
        .expect("No descriptors found");

    println!("  -> Found: {:?}", desc.name().unwrap());

    let host_info = HostInfo::new("JamSeq", "JamBand", "https://codeberg.org/ess/jamband", "1.0.0")?;

    let timers = StdArc::new(Mutex::new(Vec::new()));
    let timers_clone = timers.clone();

    let mut plugin_instance = PluginInstance::<MyHost>::new(
        |_| (),
        |_| MyHostMainThread { timers },
        &entry,
        desc.id().unwrap(),
        &host_info,
    )?;

    println!("  -> Plugin instantiated successfully!");

    let audio_config = PluginAudioConfiguration {
        sample_rate: 48000.0,
        min_frames_count: 64,
        max_frames_count: 512,
    };

    let audio_processor = plugin_instance.activate(|_, _| (), audio_config)?;
    let mut started_processor = audio_processor.start_processing()?;

    println!("  -> Plugin activated and processing started!");

    use clack_extensions::gui::{PluginGui, GuiApiType, GuiConfiguration, Window};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::wrapper::ConnectionExt as _;

    let mut x11_info = None;
    let mut gui_opt = None;

    let handle = plugin_instance.plugin_handle();
    if let Some(_t) = handle.get_extension::<clack_extensions::timer::PluginTimer>() {
        println!("  -> Plugin implements PluginTimer!");
    } else {
        println!("  -> Plugin does NOT implement PluginTimer");
    }
    if let Some(_p) = handle.get_extension::<clack_extensions::posix_fd::PluginPosixFd>() {
        println!("  -> Plugin implements PluginPosixFd!");
    } else {
        println!("  -> Plugin does NOT implement PluginPosixFd");
    }
    if let Some(gui) = handle.get_extension::<PluginGui>() {
        println!("  -> Found PluginGui extension!");
        let preferred = gui.get_preferred_api(&handle);
        println!("  -> Preferred API: {:?}", preferred);

        if let Ok((conn, screen_num)) = x11rb::connect(None) {
            println!("  -> Connected to X11 on display!");
            let screen = &conn.setup().roots[screen_num];
            let win_id = conn.generate_id().unwrap();
            let width = 800u16;
            let height = 600u16;
            conn.create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                win_id,
                screen.root,
                50, 50,
                width, height,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .background_pixel(screen.black_pixel)
                    .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
            ).unwrap();
            conn.change_property8(
                PropMode::REPLACE,
                win_id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"Surge XT - Native CLAP GUI",
            ).unwrap();

            let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS").unwrap().reply().unwrap().atom;
            let wm_delete_window = conn.intern_atom(false, b"WM_DELETE_WINDOW").unwrap().reply().unwrap().atom;
            conn.change_property32(
                PropMode::REPLACE,
                win_id,
                wm_protocols,
                AtomEnum::ATOM,
                &[wm_delete_window],
            ).unwrap();

            conn.map_window(win_id).unwrap();
            conn.flush().unwrap();
            println!("  -> Created X11 window ID: {}", win_id);

            gui.create(&handle, GuiConfiguration { api_type: GuiApiType::X11, is_floating: false }).expect("Failed gui.create");
            if let Some(size) = gui.get_size(&handle) {
                println!("  -> Plugin reported natural GUI size: {}x{}", size.width, size.height);
                conn.configure_window(win_id, &ConfigureWindowAux::new().width(size.width).height(size.height)).unwrap();
                conn.flush().unwrap();
            }
            unsafe {
                gui.set_parent(&handle, Window::from_x11_handle(win_id as _)).expect("Failed gui.set_parent");
            }
            gui.show(&handle).expect("Failed gui.show");
            println!("  -> Native Surge XT GUI is now SHOWN in X11 window!");

            x11_info = Some((conn, win_id, wm_delete_window));
            gui_opt = Some(gui);
        }
    } else {
        println!("  -> No PluginGui extension found on this plugin");
    }

    // Set up buffers
    let mut out_l = vec![0.0f32; 512];
    let mut out_r = vec![0.0f32; 512];

    let mut output_ports = AudioPorts::with_capacity(2, 1);

    let note_on = NoteOnEvent::new(0, Pckn::new(0u16, 0u16, 50u16, 0u32), 1.0); // D3 (key 50)
    let input_events_buf = [note_on];
    let input_events = InputEvents::from_buffer(&input_events_buf);
    let mut output_events_buf = EventBuffer::new();
    let mut output_events = OutputEvents::from_buffer(&mut output_events_buf);

    let input_audio = InputAudioBuffers::empty();
    let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_output_only([out_l.as_mut_slice(), out_r.as_mut_slice()]),
    }]);

    started_processor.process(
        &input_audio,
        &mut output_audio,
        &input_events,
        &mut output_events,
        Some(0),
        None,
    )?;

    let max_l: f32 = out_l.iter().map(|s| s.abs()).fold(0.0, f32::max);
    let max_r: f32 = out_r.iter().map(|s| s.abs()).fold(0.0, f32::max);

    println!("  -> Processed 512 frames! Max left: {:.4}, Max right: {:.4}", max_l, max_r);
    assert!(max_l > 0.0001 || max_r > 0.0001, "Expected sound output from Surge XT!");

    println!("✓ Surge XT is running natively inside JamSeq with clack-host!");
    println!("\n=======================================================");
    println!("🎛 Surge XT Native Window is LIVE on your desktop!");
    println!("Feel free to click around, tweak wavetables, or test controls.");
    println!("Close the window (or press Ctrl+C in this terminal) to exit.");
    println!("=======================================================\n");

    let timer_ext = handle.get_extension::<PluginTimer>();

    // Keep X11 window alive and responsive with timer pulsing
    if let Some((conn, _win_id, wm_delete)) = x11_info {
        'event_loop: loop {
            while let Ok(Some(event)) = conn.poll_for_event() {
                match event {
                    x11rb::protocol::Event::ClientMessage(msg) => {
                        if msg.data.as_data32()[0] == wm_delete {
                            println!("\nClosing window...");
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
    }

    if let Some(gui) = gui_opt {
        gui.destroy(&handle);
    }

    Ok(())
}
