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
    println!("  -> Plugin requested resize: {}x{}", width, height);
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

unsafe extern "C" fn dummy_write_fn(
    _controller: *mut c_void,
    port_index: u32,
    buffer_size: u32,
    _port_protocol: u32,
    _buffer: *const c_void,
) {
    println!("  -> UI wrote to port {} (size: {})", port_index, buffer_size);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("[test_lv2_gui] Testing AVL Drums native X11 UI...");

    let (conn, screen_num) = x11rb::connect(None)?;
    let conn = Arc::new(conn);
    let screen = &conn.setup().roots[screen_num];
    let win_id = conn.generate_id()?;
    let width = 800u16;
    let height = 500u16;

    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win_id,
        screen.root,
        50,
        50,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(screen.black_pixel)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
    )?;

    conn.change_property8(
        PropMode::REPLACE,
        win_id,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"AVL Drumkits (Black Pearl) - Native LV2 GUI",
    )?;

    let wm_protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let wm_delete_window = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property32(
        PropMode::REPLACE,
        win_id,
        wm_protocols,
        AtomEnum::ATOM,
        &[wm_delete_window],
    )?;

    conn.map_window(win_id)?;
    conn.flush()?;
    println!("  -> Created X11 Window ID: {}", win_id);

    // Setup URID Map
    let urid_state = Box::new(Mutex::new(UridState {
        uris: Vec::new(),
        map: HashMap::new(),
    }));
    let urid_map_struct = Box::new(Lv2UridMap {
        handle: (&*urid_state as *const Mutex<UridState>) as *mut c_void,
        map: urid_map_fn,
    });

    // Setup Resize feature
    let resize_state = Box::new(ResizeState {
        conn: conn.clone(),
        win_id,
    });
    let resize_struct = Box::new(Lv2UiResize {
        handle: (&*resize_state as *const ResizeState) as *mut c_void,
        ui_resize: host_resize_fn,
    });

    let parent_uri = CString::new("http://lv2plug.in/ns/extensions/ui#parent")?;
    let urid_map_uri = CString::new("http://lv2plug.in/ns/ext/urid#map")?;
    let resize_uri = CString::new("http://lv2plug.in/ns/extensions/ui#resize")?;
    let idle_interface_uri = CString::new("http://lv2plug.in/ns/extensions/ui#idleInterface")?;

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

    let lib = unsafe { libloading::Library::new("/usr/lib/lv2/avldrums.lv2/avldrumsUI_gl.so")? };
    let desc_fn: libloading::Symbol<Lv2UiDescriptorFn> = unsafe { lib.get(b"lv2ui_descriptor")? };

    let desc_ptr = unsafe { desc_fn(0) };
    if desc_ptr.is_null() {
        return Err("lv2ui_descriptor(0) returned null".into());
    }
    let desc = unsafe { &*desc_ptr };
    println!("  -> Found UI descriptor URI: {:?}", unsafe { CStr::from_ptr(desc.uri) });

    let plugin_uri = CString::new("http://gareus.org/oss/lv2/avldrums#BlackPearl")?;
    let bundle_path = CString::new("/usr/lib/lv2/avldrums.lv2/")?;

    let mut widget_ptr: *mut c_void = std::ptr::null_mut();
    let ui_handle = unsafe {
        (desc.instantiate)(
            desc_ptr,
            plugin_uri.as_ptr(),
            bundle_path.as_ptr(),
            Some(dummy_write_fn),
            std::ptr::null_mut(),
            &mut widget_ptr,
            features.as_ptr(),
        )
    };

    if ui_handle.is_null() {
        return Err("Failed to instantiate LV2 UI!".into());
    }
    println!("  -> Successfully instantiated LV2 UI! Widget: {:?}", widget_ptr);

    // If widget returned a child window ID, reparent or map it if needed
    let child_win_id = widget_ptr as usize as u32;
    if child_win_id != 0 && child_win_id != win_id {
        println!("  -> Plugin created child window ID: {}", child_win_id);
        let _ = conn.map_window(child_win_id);
        let _ = conn.flush();
    }

    // Check for idleInterface
    let idle_interface: Option<&Lv2UiIdleInterface> = if let Some(ext_fn) = desc.extension_data {
        let ptr = unsafe { ext_fn(idle_interface_uri.as_ptr()) };
        if !ptr.is_null() {
            println!("  -> Plugin implements LV2_UI__idleInterface!");
            Some(unsafe { &*(ptr as *const Lv2UiIdleInterface) })
        } else {
            None
        }
    } else {
        None
    };

    println!("\n=======================================================");
    println!("🥁 AVL Drums Native LV2 Window is LIVE on your desktop!");
    println!("Close the window (or press Ctrl+C in this terminal) to exit.");
    println!("=======================================================\n");

    'event_loop: loop {
        while let Ok(Some(event)) = conn.poll_for_event() {
            match event {
                x11rb::protocol::Event::ClientMessage(msg) => {
                    if msg.data.as_data32()[0] == wm_delete_window {
                        println!("\nClosing window...");
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
                println!("UI idle callback signaled close (code {})", res);
                break 'event_loop;
            }
        }

        let _ = conn.flush();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    println!("Cleaning up LV2 UI...");
    unsafe {
        (desc.cleanup)(ui_handle);
    }
    let _ = conn.destroy_window(win_id);
    let _ = conn.flush();
    println!("Done.");

    Ok(())
}
