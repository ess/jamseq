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

        // 3. Scan VST3 plugins in standard directories
        let mut vst3_paths = vec![
            PathBuf::from("/usr/lib/vst3"),
            PathBuf::from("/usr/local/lib/vst3"),
            dirs_fallback().join(".vst3"),
            PathBuf::from("plugins"),
        ];

        if let Ok(vst3_env) = std::env::var("VST3_PATH") {
            for p in std::env::split_paths(&vst3_env) {
                vst3_paths.push(p);
            }
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                vst3_paths.push(dir.join("plugins"));
            }
        }

        for dir in &vst3_paths {
            if dir.exists() {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let check_path = |p: std::path::PathBuf, res: &mut Vec<DiscoveredPlugin>| {
                            let is_vst3 = p.extension().and_then(|s| s.to_str()) == Some("vst3")
                                || p.to_string_lossy().ends_with(".vst3");
                            if is_vst3 {
                                let file_stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("VST3 Plugin");
                                let display_name = if file_stem.starts_with("lib") {
                                    &file_stem[3..]
                                } else {
                                    file_stem
                                };
                                let uri_str = p.to_string_lossy().to_string();
                                let lower = display_name.to_lowercase();
                                let category = if lower.contains("effect") || lower.contains("fx") {
                                    "AudioEffect".to_string()
                                } else {
                                    "Instrument".to_string()
                                };
                                if !res.iter().any(|r| r.uri_or_path == uri_str) {
                                    res.push(DiscoveredPlugin {
                                        name: display_name.to_string(),
                                        uri_or_path: uri_str,
                                        plugin_format: "VST3".to_string(),
                                        category,
                                    });
                                }
                            }
                        };

                        if path.is_dir() && path.extension().and_then(|s| s.to_str()) != Some("vst3") {
                            // Scan one sub-level (e.g. /usr/lib/vst3/Vendor/Plugin.vst3)
                            if let Ok(sub_entries) = fs::read_dir(&path) {
                                for sub in sub_entries.flatten() {
                                    check_path(sub.path(), &mut results);
                                }
                            }
                        } else {
                            check_path(path, &mut results);
                        }
                    }
                }
            }
        }

        // 4. Ensure bundled / detected JamSynth CLAP plugin is always available as a first-class plugin
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
