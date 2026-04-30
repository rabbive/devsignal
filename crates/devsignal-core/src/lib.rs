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
    #[serde(default)]
    pub platforms: PlatformsConfig,
    #[serde(default)]
    pub rules: Vec<PresenceRule>,
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
    /// Fallback small image key used during idle mode (optional).
    #[serde(default)]
    pub small_image: Option<String>,
    /// Fallback small image tooltip used during idle mode (optional).
    #[serde(default)]
    pub small_text: Option<String>,
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
    /// Discord asset key for the small (corner) image for this agent.
    #[serde(default)]
    pub small_image: Option<String>,
    /// Tooltip shown on hover over the small image.
    #[serde(default)]
    pub small_text: Option<String>,
    /// Up to 2 clickable buttons shown in the Discord presence panel.
    #[serde(default)]
    pub buttons: Vec<ButtonConfig>,
}

/// A Discord Rich Presence button (label + URL). Maximum 2 per presence payload.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ButtonConfig {
    /// Displayed on the button in Discord (1–32 characters).
    pub label: String,
    /// URL opened when the button is clicked (1–512 characters).
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PlatformsConfig {
    #[serde(default)]
    pub disabled_hosts: Vec<String>,
    #[serde(default)]
    pub disabled_agents: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PresenceRule {
    pub name: String,
    #[serde(default)]
    pub when: RuleWhen,
    #[serde(default)]
    pub then: RuleThen,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RuleWhen {
    #[serde(default)]
    pub host_bundle_ids: Vec<String>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
    #[serde(default)]
    pub active_only: bool,
    #[serde(default)]
    pub idle_only: bool,
    #[serde(default)]
    pub project_basenames: Vec<String>,
    #[serde(default)]
    pub time: Option<TimeWindow>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RuleThen {
    #[serde(default)]
    pub hide_host: bool,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimeWindow {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Copy)]
pub struct RuleContext<'a> {
    pub host_bundle_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub cwd_basename: Option<&'a str>,
    pub active: bool,
    pub local_minutes: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresencePolicyOverride {
    pub matched_rule_name: Option<String>,
    pub hide_host: bool,
    pub state: Option<String>,
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
        // Prefer the conventional dot-config path to match repo docs/scripts:
        // `~/.config/devsignal/config.toml`.
        if let Ok(home) = std::env::var("HOME") {
            let p = std::path::PathBuf::from(home)
                .join(".config")
                .join("devsignal")
                .join("config.toml");
            return p;
        }
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
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    pub buttons: Vec<ButtonConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresenceView {
    pub details: String,
    pub state: String,
    pub large_image: String,
    pub large_text: String,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    pub buttons: Vec<ButtonConfig>,
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

fn contains_ignore_ascii_case(items: &[String], needle: &str) -> bool {
    items.iter().any(|item| item.eq_ignore_ascii_case(needle))
}

pub fn host_allowed(cfg: &Config, bundle_id: Option<&str>) -> bool {
    bundle_id.is_none_or(|id| !contains_ignore_ascii_case(&cfg.platforms.disabled_hosts, id))
}

pub fn agent_allowed(cfg: &Config, agent_id: Option<&str>) -> bool {
    agent_id.is_none_or(|id| !contains_ignore_ascii_case(&cfg.platforms.disabled_agents, id))
}

fn parse_hhmm_minutes(s: &str) -> Option<u16> {
    let (hh, mm) = s.split_once(':')?;
    let hour: u16 = hh.parse().ok()?;
    let minute: u16 = mm.parse().ok()?;
    if hour < 24 && minute < 60 {
        Some(hour * 60 + minute)
    } else {
        None
    }
}

impl TimeWindow {
    pub fn matches_minutes(&self, minutes: u16) -> bool {
        let Some(start) = parse_hhmm_minutes(&self.start) else {
            return false;
        };
        let Some(end) = parse_hhmm_minutes(&self.end) else {
            return false;
        };
        if start <= end {
            minutes >= start && minutes <= end
        } else {
            minutes >= start || minutes <= end
        }
    }
}

impl RuleWhen {
    fn matches(&self, ctx: &RuleContext<'_>) -> bool {
        if self.active_only && !ctx.active {
            return false;
        }
        if self.idle_only && ctx.active {
            return false;
        }
        if !self.host_bundle_ids.is_empty()
            && !ctx
                .host_bundle_id
                .is_some_and(|id| contains_ignore_ascii_case(&self.host_bundle_ids, id))
        {
            return false;
        }
        if !self.agent_ids.is_empty()
            && !ctx
                .agent_id
                .is_some_and(|id| contains_ignore_ascii_case(&self.agent_ids, id))
        {
            return false;
        }
        if !self.project_basenames.is_empty()
            && !ctx
                .cwd_basename
                .is_some_and(|name| contains_ignore_ascii_case(&self.project_basenames, name))
        {
            return false;
        }
        if let Some(window) = &self.time {
            let Some(minutes) = ctx.local_minutes else {
                return false;
            };
            if !window.matches_minutes(minutes) {
                return false;
            }
        }
        true
    }
}

pub fn apply_rules(cfg: &Config, ctx: &RuleContext<'_>) -> PresencePolicyOverride {
    for rule in &cfg.rules {
        if rule.when.matches(ctx) {
            return PresencePolicyOverride {
                matched_rule_name: Some(rule.name.clone()),
                hide_host: rule.then.hide_host,
                state: rule.then.state.clone(),
            };
        }
    }
    PresencePolicyOverride::default()
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
        small_image: rule.small_image.clone(),
        small_text: rule.small_text.clone(),
        buttons: rule.buttons.clone(),
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
            small_image: a.small_image.clone(),
            small_text: a.small_text.clone(),
            buttons: a.buttons.clone(),
            start_timestamp_unix: session_start_unix,
        },
        None => PresenceView {
            details: "Idle".to_string(),
            state: format!("{host} · no agent CLI detected"),
            large_image: cfg.discord.large_image.clone(),
            large_text: cfg.discord.large_text.clone(),
            small_image: cfg.discord.small_image.clone(),
            small_text: cfg.discord.small_text.clone(),
            buttons: vec![],
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
                small_image: None,
                small_text: None,
            },
            agents: vec![],
            platforms: PlatformsConfig::default(),
            rules: vec![],
        }
    }

    #[test]
    fn platform_config_disables_hosts_and_agents_by_id() {
        let mut cfg = sample_config();
        cfg.platforms.disabled_hosts = vec!["com.apple.Terminal".into()];
        cfg.platforms.disabled_agents = vec!["opencode".into()];

        assert!(!host_allowed(&cfg, Some("com.apple.Terminal")));
        assert!(host_allowed(&cfg, Some("com.microsoft.VSCode")));
        assert!(host_allowed(&cfg, None));
        assert!(!agent_allowed(&cfg, Some("opencode")));
        assert!(agent_allowed(&cfg, Some("claude_code")));
        assert!(agent_allowed(&cfg, None));
    }

    #[test]
    fn rule_time_window_matches_same_day_and_overnight() {
        let day = TimeWindow {
            start: "09:00".into(),
            end: "17:00".into(),
        };
        assert!(day.matches_minutes(9 * 60));
        assert!(day.matches_minutes(12 * 60));
        assert!(!day.matches_minutes(18 * 60));

        let overnight = TimeWindow {
            start: "22:00".into(),
            end: "06:00".into(),
        };
        assert!(overnight.matches_minutes(23 * 60));
        assert!(overnight.matches_minutes(2 * 60));
        assert!(!overnight.matches_minutes(12 * 60));
    }

    #[test]
    fn apply_rules_returns_first_matching_override() {
        let mut cfg = sample_config();
        cfg.rules = vec![
            PresenceRule {
                name: "terminal_focus".into(),
                when: RuleWhen {
                    host_bundle_ids: vec!["com.apple.Terminal".into()],
                    agent_ids: vec!["claude_code".into()],
                    active_only: true,
                    idle_only: false,
                    project_basenames: vec![],
                    time: None,
                },
                then: RuleThen {
                    hide_host: true,
                    state: Some("Deep work".into()),
                },
            },
            PresenceRule {
                name: "later_rule_ignored".into(),
                when: RuleWhen::default(),
                then: RuleThen {
                    hide_host: false,
                    state: Some("Should not win".into()),
                },
            },
        ];

        let ctx = RuleContext {
            host_bundle_id: Some("com.apple.Terminal"),
            agent_id: Some("claude_code"),
            cwd_basename: Some("devsignal"),
            active: true,
            local_minutes: Some(12 * 60),
        };

        let out = apply_rules(&cfg, &ctx);
        assert_eq!(out.matched_rule_name.as_deref(), Some("terminal_focus"));
        assert!(out.hide_host);
        assert_eq!(out.state.as_deref(), Some("Deep work"));
    }

    #[test]
    fn default_path_prefers_home_dot_config() {
        // This test asserts path shape rather than exact HOME contents.
        let p = Config::default_path();
        let s = p.to_string_lossy();
        assert!(s.contains("/.config/devsignal/config.toml"));
    }

    fn rule(id: &str, priority: i32) -> AgentRule {
        AgentRule {
            id: id.to_string(),
            label: None,
            process_names: vec![],
            argv_substrings: vec![],
            large_image: None,
            priority,
            small_image: None,
            small_text: None,
            buttons: vec![],
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
            small_image: None,
            small_text: None,
            buttons: vec![],
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
            small_image: None,
            small_text: None,
            buttons: vec![],
            start_timestamp_unix: None,
        };
        let b = PresenceView {
            details: "B".into(),
            state: "s".into(),
            large_image: "x".into(),
            large_text: "".into(),
            small_image: None,
            small_text: None,
            buttons: vec![],
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
            small_image: None,
            small_text: None,
            buttons: vec![],
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
            small_image: None,
            small_text: None,
            buttons: vec![],
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
        assert!(v.small_image.is_none());
        assert!(v.buttons.is_empty());

        let idle = build_presence_view(&cfg, None, None, None, None);
        assert_eq!(idle.details, "Idle");
        assert_eq!(idle.state, "macOS · no agent CLI detected");
        assert_eq!(idle.large_image, cfg.discord.large_image);
        assert!(idle.start_timestamp_unix.is_none());
        assert!(idle.buttons.is_empty());
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
            small_image: None,
            small_text: None,
            buttons: vec![],
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
            small_image: None,
            small_text: None,
            buttons: vec![],
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
            small_image: None,
            small_text: None,
            buttons: vec![],
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
    fn agent_rule_deserializes_small_image_and_buttons() {
        let toml_str = r#"
            [discord]
            client_id = "123"

            [[agents]]
            id = "test_agent"
            process_names = ["test"]
            small_image = "test_icon"
            small_text = "Test v1"

            [[agents.buttons]]
            label = "Docs"
            url = "https://example.com"
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse failed");
        let rule = &cfg.agents[0];
        assert_eq!(rule.small_image.as_deref(), Some("test_icon"));
        assert_eq!(rule.small_text.as_deref(), Some("Test v1"));
        assert_eq!(rule.buttons.len(), 1);
        assert_eq!(rule.buttons[0].label, "Docs");
        assert_eq!(rule.buttons[0].url, "https://example.com");
    }

    #[test]
    fn discord_section_deserializes_small_image_defaults() {
        let toml_str = r#"
            [discord]
            client_id = "123"
            small_image = "idle_icon"
            small_text = "Idle"

            [[agents]]
            id = "a"
            process_names = ["a"]
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse failed");
        assert_eq!(cfg.discord.small_image.as_deref(), Some("idle_icon"));
        assert_eq!(cfg.discord.small_text.as_deref(), Some("Idle"));
    }

    #[test]
    fn build_presence_view_propagates_small_image_and_buttons() {
        let cfg = sample_config();
        let agent = ActiveAgent {
            id: "claude".into(),
            label: "Claude Code".into(),
            large_image: "claude".into(),
            small_image: Some("devsignal".into()),
            small_text: Some("devsignal v0.2".into()),
            buttons: vec![ButtonConfig {
                label: "Docs".into(),
                url: "https://claude.ai/code".into(),
            }],
        };
        let v = build_presence_view(&cfg, Some(&agent), None, None, None);
        assert_eq!(v.small_image.as_deref(), Some("devsignal"));
        assert_eq!(v.small_text.as_deref(), Some("devsignal v0.2"));
        assert_eq!(v.buttons.len(), 1);
        assert_eq!(v.buttons[0].label, "Docs");
        assert_eq!(v.buttons[0].url, "https://claude.ai/code");
    }

    #[test]
    fn build_presence_view_idle_uses_discord_section_small_image() {
        let mut cfg = sample_config();
        cfg.discord.small_image = Some("idle_icon".into());
        cfg.discord.small_text = Some("No agent".into());
        let v = build_presence_view(&cfg, None, None, None, None);
        assert_eq!(v.small_image.as_deref(), Some("idle_icon"));
        assert_eq!(v.small_text.as_deref(), Some("No agent"));
        assert!(v.buttons.is_empty());
    }

    #[test]
    fn build_presence_view_no_small_image_returns_none() {
        let cfg = sample_config();
        let agent = ActiveAgent {
            id: "x".into(),
            label: "X".into(),
            large_image: "x".into(),
            small_image: None,
            small_text: None,
            buttons: vec![],
        };
        let v = build_presence_view(&cfg, Some(&agent), None, None, None);
        assert!(v.small_image.is_none());
        assert!(v.small_text.is_none());
        assert!(v.buttons.is_empty());
    }
}
