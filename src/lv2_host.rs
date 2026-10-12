use lilv::World;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

#[repr(C)]
struct Lv2Feature {
    uri: *const c_char,
    data: *mut c_void,
}

#[repr(C)]
struct Lv2UridMap {
    handle: *mut c_void,
    map: unsafe extern "C" fn(handle: *mut c_void, uri: *const c_char) -> u32,
}

#[repr(C)]
struct Lv2UiResize {
    handle: *mut c_void,
    ui_resize: unsafe extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32,
}

#[repr(C)]
struct Lv2UiIdleInterface {
    idle: unsafe extern "C" fn(ui: *mut c_void) -> i32,
}

#[repr(C)]
struct Lv2UiDescriptor {
    uri: *const c_char,
    instantiate: unsafe extern "C" fn(
        descriptor: *const Lv2UiDescriptor,
        plugin_uri: *const c_char,
        bundle_path: *const c_char,
        write_function: Option<
            unsafe extern "C" fn(
                controller: *mut c_void,
                port_index: u32,
                buffer_size: u32,
                port_protocol: u32,
                buffer: *const c_void,
            ),
        >,
        controller: *mut c_void,
        widget: *mut *mut c_void,
        features: *const *const Lv2Feature,
    ) -> *mut c_void,
    cleanup: unsafe extern "C" fn(ui: *mut c_void),
    port_event: Option<
        unsafe extern "C" fn(
            ui: *mut c_void,
            port_index: u32,
            buffer_size: u32,
            format: u32,
            buffer: *const c_void,
        ),
    >,
    extension_data: Option<unsafe extern "C" fn(uri: *const c_char) -> *const c_void>,
}

type Lv2UiDescriptorFn = unsafe extern "C" fn(index: u32) -> *const Lv2UiDescriptor;

struct UridState {
    uris: Vec<String>,
    map: HashMap<String, u32>,
}

unsafe extern "C" fn urid_map_fn(handle: *mut c_void, uri_ptr: *const c_char) -> u32 {
    let state_arc = unsafe { &*(handle as *const Mutex<UridState>) };
    let uri_str = unsafe { CStr::from_ptr(uri_ptr) }.to_string_lossy().to_string();
    let mut state = state_arc.lock().unwrap();
    if let Some(&id) = state.map.get(&uri_str) {
        return id;
    }
    let id = (state.uris.len() as u32) + 1;
    state.uris.push(uri_str.clone());
    state.map.insert(uri_str, id);
    id
}

struct ResizeState {
    conn: Arc<x11rb::rust_connection::RustConnection>,
    win_id: u32,
}

unsafe extern "C" fn host_resize_fn(handle: *mut c_void, width: i32, height: i32) -> i32 {
    let state = unsafe { &*(handle as *const ResizeState) };
    let _ = state.conn.configure_window(
        state.win_id,
        &ConfigureWindowAux::new()
            .width(width as u32)
            .height(height as u32),
    );
    let _ = state.conn.flush();
    0
}

pub struct UiController {
    pub pending_midi: Arc<Mutex<Vec<[u8; 3]>>>,
}

unsafe extern "C" fn ui_write_fn(
    controller: *mut c_void,
    _port_index: u32,
    buffer_size: u32,
    _port_protocol: u32,
    buffer: *const c_void,
) {
    if controller.is_null() || buffer.is_null() || buffer_size == 0 {
        return;
    }
    let ctrl = unsafe { &*(controller as *const UiController) };
    let slice = unsafe { std::slice::from_raw_parts(buffer as *const u8, buffer_size as usize) };

    if buffer_size == 3 && (slice[0] & 0x80) != 0 {
        ctrl.pending_midi.lock().unwrap().push([slice[0], slice[1], slice[2]]);
        return;
    }

    for i in 0..slice.len().saturating_sub(2) {
        let status = slice[i];
        if (0x80..=0xEF).contains(&status) {
            let b1 = slice[i + 1];
            let b2 = slice[i + 2];
            if b1 < 128 && b2 < 128 {
                ctrl.pending_midi.lock().unwrap().push([status, b1, b2]);
                break;
            }
        }
    }
}

#[repr(C, align(8))]
struct AtomSequenceBuffer {
    // LV2_Atom
    atom_size: u32,
    atom_type: u32,
    // LV2_Atom_Sequence_Body
    unit: u32,
    pad: u32,
    // Storage for events
    data: [u8; 1024],
}

