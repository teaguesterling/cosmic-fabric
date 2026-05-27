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

/// The personalization profile's curated set, as include/exclude globs over
/// pattern names (`*`/`?` wildcards, or exact names). A pattern is active when it
/// matches an `include` glob (or `include` is empty = all) and no `exclude` glob.
/// `exclude` wins. A custom pack is just a glob in the config (e.g.
/// `include = ["mypack-*"]`) — pack names are data, never baked into the code.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Surface {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Minimal glob match supporting `*` (any run) and `?` (one char); no wildcard
/// means an exact-name match.
pub fn glob_match(pat: &str, text: &str) -> bool {
    fn rec(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => rec(&p[1..], t) || (!t.is_empty() && rec(p, &t[1..])),
            Some(b'?') => !t.is_empty() && rec(&p[1..], &t[1..]),
            Some(&c) => !t.is_empty() && t[0] == c && rec(&p[1..], &t[1..]),
        }
    }
    rec(pat.as_bytes(), text.as_bytes())
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
    /// The curated working set: every pattern in `all` that passes the
    /// include/exclude globs.
    pub fn active_patterns(&self, all: &[String]) -> Vec<String> {
        all.iter().filter(|p| self.is_active(p)).cloned().collect()
    }

    pub fn is_active(&self, name: &str) -> bool {
        let included = self.surface.include.is_empty()
            || self.surface.include.iter().any(|g| glob_match(g, name));
        let excluded = self.surface.exclude.iter().any(|g| glob_match(g, name));
        included && !excluded
    }

    /// Force a pattern in/out of the active set with an exact-name entry, so the
    /// edit holds regardless of the surrounding globs (exact `exclude` overrides
    /// an `include` glob; exact `include` overrides nothing it doesn't need to).
    pub fn set_active(&mut self, name: &str, on: bool) {
        if on {
            self.surface.exclude.retain(|g| g != name);
            if !self.is_active(name) {
                self.surface.include.push(name.to_string());
            }
        } else {
            self.surface.include.retain(|g| g != name);
            if self.is_active(name) {
                self.surface.exclude.push(name.to_string());
            }
        }
    }

    pub fn toggle_active(&mut self, name: &str) {
        let on = self.is_active(name);
        self.set_active(name, !on);
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
        // The installed policy.toml has a per-pattern override; if load()
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

    #[test]
    fn glob_matches() {
        assert!(glob_match("pack-*", "pack-summarize"));
        assert!(!glob_match("pack-*", "extract_wisdom"));
        assert!(glob_match("*wisdom", "extract_wisdom"));
        assert!(glob_match("extract_*", "extract_wisdom"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exacto"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a?c", "abc"));
    }

    #[test]
    fn active_set_globs_and_toggle() {
        let all: Vec<String> = ["pack-x", "pack-y", "extract_wisdom", "summarize"]
            .iter().map(|s| s.to_string()).collect();
        // empty include = all
        let mut p = Policy::default();
        assert_eq!(p.active_patterns(&all).len(), 4);
        // include glob narrows
        p.surface.include = vec!["pack-*".into()];
        assert_eq!(p.active_patterns(&all), vec!["pack-x", "pack-y"]);
        // toggle one extra pattern in (exact include added)
        p.toggle_active("extract_wisdom");
        assert!(p.is_active("extract_wisdom"));
        assert_eq!(p.active_patterns(&all).len(), 3);
        // toggle a glob-matched one out (exact exclude added, overrides include)
        p.toggle_active("pack-x");
        assert!(!p.is_active("pack-x"));
        assert!(p.is_active("pack-y"));
        assert_eq!(p.active_patterns(&all).len(), 2);
    }
}
