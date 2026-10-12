use lilv::World;

fn main() {
    println!("[test_lv2] Initializing Lilv World and discovering installed LV2 plugins...");
    let world = World::with_load_all();
    println!("Scanning LV2 plugins on this system...");

    let mut found_avldrums = Vec::new();
    let mut total_count = 0;

    for plugin in world.plugins().into_iter() {
        total_count += 1;
        let uri_node = plugin.uri();
        let name_node = plugin.name();
        let uri = uri_node.as_str().unwrap_or("unknown");
        let name = name_node.as_str().unwrap_or("unknown");

        if uri.contains("avldrum")
            || name.to_lowercase().contains("avl")
            || name.to_lowercase().contains("black pearl")
            || name.to_lowercase().contains("red zeppelin")
        {
            found_avldrums.push((name.to_string(), uri.to_string()));
        }
    }

    println!("Total LV2 plugins discovered: {}", total_count);
    println!("\nDiscovered AVL Drumkits plugins:");
    for (name, uri) in &found_avldrums {
        println!("  • Name: {:30} | URI: {}", name, uri);
    }

    println!("\n--- Inspecting LV2 UIs ---");
    for plugin in world.plugins().into_iter() {
        let name_node = plugin.name();
        let name = name_node.as_str().unwrap_or("unknown");
        if let Some(uis) = plugin.uis() {
            let ui_list: Vec<_> = uis.iter().collect();
            if !ui_list.is_empty() {
                println!("Plugin '{}' has {} UI(s):", name, ui_list.len());
                for ui in ui_list {
                    let ui_uri = ui.uri().as_str().unwrap_or("?").to_string();
                    let bundle = ui.bundle_uri().map(|b| b.as_str().unwrap_or("?").to_string()).unwrap_or_default();
                    let binary = ui.binary_uri().map(|b| b.as_str().unwrap_or("?").to_string()).unwrap_or_default();
                    let classes: Vec<String> = ui.classes().into_iter().map(|c| c.as_str().unwrap_or("?").to_string()).collect();
                    println!("    UI URI: {}", ui_uri);
                    println!("    Classes: {:?}", classes);
                    println!("    Bundle: {}", bundle);
                    println!("    Binary: {}", binary);
                }
            }
        }
    }
}
