//! Core types, configuration, and presence snapshot building for `devsignal`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
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
    /// If non-empty, require at least one of these substrings in the command line (case-insensitive).
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

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.discord.client_id.trim().is_empty(),
            "discord.client_id must be set"
        );
        anyhow::ensure!(
            !self.agents.is_empty(),
            "at least one [[agents]] entry is required"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAgent {
    pub id: String,
    pub label: String,
    pub large_image: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// Known bundle id → short label for Discord `state` (editors, terminals, JetBrains SKUs).
pub const HOST_BUNDLE_LABELS: &[(&str, &str)] = &[
    ("com.todesktop.230313mzl4w4u92", "Cursor"),
    ("com.microsoft.VSCode", "VS Code"),
    ("com.vscodium", "VSCodium"),
    ("dev.zed.Zed", "Zed"),
    ("com.apple.dt.Xcode", "Xcode"),
    ("com.sublimetext.4", "Sublime Text"),
    ("com.sublimetext.3", "Sublime Text"),
    ("com.panic.Nova", "Nova"),
    ("com.jetbrains.fleet", "Fleet"),
    ("com.jetbrains.intellij", "IntelliJ IDEA"),
    ("com.jetbrains.pycharm", "PyCharm"),
    ("com.jetbrains.WebStorm", "WebStorm"),
    ("com.jetbrains.goland", "GoLand"),
    ("com.jetbrains.rubymine", "RubyMine"),
    ("com.jetbrains.clion", "CLion"),
    ("com.jetbrains.phpstorm", "PhpStorm"),
    ("com.jetbrains.rustrover", "RustRover"),
    ("com.jetbrains.datagrip", "DataGrip"),
    ("com.jetbrains.aqua", "Aqua"),
    ("com.apple.Terminal", "Terminal"),
    ("com.googlecode.iterm2", "iTerm2"),
    ("dev.warp.Warp-Stable", "Warp"),
    ("com.mitchellh.ghostty", "Ghostty"),
    ("net.kovidgoyal.kitty", "Kitty"),
    ("org.alacritty.Alacritty", "Alacritty"),
    ("co.zeit.hyper", "Hyper"),
    ("com.raphaelamorim.tabby", "Tabby"),
    ("com.github.wez.wezterm", "WezTerm"),
];

/// Map common macOS bundle IDs to a short host label for Discord `state`.
/// Covers Tier A/B editors plus common terminals (Tier C).
pub fn host_label_for_bundle(bundle_id: &str) -> String {
    for (id, label) in HOST_BUNDLE_LABELS {
        if *id == bundle_id {
            return (*label).to_string();
        }
    }
    if bundle_id.starts_with("com.jetbrains.") || bundle_id.contains("jetbrains") {
        return "JetBrains".to_string();
    }
    if bundle_id.starts_with("com.google.android.studio") {
        return "Android Studio".to_string();
    }
    bundle_id.to_string()
}

/// Match a process against an agent rule: `process_names` vs process `name` (case-insensitive)
/// or vs the **basename** of `cmd[0]` (for wrapped CLIs, e.g. `node …/codex.js`), then optional
/// `argv_substrings` against the full command line (case-insensitive).
pub fn process_matches_rule(name: &str, cmd: &[impl AsRef<OsStr>], rule: &AgentRule) -> bool {
    let name_l = name.to_lowercase();
    let name_hit = rule
        .process_names
        .iter()
        .any(|n| n.to_lowercase() == name_l);
    let argv0_hit = cmd.first().is_some_and(|a| {
        let base = Path::new(a.as_ref())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let base_l = base.to_lowercase();
        rule.process_names
            .iter()
            .any(|n| n.to_lowercase() == base_l)
    });
    if !name_hit && !argv0_hit {
        return false;
    }
    if rule.argv_substrings.is_empty() {
        return true;
    }
    let joined = cmd
        .iter()
        .map(|s| s.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let joined_l = joined.to_lowercase();
    rule.argv_substrings
        .iter()
        .any(|needle| joined_l.contains(&needle.to_lowercase()))
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
    matches.sort_by(|a, b| a.0.priority.cmp(&b.0.priority).then_with(|| a.1.cmp(&b.1)));
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
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn sample_config() -> Config {
        Config {
            poll_interval_secs: 2,
            min_push_interval_secs: 20,
            idle_mode: IdleMode::Status,
            show_cwd_basename: false,
            discord: DiscordSection {
                client_id: "123".to_string(),
                large_image: "devsignal".to_string(),
                large_text: "devsignal".to_string(),
            },
            agents: vec![],
        }
    }

    fn rule(id: &str, priority: i32) -> AgentRule {
        AgentRule {
            id: id.to_string(),
            label: None,
            process_names: vec![],
            argv_substrings: vec![],
            large_image: None,
            priority,
        }
    }

    #[test]
    fn redact_cwd_basename_last_segment() {
        let p = PathBuf::from("/Users/demo/projects/myapp");
        assert_eq!(redact_cwd_basename(&p).as_deref(), Some("myapp"));
    }

    #[test]
    fn debouncer_equal_payload_suppressed() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let v = PresenceView {
            details: "A".into(),
            state: "B".into(),
            large_image: "x".into(),
            large_text: "".into(),
            start_timestamp_unix: None,
        };
        assert!(d.should_push(&v, true));
        assert!(!d.should_push(&v, false));
    }

    #[test]
    fn debouncer_new_payload_before_min_interval_suppressed() {
        let mut d = Debouncer::new(Duration::from_millis(400));
        let a = PresenceView {
            details: "A".into(),
            state: "s".into(),
            large_image: "x".into(),
            large_text: "".into(),
            start_timestamp_unix: None,
        };
        let b = PresenceView {
            details: "B".into(),
            state: "s".into(),
            large_image: "x".into(),
            large_text: "".into(),
            start_timestamp_unix: None,
        };
        assert!(d.should_push(&a, true));
        assert!(!d.should_push(&b, false));
        std::thread::sleep(Duration::from_millis(450));
        assert!(d.should_push(&b, false));
    }

    #[test]
    fn debouncer_force_always_pushes() {
        let mut d = Debouncer::new(Duration::from_secs(60));
        let v = PresenceView {
            details: "A".into(),
            state: "s".into(),
            large_image: "x".into(),
            large_text: "".into(),
            start_timestamp_unix: None,
        };
        assert!(d.should_push(&v, true));
        assert!(d.should_push(&v, true));
    }

    #[test]
    fn select_active_agent_priority_and_pid_tiebreak() {
        let r10 = rule("a", 10);
        let r20 = rule("b", 20);
        let out = select_active_agent(vec![(r20.clone(), 100), (r10.clone(), 200)]);
        assert_eq!(out.as_ref().map(|(a, _)| a.id.as_str()), Some("a"));

        let out2 = select_active_agent(vec![(r10.clone(), 50), (r10, 30)]);
        assert_eq!(out2.map(|(_, pid)| pid), Some(30));
    }

    #[test]
    fn select_active_agent_empty() {
        assert!(select_active_agent(vec![]).is_none());
    }

    #[test]
    fn build_presence_view_agent_and_idle() {
        let cfg = sample_config();
        let agent = ActiveAgent {
            id: "x".into(),
            label: "My Agent".into(),
            large_image: "img".into(),
        };
        let v = build_presence_view(
            &cfg,
            Some(&agent),
            Some("com.microsoft.VSCode"),
            Some(99),
            Some("proj"),
        );
        assert_eq!(v.details, "My Agent");
        assert_eq!(v.state, "In VS Code · proj");
        assert_eq!(v.large_image, "img");
        assert_eq!(v.start_timestamp_unix, Some(99));

        let idle = build_presence_view(&cfg, None, None, None, None);
        assert_eq!(idle.details, "Idle");
        assert_eq!(idle.state, "macOS · no agent CLI detected");
        assert_eq!(idle.large_image, cfg.discord.large_image);
        assert!(idle.start_timestamp_unix.is_none());
    }

    #[test]
    fn host_label_known_and_jetbrains_fallback() {
        assert_eq!(host_label_for_bundle("com.microsoft.VSCode"), "VS Code");
        assert_eq!(host_label_for_bundle("com.jetbrains.pycharm"), "PyCharm");
        assert_eq!(
            host_label_for_bundle("com.jetbrains.unknownide"),
            "JetBrains"
        );
        assert_eq!(
            host_label_for_bundle("com.example.unknown"),
            "com.example.unknown"
        );
    }

    #[test]
    fn host_bundle_labels_include_hyper_tabby_wezterm() {
        assert!(HOST_BUNDLE_LABELS
            .iter()
            .any(|(id, _)| *id == "co.zeit.hyper"));
        assert!(HOST_BUNDLE_LABELS
            .iter()
            .any(|(id, _)| *id == "com.github.wez.wezterm"));
    }

    #[test]
    fn process_matches_rule_name_and_argv_case_insensitive() {
        let r = AgentRule {
            id: "t".into(),
            label: None,
            process_names: vec!["node".into()],
            argv_substrings: vec!["CODEX".into()],
            large_image: None,
            priority: 0,
        };
        assert!(process_matches_rule(
            "NODE",
            &[OsStr::new("node"), OsStr::new("--codex")],
            &r
        ));
        assert!(!process_matches_rule("ruby", &[OsStr::new("ruby")], &r));
    }

    #[test]
    fn process_matches_rule_empty_argv_substrings() {
        let r = AgentRule {
            id: "t".into(),
            label: None,
            process_names: vec!["foo".into()],
            argv_substrings: vec![],
            large_image: None,
            priority: 0,
        };
        let empty: &[&OsStr] = &[];
        assert!(process_matches_rule("foo", empty, &r));
    }

    #[test]
    fn process_matches_rule_argv0_basename_wrapped_cli() {
        let r = AgentRule {
            id: "codex".into(),
            label: None,
            process_names: vec!["codex".into()],
            argv_substrings: vec![],
            large_image: None,
            priority: 0,
        };
        assert!(process_matches_rule(
            "node",
            &[OsStr::new("/usr/local/bin/codex")],
            &r
        ));
    }

    #[test]
    fn agent_rule_deserializes_large_image_and_priority() {
        let toml_str = r#"
            [discord]
            client_id = "123"

            [[agents]]
            id = "test_agent"
            process_names = ["test"]
            large_image = "test_icon"
            priority = 7
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse failed");
        let rule = &cfg.agents[0];
        assert_eq!(rule.large_image.as_deref(), Some("test_icon"));
        assert_eq!(rule.priority, 7);
    }

    #[test]
    fn discord_section_deserializes_large_image_and_large_text() {
        let toml_str = r#"
            [discord]
            client_id = "123"
            large_image = "idle_icon"
            large_text = "Idle"

            [[agents]]
            id = "a"
            process_names = ["a"]
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse failed");
        assert_eq!(cfg.discord.large_image, "idle_icon");
        assert_eq!(cfg.discord.large_text, "Idle");
    }
}
