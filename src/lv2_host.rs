use lilv::World;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

#[repr(C)]
struct Lv2UridMap {
    handle: *mut c_void,
    map: unsafe extern "C" fn(handle: *mut c_void, uri: *const c_char) -> u32,
}

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

#[derive(Clone, Debug)]
pub struct Lv2ControlPort {
    pub index: usize,
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
}

pub struct Lv2Track {
    pub uri: String,
    pub instance: Arc<Mutex<lilv::instance::ActiveInstance>>,
    pub midi_port: Option<usize>,
    pub notify_port: Option<usize>,
    pub out_l_port: usize,
    pub out_r_port: usize,
    pub pending_midi: Arc<Mutex<Vec<[u8; 3]>>>,
    pub control_ports: Arc<Mutex<Vec<Lv2ControlPort>>>,
    pub port_index_map: Arc<Mutex<HashMap<String, usize>>>,
    pub out_l: Vec<f32>,
    pub out_r: Vec<f32>,
    atom_buf: Box<AtomSequenceBuffer>,
    notify_buf: Box<AtomSequenceBuffer>,
    seq_urid: u32,
    midi_urid: u32,
}

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

impl Lv2Track {
    pub fn load(plugin_uri: &str, sample_rate: f32) -> anyhow::Result<Self> {
        println!("[jamseq:lv2] Loading LV2 plugin (headless DSP): {}", plugin_uri);

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
        let feat_buf_sz = lv2_raw::LV2Feature {
            uri: CString::new("http://lv2plug.in/ns/ext/buf-size#boundedBlockLength")?.into_raw(),
            data: std::ptr::null_mut(),
        };

        // Construct options array terminated by zeroed option
        #[repr(C)]
        struct Lv2Option {
            context: i32,
            subject: u32,
            key: u32,
            size: u32,
            type_: u32,
            value: *const c_void,
        }

        let max_block_sz = Box::leak(Box::new(4096u32));
        let min_block_sz = Box::leak(Box::new(32u32));
        let int_type_urid = {
            let c = CString::new("http://lv2plug.in/ns/ext/atom#Int")?;
            unsafe { urid_map_fn(urid_map_struct.handle, c.as_ptr()) }
        };
        let max_block_urid = {
            let c = CString::new("http://lv2plug.in/ns/ext/buf-size#maxBlockLength")?;
            unsafe { urid_map_fn(urid_map_struct.handle, c.as_ptr()) }
        };
        let min_block_urid = {
            let c = CString::new("http://lv2plug.in/ns/ext/buf-size#minBlockLength")?;
            unsafe { urid_map_fn(urid_map_struct.handle, c.as_ptr()) }
        };

        let options_array = Box::leak(Box::new([
            Lv2Option {
                context: 0, // LV2_OPTIONS_INSTANCE
                subject: 0,
                key: max_block_urid,
                size: 4,
                type_: int_type_urid,
                value: (max_block_sz as *const u32) as *const c_void,
            },
            Lv2Option {
                context: 0,
                subject: 0,
                key: min_block_urid,
                size: 4,
                type_: int_type_urid,
                value: (min_block_sz as *const u32) as *const c_void,
            },
            Lv2Option {
                context: 0,
                subject: 0,
                key: 0,
                size: 0,
                type_: 0,
                value: std::ptr::null(),
            },
        ]));

        let feat_options = lv2_raw::LV2Feature {
            uri: CString::new("http://lv2plug.in/ns/ext/options#options")?.into_raw(),
            data: (options_array.as_ptr()) as *mut c_void,
        };

        let features = [feat_urid_map, feat_worker, feat_buf_sz, feat_options];

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

        // Determine audio, MIDI, and control ports
        let num_ports = plugin.ports_count();
        let mut midi_port = None;
        let mut notify_port = None;
        let mut audio_outs = Vec::new();
        let mut control_ports = Vec::new();
        let mut port_index_map = HashMap::new();

        let input_class = world.new_uri("http://lv2plug.in/ns/lv2core#InputPort");
        let output_class = world.new_uri("http://lv2plug.in/ns/lv2core#OutputPort");
        let control_class = world.new_uri("http://lv2plug.in/ns/lv2core#ControlPort");
        let audio_class = world.new_uri("http://lv2plug.in/ns/lv2core#AudioPort");
        let atom_class = world.new_uri("http://lv2plug.in/ns/ext/atom#AtomPort");

        for i in 0..num_ports {
            if let Some(port) = plugin.port_by_index(i) {
                let sym_node = port.symbol();
                let symbol = sym_node
                    .as_ref()
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();

                let is_input = port.is_a(&input_class);
                let is_output = port.is_a(&output_class);
                let is_control = port.is_a(&control_class);
                let is_audio = port.is_a(&audio_class);
                let is_atom = port.is_a(&atom_class);

                if is_input && (is_atom || symbol == "control" || symbol.contains("midi")) {
                    if midi_port.is_none() {
                        midi_port = Some(i);
                    }
                } else if is_output && (is_atom || symbol == "notify") {
                    notify_port = Some(i);
                } else if is_output && is_audio {
                    audio_outs.push(i);
                } else if is_input && is_control {
                    let name = port.name()
                        .and_then(|n| n.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| {
                            if !symbol.is_empty() { symbol.clone() } else { format!("Param {}", i) }
                        });
                    let range = port.range();
                    let def_val = range.default.as_ref().and_then(|n| n.as_float()).unwrap_or(0.0);
                    let min_val = range.minimum.as_ref().and_then(|n| n.as_float()).unwrap_or(0.0);
                    let max_val = range.maximum.as_ref().and_then(|n| n.as_float()).unwrap_or(1.0);

                    let idx = control_ports.len();
                    port_index_map.insert(name.clone(), idx);
                    port_index_map.insert(name.to_lowercase(), idx);
                    if !symbol.is_empty() {
                        port_index_map.insert(symbol.clone(), idx);
                    }

                    control_ports.push(Lv2ControlPort {
                        index: i,
                        name,
                        min: min_val,
                        max: max_val,
                        default: def_val,
                        value: def_val,
                    });
                }
            }
        }

        let out_l_port = if audio_outs.len() >= 2 { audio_outs[0] } else { 2.min(num_ports - 1) };
        let out_r_port = if audio_outs.len() >= 2 { audio_outs[1] } else { 3.min(num_ports - 1) };

        println!(
            "[jamseq:lv2] Configured ports: MIDI In: {:?}, Notify: {:?}, Out L: {}, Out R: {}, Control Ports: {}",
            midi_port, notify_port, out_l_port, out_r_port, control_ports.len()
        );

        let active_instance = unsafe { instance.activate() };
        let active_arc = Arc::new(Mutex::new(active_instance));

        // Get URIDs for Atom Sequence and MIDI event
        let seq_urid = unsafe {
            let c = CString::new("http://lv2plug.in/ns/ext/atom#Sequence")?;
            urid_map_fn(urid_map_struct.handle, c.as_ptr())
        };
        let midi_urid = unsafe {
            let c = CString::new("http://lv2plug.in/ns/ext/midi#MidiEvent")?;
            urid_map_fn(urid_map_struct.handle, c.as_ptr())
        };

        let mut atom_buf = Box::new(AtomSequenceBuffer {
            atom_size: 0,
            atom_type: seq_urid,
            unit: 0,
            pad: 0,
            data: [0u8; 1024],
        });
        let mut notify_buf = Box::new(AtomSequenceBuffer {
            atom_size: 0,
            atom_type: seq_urid,
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

        let pending_midi = Arc::new(Mutex::new(Vec::new()));

        Ok(Self {
            uri: plugin_uri.to_string(),
            instance: active_arc,
            midi_port,
            notify_port,
            out_l_port,
            out_r_port,
            pending_midi,
            control_ports: Arc::new(Mutex::new(control_ports)),
            port_index_map: Arc::new(Mutex::new(port_index_map)),
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

    pub fn get_parameters(&self) -> Vec<(String, f32, f32, f32)> {
        let ports = self.control_ports.lock().unwrap();
        ports.iter().map(|p| (p.name.clone(), p.value, p.min, p.max)).collect()
    }

    pub fn set_parameter(&self, name: &str, value: f32) {
        let mut ports = self.control_ports.lock().unwrap();
        let map = self.port_index_map.lock().unwrap();
        let idx = map.get(name).or_else(|| map.get(&name.to_lowercase()));
        if let Some(&i) = idx {
            if let Some(port) = ports.get_mut(i) {
                port.value = value.clamp(port.min, port.max);
            }
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

                // Connect control ports to live values
                if let Ok(mut ports) = self.control_ports.lock() {
                    for port in ports.iter_mut() {
                        inst.instance_mut().connect_port_mut(port.index, &mut port.value as *mut f32);
                    }
                }

                inst.run(num_samples);
            }
        }

        for i in 0..num_samples {
            scratch_l[i] += self.out_l[i];
            scratch_r[i] += self.out_r[i];
        }
    }
}
