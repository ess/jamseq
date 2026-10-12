use lilv::World;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPlugin {
    pub name: String,
    pub uri_or_path: String,
    pub plugin_format: String, // "LV2" or "CLAP"
    pub category: String,      // "Instrument" or "AudioEffect"
}

pub struct PluginScanner;

impl PluginScanner {
    /// Scan all system and user directories for actual installed LV2 and CLAP plugins
    pub fn scan_all() -> Vec<DiscoveredPlugin> {
        let mut results = Vec::new();

        // 1. Scan LV2 plugins via Lilv
        let world = Box::leak(Box::new(World::with_load_all()));
        for plugin in world.plugins().into_iter() {
            let uri = plugin.uri().as_str().unwrap_or("").to_string();
            let name = plugin.name().as_str().unwrap_or("Unnamed LV2").to_string();

            if !uri.is_empty() {
                results.push(DiscoveredPlugin {
                    name,
                    uri_or_path: uri,
                    plugin_format: "LV2".to_string(),
                    category: "Instrument".to_string(),
                });
            }
        }

        // 2. Scan CLAP plugins in standard directories
        let clap_paths = [
            PathBuf::from("/usr/lib/clap"),
            PathBuf::from("/usr/local/lib/clap"),
            dirs_fallback().join(".clap"),
        ];

        for dir in &clap_paths {
            if dir.exists() {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("clap") {
                            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("CLAP Plugin");
                            results.push(DiscoveredPlugin {
                                name: file_stem.to_string(),
                                uri_or_path: path.to_string_lossy().to_string(),
                                plugin_format: "CLAP".to_string(),
                                category: "Instrument".to_string(),
                            });
                        }
                    }
                }
            }
        }

        results
    }
}

fn dirs_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
    } else {
        PathBuf::from("/tmp")
    }
}
