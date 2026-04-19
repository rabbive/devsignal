//! Core types, configuration, and presence snapshot building for `devsignal`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// Top-level config loaded from `~/.config/devsignal/config.toml` (or `--config`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_min_push_interval_secs")]
    pub min_push_interval_secs: u64,
    /// When no agent CLI is detected: show an idle line, or clear Rich Presence entirely.
    #[serde(default)]
    pub idle_mode: IdleMode,
    /// Append the working-directory **basename** for the winning agent process (never full paths).
    #[serde(default)]
    pub show_cwd_basename: bool,
    pub discord: DiscordSection,
    #[serde(default)]
    pub agents: Vec<AgentRule>,
}

/// What to do when no configured agent process is running.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IdleMode {
    /// Show `Idle` / host in Discord (default).
    #[default]
    Status,
    /// Call Discord `CLEAR_ACTIVITY` so nothing is displayed for this app.
    Clear,
}

fn default_poll_interval_secs() -> u64 {
    2
}

fn default_min_push_interval_secs() -> u64 {
    20
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscordSection {
    /// Discord Application (Rich Presence) client ID.
    pub client_id: String,
    #[serde(default = "default_large_image")]
    pub large_image: String,
    #[serde(default)]
    pub large_text: String,
}

fn default_large_image() -> String {
    "devsignal".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentRule {
    /// Stable id: `claude_code`, `codex`, `opencode`, ...
    pub id: String,
    /// Human label for Discord `details`, e.g. "Claude Code".
    #[serde(default)]
    pub label: Option<String>,
    /// `sysinfo` process names to match (case-insensitive).
    #[serde(default)]
    pub process_names: Vec<String>,
    /// If non-empty, require at least one of these substrings in the command line.
    #[serde(default)]
    pub argv_substrings: Vec<String>,
    /// Discord `large_image` key for this agent (falls back to global).
    #[serde(default)]
    pub large_image: Option<String>,
    /// Lower number = higher priority when multiple agents match.
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    100
}

impl Config {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw).context("parse config TOML")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn default_path() -> std::path::PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        base.join("devsignal").join("config.toml")
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.discord.client_id.trim().is_empty(),
            "discord.client_id must be set"
        );
        anyhow::ensure!(!self.agents.is_empty(), "at least one [[agents]] entry is required");
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAgent {
    pub id: String,
    pub label: String,
    pub large_image: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceView {
    pub details: String,
    pub state: String,
    pub large_image: String,
    pub large_text: String,
    pub start_timestamp_unix: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Debouncer {
    min_interval: Duration,
    last_payload: Option<PresenceView>,
    last_push: Option<Instant>,
}

impl Debouncer {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_payload: None,
            last_push: None,
        }
    }

    /// Returns `true` if Discord should be updated now.
    pub fn should_push(&mut self, next: &PresenceView, force: bool) -> bool {
        if force {
            self.last_payload = Some(next.clone());
            self.last_push = Some(Instant::now());
            return true;
        }
        if self.last_payload.as_ref() == Some(next) {
            return false;
        }
        let now = Instant::now();
        if let Some(t) = self.last_push {
            if now.duration_since(t) < self.min_interval {
                return false;
            }
        }
        self.last_payload = Some(next.clone());
        self.last_push = Some(now);
        true
    }
}

/// Map common macOS bundle IDs to a short host label for Discord `state`.
/// Covers Tier A/B editors plus common terminals (Tier C).
pub fn host_label_for_bundle(bundle_id: &str) -> String {
    let map: HashMap<&str, &str> = [
        ("com.todesktop.230313mzl4w4u92", "Cursor"),
        ("com.microsoft.VSCode", "VS Code"),
        ("com.vscodium", "VSCodium"),
        ("dev.zed.Zed", "Zed"),
        ("com.apple.dt.Xcode", "Xcode"),
        ("com.sublimetext.4", "Sublime Text"),
        ("com.sublimetext.3", "Sublime Text"),
        ("com.panic.Nova", "Nova"),
        ("com.jetbrains.fleet", "Fleet"),
        ("com.apple.Terminal", "Terminal"),
        ("com.googlecode.iterm2", "iTerm2"),
        ("dev.warp.Warp-Stable", "Warp"),
        ("com.mitchellh.ghostty", "Ghostty"),
        ("net.kovidgoyal.kitty", "Kitty"),
        ("org.alacritty.Alacritty", "Alacritty"),
    ]
    .into_iter()
    .collect();

    if let Some(l) = map.get(bundle_id) {
        return (*l).to_string();
    }
    if bundle_id.starts_with("com.jetbrains.") || bundle_id.contains("jetbrains") {
        return "JetBrains".to_string();
    }
    if bundle_id.starts_with("com.google.android.studio") {
        return "Android Studio".to_string();
    }
    bundle_id.to_string()
}

/// Return a single directory name for presence text (last path segment). Never returns full paths.
pub fn redact_cwd_basename(cwd: &Path) -> Option<String> {
    let leaf = cwd.file_name()?.to_str()?.trim();
    if leaf.is_empty() || leaf == "." {
        return None;
    }
    // Avoid noisy system roots.
    if cwd.components().count() <= 1 {
        return None;
    }
    Some(leaf.to_string())
}

/// Choose the winning agent: lowest `priority` value wins; ties break on lower PID.
/// Returns the matching process id for optional CWD lookup.
pub fn select_active_agent(mut matches: Vec<(AgentRule, u32)>) -> Option<(ActiveAgent, u32)> {
    if matches.is_empty() {
        return None;
    }
    matches.sort_by(|a, b| {
        a.0.priority
            .cmp(&b.0.priority)
            .then_with(|| a.1.cmp(&b.1))
    });
    let (rule, pid) = matches.into_iter().next()?;
    let label = rule
        .label
        .clone()
        .unwrap_or_else(|| rule.id.replace('_', " "));
    let large = rule
        .large_image
        .clone()
        .unwrap_or_else(|| "devsignal".to_string());
    let agent = ActiveAgent {
        id: rule.id.clone(),
        label,
        large_image: large,
    };
    Some((agent, pid))
}

pub fn build_presence_view(
    cfg: &Config,
    agent: Option<&ActiveAgent>,
    host_bundle_id: Option<&str>,
    session_start_unix: Option<u64>,
    cwd_basename: Option<&str>,
) -> PresenceView {
    let host = host_bundle_id
        .map(host_label_for_bundle)
        .unwrap_or_else(|| "macOS".to_string());

    let cwd_suffix = cwd_basename
        .filter(|s| !s.is_empty())
        .map(|s| format!(" · {s}"))
        .unwrap_or_default();

    match agent {
        Some(a) => PresenceView {
            details: a.label.clone(),
            state: format!("In {host}{cwd_suffix}"),
            large_image: a.large_image.clone(),
            large_text: cfg.discord.large_text.clone(),
            start_timestamp_unix: session_start_unix,
        },
        None => PresenceView {
            details: "Idle".to_string(),
            state: format!("{host} · no agent CLI detected"),
            large_image: cfg.discord.large_image.clone(),
            large_text: cfg.discord.large_text.clone(),
            start_timestamp_unix: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn redact_cwd_basename_last_segment() {
        let p = PathBuf::from("/Users/demo/projects/myapp");
        assert_eq!(
            redact_cwd_basename(&p).as_deref(),
            Some("myapp")
        );
    }
}
