//! Typed read/write of `~/.config/cosmic-fabric/policy.toml` for the settings
//! window. The daemon re-reads the file per run, so edits take effect live.
//! (Note: round-tripping via the `toml` crate drops the example's comments.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/cosmic-fabric/policy.toml")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPick {
    pub model: String,
    pub vendor: String,
    #[serde(default)]
    pub extra: Vec<String>,
}

impl Default for ModelPick {
    fn default() -> Self {
        Self {
            model: "qwen3:14b-iq4xs".into(),
            vendor: "Ollama".into(),
            extra: vec!["--thinking=off".into(), "--suppress-think".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputCfg {
    pub mode: String,
}
impl Default for OutputCfg {
    fn default() -> Self {
        Self { mode: "notify".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaCfg {
    pub bin: String,
    pub url: String,
    pub warn_below_gpu: u32,
}
impl Default for OllamaCfg {
    fn default() -> Self {
        Self {
            bin: "/opt/ollama/bin/ollama".into(),
            url: "http://localhost:11434".into(),
            warn_below_gpu: 99,
        }
    }
}

/// The personalization profile's curated set: which patterns are surfaced in the
/// quick surfaces (kit) and the run dropdown. Empty → fall back to `scribe-*`
/// (back-compat for configs written before curation existed).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Surface {
    #[serde(default)]
    pub active: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Policy {
    #[serde(default)]
    pub default: ModelPick,
    #[serde(default)]
    pub patterns: BTreeMap<String, ModelPick>,
    #[serde(default)]
    pub output: OutputCfg,
    #[serde(default)]
    pub ollama: OllamaCfg,
    #[serde(default)]
    pub surface: Surface,
}

impl Policy {
    /// The curated working set, intersected with what fabric actually has.
    /// Falls back to `scribe-*` when nothing is curated yet.
    pub fn active_patterns(&self, all: &[String]) -> Vec<String> {
        if self.surface.active.is_empty() {
            all.iter().filter(|p| p.starts_with("scribe-")).cloned().collect()
        } else {
            self.surface
                .active
                .iter()
                .filter(|a| all.iter().any(|p| p == *a))
                .cloned()
                .collect()
        }
    }

    pub fn is_active(&self, name: &str) -> bool {
        if self.surface.active.is_empty() {
            name.starts_with("scribe-")
        } else {
            self.surface.active.iter().any(|a| a == name)
        }
    }

    /// Toggle a pattern's membership in the curated set. The first toggle
    /// "materializes" the implicit scribe-* default into an explicit list so the
    /// user's edit sticks.
    pub fn toggle_active(&mut self, name: &str, all: &[String]) {
        if self.surface.active.is_empty() {
            self.surface.active = self.active_patterns(all);
        }
        if let Some(i) = self.surface.active.iter().position(|a| a == name) {
            self.surface.active.remove(i);
        } else {
            self.surface.active.push(name.to_string());
        }
    }
}

pub fn load() -> Policy {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(p: &Policy) -> Result<(), String> {
    let s = toml::to_string_pretty(p).map_err(|e| e.to_string())?;
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_policy_loads_and_round_trips() {
        let p = load();
        eprintln!(
            "loaded: default={} ({}), patterns={}, mode={}, ollama={}",
            p.default.model,
            p.default.vendor,
            p.patterns.len(),
            p.output.mode,
            p.ollama.url
        );
        // The installed policy.toml has a scribe-visualize override; if load()
        // had silently fallen back to Default, patterns would be empty (and a
        // save would clobber the user's config). Guard against that.
        assert!(
            !p.patterns.is_empty(),
            "policy.toml parsed to empty patterns — load() likely fell back to Default"
        );
        // Data survives a save/load round-trip.
        let s = toml::to_string_pretty(&p).unwrap();
        let p2: Policy = toml::from_str(&s).unwrap();
        assert_eq!(p.default.model, p2.default.model);
        assert_eq!(p.patterns.len(), p2.patterns.len());
        assert_eq!(p.output.mode, p2.output.mode);
    }
}