pub struct Lv2Track {
    pub uri: String,
    pub instance: Arc<Mutex<lilv::instance::ActiveInstance>>,
    pub midi_port: Option<usize>,
    pub notify_port: Option<usize>,
    pub out_l_port: usize,
    pub out_r_port: usize,
    pub pending_midi: Arc<Mutex<Vec<[u8; 3]>>>,
    pub gui_open_tx: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    pub out_l: Vec<f32>,
    pub out_r: Vec<f32>,
    atom_buf: Box<AtomSequenceBuffer>,
    notify_buf: Box<AtomSequenceBuffer>,
    seq_urid: u32,
    midi_urid: u32,
}

impl Lv2Track {
    pub fn load(plugin_uri: &str, sample_rate: f32) -> anyhow::Result<Self> {
        println!("[jamseq:lv2] Loading LV2 plugin: {}", plugin_uri);

        let world = Box::leak(Box::new(World::with_load_all()));
        let uri_node = world.new_uri(plugin_uri);
        let plugin = world
            .plugins()
            .into_iter()
            .find(|p| p.uri() == uri_node)
            .ok_or_else(|| anyhow::anyhow!("Plugin URI not found: {}", plugin_uri))?;

        let plugin_name = plugin.name().as_str().unwrap_or("LV2 Plugin").to_string();

        let urid_state = Box::leak(Box::new(Mutex::new(UridState {
            uris: Vec::new(),
            map: HashMap::new(),
        })));
        let urid_map_struct = Box::leak(Box::new(Lv2UridMap {
            handle: (urid_state as *const Mutex<UridState>) as *mut c_void,
            map: urid_map_fn,
        }));

#[repr(C)]
struct Lv2WorkerSchedule {
    handle: *mut c_void,
    schedule_work: unsafe extern "C" fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Lv2WorkerInterface {
    work: unsafe extern "C" fn(
        instance: *mut c_void,
        respond: unsafe extern "C" fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32,
        handle: *mut c_void,
        size: u32,
        data: *const c_void,
    ) -> i32,
    work_response: unsafe extern "C" fn(instance: *mut c_void, size: u32, body: *const c_void) -> i32,
    end_run: Option<unsafe extern "C" fn(instance: *mut c_void) -> i32>,
}

struct WorkerState {
    instance_ptr: *mut c_void,
    iface: Option<Lv2WorkerInterface>,
}

unsafe extern "C" fn worker_respond_fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32 {
    let state = unsafe { &*(handle as *const Mutex<WorkerState>) };
    let guard = state.lock().unwrap();
    if let Some(iface) = guard.iface {
        if !guard.instance_ptr.is_null() {
            unsafe {
                let _ = (iface.work_response)(guard.instance_ptr, size, data);
                if let Some(end_run) = iface.end_run {
                    let _ = end_run(guard.instance_ptr);
                }
            }
        }
    }
    0
}

unsafe extern "C" fn worker_schedule_fn(handle: *mut c_void, size: u32, data: *const c_void) -> i32 {
    let state = unsafe { &*(handle as *const Mutex<WorkerState>) };
    let (inst, iface) = {
        let guard = state.lock().unwrap();
        (guard.instance_ptr, guard.iface)
    };
    if let Some(iface) = iface {
        if !inst.is_null() {
            unsafe {
                let _ = (iface.work)(inst, worker_respond_fn, handle, size, data);
            }
        }
    }
    0
}

        let worker_state = Box::leak(Box::new(Mutex::new(WorkerState {
            instance_ptr: std::ptr::null_mut(),
            iface: None,
        })));
        let worker_sched_struct = Box::leak(Box::new(Lv2WorkerSchedule {
            handle: (worker_state as *const Mutex<WorkerState>) as *mut c_void,
            schedule_work: worker_schedule_fn,
        }));

        let feat_urid_map = lv2_raw::LV2Feature {
            uri: CString::new("http://lv2plug.in/ns/ext/urid#map")?.into_raw(),
            data: (urid_map_struct as *const Lv2UridMap) as *mut c_void,
        };
        let feat_worker = lv2_raw::LV2Feature {
            uri: CString::new("http://lv2plug.in/ns/ext/worker#schedule")?.into_raw(),
            data: (worker_sched_struct as *const Lv2WorkerSchedule) as *mut c_void,
        };
        let features = [feat_urid_map, feat_worker];

        let instance = unsafe {
            plugin
                .instantiate(sample_rate as f64, &features)
                .ok_or_else(|| anyhow::anyhow!("Failed to instantiate LV2 plugin: {}", plugin_name))?
        };

        // Wire instance pointer and worker interface for background loading
        if let Some(iface_ptr) = unsafe { instance.extension_data::<Lv2WorkerInterface>("http://lv2plug.in/ns/ext/worker#interface") } {
            let iface = unsafe { *iface_ptr.as_ptr() };
            let raw_handle = instance.handle();
            let mut guard = worker_state.lock().unwrap();
            guard.instance_ptr = raw_handle;
            guard.iface = Some(iface);
        }

        // Determine audio and MIDI ports
        let num_ports = plugin.ports_count();
        let mut midi_port = None;
        let mut notify_port = None;
        let mut audio_outs = Vec::new();

        for i in 0..num_ports {
            if let Some(port) = plugin.port_by_index(i) {
                let sym_node = port.symbol();
                let symbol = sym_node
                    .as_ref()
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let _classes: Vec<String> = port.classes().into_iter().map(|c| c.as_str().unwrap_or("?").to_string()).collect();
                if symbol == "control" || symbol.contains("midi") || symbol.contains("in") {
                    if midi_port.is_none() {
                        midi_port = Some(i);
                    }
                } else if symbol == "notify" {
                    notify_port = Some(i);
                } else if symbol.contains("out") || symbol == "left" || symbol == "right" {
                    audio_outs.push(i);
                }
            }
        }

        let out_l_port = if audio_outs.len() >= 2 { audio_outs[0] } else { 2.min(num_ports - 1) };
        let out_r_port = if audio_outs.len() >= 2 { audio_outs[1] } else { 3.min(num_ports - 1) };

        println!(
            "[jamseq:lv2] Configured ports: MIDI In: {:?}, Notify: {:?}, Out L: {}, Out R: {}",
            midi_port, notify_port, out_l_port, out_r_port
        );

        let active_instance = unsafe { instance.activate() };
        let active_arc = Arc::new(Mutex::new(active_instance));

        // Get URIDs for Atom Sequence and MIDI event
        let seq_urid = {
            let c = CString::new("http://lv2plug.in/ns/ext/atom#Sequence")?;
            unsafe { urid_map_fn((&*urid_state as *const Mutex<UridState>) as *mut c_void, c.as_ptr()) }
        };
        let midi_urid = {
            let c = CString::new("http://lv2plug.in/ns/ext/midi#MidiEvent")?;
            unsafe { urid_map_fn((&*urid_state as *const Mutex<UridState>) as *mut c_void, c.as_ptr()) }
        };

        let mut atom_buf = Box::new(AtomSequenceBuffer {
            atom_size: 8,
            atom_type: seq_urid,
            unit: 0,
            pad: 0,
            data: [0u8; 1024],
        });

        let mut notify_buf = Box::new(AtomSequenceBuffer {
            atom_size: 1016,
            atom_type: 0,
            unit: 0,
            pad: 0,
            data: [0u8; 1024],
        });

        // Pre-connect atom ports
        {
            let mut act = active_arc.lock().unwrap();
            unsafe {
                if let Some(p) = midi_port {
                    act.instance_mut().connect_port_mut(p, &mut *atom_buf as *mut AtomSequenceBuffer);
                }
                if let Some(p) = notify_port {
                    act.instance_mut().connect_port_mut(p, &mut *notify_buf as *mut AtomSequenceBuffer);
                }
            }
        }

        let (gui_tx, gui_rx) = std::sync::mpsc::channel::<()>();
        let gui_open_tx = Arc::new(Mutex::new(Some(gui_tx)));

        // Locate UI if available
        let mut ui_info = None;
        if let Some(uis) = plugin.uis() {
            for ui in uis.iter() {
                if let Some(bin_node) = ui.binary_uri() {
                    let bin_str = bin_node.as_str().unwrap_or("");
                    let bin_path = if bin_str.starts_with("file://") {
                        bin_str.trim_start_matches("file://")
                    } else {
                        bin_str
                    };
                    let bundle_node = ui.bundle_uri();
                    let bundle_str = bundle_node.as_ref().and_then(|b| b.as_str()).unwrap_or("");
                    let bundle_path = if bundle_str.starts_with("file://") {
                        bundle_str.trim_start_matches("file://")
                    } else {
                        bundle_str
                    };
                    ui_info = Some((bin_path.to_string(), bundle_path.to_string()));
                    break;
                }
            }
        }

        let pending_midi = Arc::new(Mutex::new(Vec::new()));
        let pending_midi_gui = pending_midi.clone();
        let plugin_uri_owned = plugin_uri.to_string();

        if let Some((bin_path, bundle_path)) = ui_info {
            std::thread::Builder::new()
                .name(format!("lv2-gui-{}", plugin_name))
                .spawn(move || {
                    let lib = match unsafe { libloading::Library::new(&bin_path) } {
                        Ok(l) => l,
                        Err(e) => {
                            println!("[jamseq:lv2] Failed to load UI binary: {}", e);
                            return;
                        }
                    };

                    let desc_fn: libloading::Symbol<Lv2UiDescriptorFn> = match unsafe { lib.get(b"lv2ui_descriptor") } {
                        Ok(f) => f,
                        Err(e) => {
                            println!("[jamseq:lv2] Failed to resolve lv2ui_descriptor: {}", e);
                            return;
                        }
                    };

                    let desc_ptr = unsafe { desc_fn(0) };
                    if desc_ptr.is_null() {
                        return;
                    }
                    let desc = unsafe { &*desc_ptr };

                    while let Ok(()) = gui_rx.recv() {
                        println!("[jamseq:lv2] Opening native X11 window on display :0...");
                        if let Ok((conn, screen_num)) = x11rb::connect(None) {
                            let conn = Arc::new(conn);
                            let screen = &conn.setup().roots[screen_num];
                            let win_id = match conn.generate_id() {
                                Ok(id) => id,
                                Err(_) => continue,
                            };

                            let _ = conn.create_window(
                                x11rb::COPY_DEPTH_FROM_PARENT,
                                win_id,
                                screen.root,
                                100, 100,
                                800, 500,
                                0,
                                WindowClass::INPUT_OUTPUT,
                                screen.root_visual,
                                &CreateWindowAux::new()
                                    .background_pixel(screen.black_pixel)
                                    .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
                            );

                            let window_title = format!("{} - Native LV2 GUI", plugin_name);
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

                            let urid_state = Box::new(Mutex::new(UridState {
                                uris: Vec::new(),
                                map: HashMap::new(),
                            }));
                            let urid_map_struct = Box::new(Lv2UridMap {
                                handle: (&*urid_state as *const Mutex<UridState>) as *mut c_void,
                                map: urid_map_fn,
                            });

                            let resize_state = Box::new(ResizeState {
                                conn: conn.clone(),
                                win_id,
                            });
                            let resize_struct = Box::new(Lv2UiResize {
                                handle: (&*resize_state as *const ResizeState) as *mut c_void,
                                ui_resize: host_resize_fn,
                            });

                            let parent_uri = CString::new("http://lv2plug.in/ns/extensions/ui#parent").unwrap();
                            let urid_map_uri = CString::new("http://lv2plug.in/ns/ext/urid#map").unwrap();
                            let resize_uri = CString::new("http://lv2plug.in/ns/extensions/ui#resize").unwrap();
                            let idle_interface_uri = CString::new("http://lv2plug.in/ns/extensions/ui#idleInterface").unwrap();

                            let feat_parent = Lv2Feature {
                                uri: parent_uri.as_ptr(),
                                data: win_id as usize as *mut c_void,
                            };
                            let feat_urid_map = Lv2Feature {
                                uri: urid_map_uri.as_ptr(),
                                data: (&*urid_map_struct as *const Lv2UridMap) as *mut c_void,
                            };
                            let feat_resize = Lv2Feature {
                                uri: resize_uri.as_ptr(),
                                data: (&*resize_struct as *const Lv2UiResize) as *mut c_void,
                            };

                            let features: [*const Lv2Feature; 4] = [
                                &feat_parent,
                                &feat_urid_map,
                                &feat_resize,
                                std::ptr::null(),
                            ];

                            let p_uri = CString::new(plugin_uri_owned.clone()).unwrap();
                            let b_path = CString::new(bundle_path.clone()).unwrap();
                            let mut widget_ptr: *mut c_void = std::ptr::null_mut();

                            let ctrl_context = Box::new(UiController {
                                pending_midi: pending_midi_gui.clone(),
                            });

                            let ui_handle = unsafe {
                                (desc.instantiate)(
                                    desc_ptr,
                                    p_uri.as_ptr(),
                                    b_path.as_ptr(),
                                    Some(ui_write_fn),
                                    (&*ctrl_context as *const UiController) as *mut c_void,
                                    &mut widget_ptr,
                                    features.as_ptr(),
                                )
                            };

                            if ui_handle.is_null() {
                                println!("[jamseq:lv2] Failed to instantiate UI handle");
                                continue;
                            }

                            let child_win_id = widget_ptr as usize as u32;
                            if child_win_id != 0 && child_win_id != win_id {
                                let _ = conn.map_window(child_win_id);
                                let _ = conn.flush();
                            }

                            let idle_interface: Option<&Lv2UiIdleInterface> = if let Some(ext_fn) = desc.extension_data {
                                let ptr = unsafe { ext_fn(idle_interface_uri.as_ptr()) };
                                if !ptr.is_null() {
                                    Some(unsafe { &*(ptr as *const Lv2UiIdleInterface) })
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            println!("[jamseq:lv2] Native LV2 GUI is displayed and active.");

                            'event_loop: loop {
                                while let Ok(Some(event)) = conn.poll_for_event() {
                                    match event {
                                        x11rb::protocol::Event::ClientMessage(msg) => {
                                            if msg.data.as_data32()[0] == wm_delete_window {
                                                println!("[jamseq:lv2] User closed plugin window.");
                                                break 'event_loop;
                                            }
                                        }
                                        x11rb::protocol::Event::DestroyNotify(_) => break 'event_loop,
                                        _ => {}
                                    }
                                }

                                if let Some(idle) = idle_interface {
                                    let res = unsafe { (idle.idle)(ui_handle) };
                                    if res != 0 {
                                        break 'event_loop;
                                    }
                                }

                                let _ = conn.flush();
                                std::thread::sleep(std::time::Duration::from_millis(16));
                            }

                            unsafe {
                                (desc.cleanup)(ui_handle);
                            }
                            let _ = conn.destroy_window(win_id);
                            let _ = conn.flush();
                        }
                    }
                })?;
        }

        Ok(Self {
            uri: plugin_uri.to_string(),
            instance: active_arc,
            midi_port,
            notify_port,
            out_l_port,
            out_r_port,
            pending_midi,
            gui_open_tx,
            out_l: vec![0.0f32; 1024],
            out_r: vec![0.0f32; 1024],
            atom_buf,
            notify_buf,
            seq_urid,
            midi_urid,
        })
    }

    pub fn note_on(&self, note: u8, velocity: f32) {
        let vel_byte = (velocity * 127.0).clamp(0.0, 127.0) as u8;
        self.pending_midi.lock().unwrap().push([0x90, note, vel_byte]);
    }

    pub fn note_off(&self, note: u8) {
        self.pending_midi.lock().unwrap().push([0x80, note, 0]);
    }

    pub fn all_notes_off(&self) {
        self.pending_midi.lock().unwrap().push([0xB0, 123, 0]); // All notes off CC
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

        // Build atom sequence
        let mut offset = 0usize;
        {
            let mut pending = self.pending_midi.lock().unwrap();
            for msg in pending.drain(..) {
                if offset + 19 <= self.atom_buf.data.len() {
                    // time_frames (int64_t = 0)
                    self.atom_buf.data[offset..offset + 8].copy_from_slice(&0i64.to_ne_bytes());
                    offset += 8;
                    // size = 3 (u32)
                    self.atom_buf.data[offset..offset + 4].copy_from_slice(&3u32.to_ne_bytes());
                    offset += 4;
                    // type = midi_urid (u32)
                    self.atom_buf.data[offset..offset + 4].copy_from_slice(&self.midi_urid.to_ne_bytes());
                    offset += 4;
                    // 3 bytes data
                    self.atom_buf.data[offset..offset + 3].copy_from_slice(&msg);
                    offset += 3;
                    // Pad to 8-byte alignment (offset is now 19, pad 5 bytes to 24)
                    let pad = (8 - (offset % 8)) % 8;
                    for i in 0..pad {
                        self.atom_buf.data[offset + i] = 0;
                    }
                    offset += pad;
                }
            }
        }

        self.atom_buf.atom_size = (8 + offset) as u32;
        self.atom_buf.atom_type = self.seq_urid;
        self.atom_buf.unit = 0;
        self.atom_buf.pad = 0;

        self.notify_buf.atom_size = 1032;
        self.notify_buf.atom_type = self.seq_urid;
        self.notify_buf.unit = 0;
        self.notify_buf.pad = 0;

        if let Ok(mut inst) = self.instance.lock() {
            unsafe {
                if let Some(p) = self.midi_port {
                    inst.instance_mut().connect_port_mut(p, &mut *self.atom_buf as *mut AtomSequenceBuffer);
                }
                if let Some(p) = self.notify_port {
                    inst.instance_mut().connect_port_mut(p, &mut *self.notify_buf as *mut AtomSequenceBuffer);
                }
                inst.instance_mut().connect_port_mut(self.out_l_port, self.out_l.as_mut_ptr());
                inst.instance_mut().connect_port_mut(self.out_r_port, self.out_r.as_mut_ptr());

                inst.run(num_samples);
            }
        }

        for i in 0..num_samples {
            scratch_l[i] += self.out_l[i];
            scratch_r[i] += self.out_r[i];
        }
    }
}
