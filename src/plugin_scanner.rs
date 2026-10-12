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

        // 2. Scan CLAP plugins in standard and bundled directories
        let mut clap_paths = vec![
            PathBuf::from("/usr/lib/clap"),
            PathBuf::from("/usr/local/lib/clap"),
            dirs_fallback().join(".clap"),
            PathBuf::from("plugins"),
        ];

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                clap_paths.push(dir.join("plugins"));
            }
        }

        for dir in &clap_paths {
            if dir.exists() {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let is_clap = path.extension().and_then(|s| s.to_str()) == Some("clap");
                        let is_so = path.extension().and_then(|s| s.to_str()) == Some("so");
                        if is_clap || is_so {
                            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("CLAP Plugin");
                            let display_name = if file_stem.starts_with("lib") {
                                &file_stem[3..]
                            } else {
                                file_stem
                            };
                            let uri_str = path.to_string_lossy().to_string();
                            if !results.iter().any(|r| r.uri_or_path == uri_str) {
                                results.push(DiscoveredPlugin {
                                    name: display_name.to_string(),
                                    uri_or_path: uri_str,
                                    plugin_format: "CLAP".to_string(),
                                    category: "Instrument".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // 3. Ensure bundled / detected JamSynth CLAP plugin is always available as a first-class plugin
        if let Some(jamsynth_path) = find_jamsynth_plugin() {
            let uri_str = jamsynth_path.to_string_lossy().to_string();
            if !results.iter().any(|r| r.name == "JamSynth" || r.name == "jamsynth" || r.uri_or_path == uri_str) {
                results.insert(0, DiscoveredPlugin {
                    name: "JamSynth".to_string(),
                    uri_or_path: uri_str,
                    plugin_format: "CLAP".to_string(),
                    category: "Instrument".to_string(),
                });
            }
        }

        results
    }
}

/// Find the location of the compiled or bundled JamSynth CLAP plugin library
pub fn find_jamsynth_plugin() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("JAMSYNTH_CLAP_PATH") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("plugins/JamSynth.clap"));
            candidates.push(dir.join("plugins/libjamsynth.so"));
            candidates.push(dir.join("JamSynth.clap"));
            candidates.push(dir.join("libjamsynth.so"));
        }
    }

    // Common dev / workspace / project paths
    candidates.push(PathBuf::from("plugins/JamSynth.clap"));
    candidates.push(PathBuf::from("plugins/libjamsynth.so"));
    candidates.push(PathBuf::from("target/release/libjamsynth.so"));
    candidates.push(PathBuf::from("target/debug/libjamsynth.so"));
    candidates.push(PathBuf::from("../target/release/libjamsynth.so"));
    candidates.push(PathBuf::from("../target/debug/libjamsynth.so"));
    candidates.push(PathBuf::from("../../target/release/libjamsynth.so"));
    candidates.push(PathBuf::from("../../target/debug/libjamsynth.so"));

    // Standard system/user CLAP directories
    candidates.push(dirs_fallback().join(".clap/JamSynth.clap"));
    candidates.push(dirs_fallback().join(".clap/libjamsynth.so"));
    candidates.push(PathBuf::from("/usr/lib/clap/JamSynth.clap"));
    candidates.push(PathBuf::from("/usr/lib/clap/libjamsynth.so"));
    candidates.push(PathBuf::from("/usr/local/lib/clap/JamSynth.clap"));
    candidates.push(PathBuf::from("/usr/local/lib/clap/libjamsynth.so"));

    for candidate in candidates {
        if candidate.exists() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    None
}

fn dirs_fallback() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
    } else {
        PathBuf::from("/tmp")
    }
}
