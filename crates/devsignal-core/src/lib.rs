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

/// Discord's documented limits for the activity fields devsignal populates. Exceeding any of them
/// makes Discord reject the whole payload, which surfaces only as a `warn!` — so these are enforced
/// at config-load time instead.
pub const BUTTON_LABEL_MAX_CHARS: usize = 32;
pub const BUTTON_URL_MAX_CHARS: usize = 512;
pub const MAX_BUTTONS: usize = 2;

/// A Discord Rich Presence button (label + URL). Maximum 2 per presence payload.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ButtonConfig {
    /// Displayed on the button in Discord (1–32 characters).
    pub label: String,
    /// URL opened when the button is clicked (1–512 characters).
    pub url: String,
}

impl ButtonConfig {
    pub fn validate(&self) -> Result<()> {
        let label = self.label.trim();
        anyhow::ensure!(!label.is_empty(), "button label must not be empty");
        // Discord counts characters, not bytes.
        let label_len = self.label.chars().count();
        anyhow::ensure!(
            label_len <= BUTTON_LABEL_MAX_CHARS,
            "button label {:?} is {} characters; Discord allows at most {}",
            self.label,
            label_len,
            BUTTON_LABEL_MAX_CHARS
        );

        anyhow::ensure!(
            !self.url.trim().is_empty(),
            "button {:?} must have a url",
            self.label
        );
        let url_len = self.url.chars().count();
        anyhow::ensure!(
            url_len <= BUTTON_URL_MAX_CHARS,
            "button {:?} url is {} characters; Discord allows at most {}",
            self.label,
            url_len,
            BUTTON_URL_MAX_CHARS
        );
        anyhow::ensure!(
            self.url.starts_with("http://") || self.url.starts_with("https://"),
            "button {:?} url must start with http:// or https:// (got {:?})",
            self.label,
            self.url
        );
        Ok(())
    }
}

/// A Discord Application ID is an opaque numeric snowflake. Rejecting non-numeric values at
/// config-load time avoids a misleading "is Discord running?" failure at IPC connect.
pub fn parse_numeric_id(raw: &str) -> Result<String> {
    let s = raw.trim();
    anyhow::ensure!(!s.is_empty(), "Discord Application ID cannot be empty");
    anyhow::ensure!(
        s.chars().all(|c| c.is_ascii_digit()),
        "Discord Application ID must be numeric (got {s:?}); \
         copy it from https://discord.com/developers/applications"
    );
    Ok(s.to_string())
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

/// Built-in agent CLI presets — the single source of truth for `devsignal init` and the shipped
/// `config.example.toml`. Both used to hardcode their own copy, which drifts.
///
/// **Only agents whose process names have been confirmed on a real machine live here.** A default
/// that silently never matches is worse than no default: the daemon looks broken and the user has no
/// way to tell a wrong `process_names` from "devsignal is not working". Ten further presets ship as
/// opt-in snippets in `docs/community-presets.md`, to be confirmed with `devsignal detect` and added
/// with `devsignal agents add`.
///
/// `process_names` matches the process name **or** the basename of `argv[0]`, case-insensitively, so
/// Node- and Python-wrapped CLIs are covered without extra entries.
///
/// `large_image` is a Discord art-asset **key**, not a URL — a key you have not uploaded in the
/// Developer Portal renders blank. `devsignal init` prints the full list of keys to upload.
/// Priorities are spaced by 10 so you can slot custom rules between presets.
pub fn agent_presets() -> Vec<AgentRule> {
    fn preset(
        id: &str,
        label: &str,
        process_names: &[&str],
        priority: i32,
        button: Option<(&str, &str)>,
    ) -> AgentRule {
        AgentRule {
            id: id.to_string(),
            label: Some(label.to_string()),
            process_names: process_names.iter().map(|s| s.to_string()).collect(),
            argv_substrings: vec![],
            large_image: Some(id.to_string()),
            priority,
            small_image: Some("devsignal".to_string()),
            small_text: Some("devsignal".to_string()),
            buttons: button
                .map(|(label, url)| {
                    vec![ButtonConfig {
                        label: label.to_string(),
                        url: url.to_string(),
                    }]
                })
                .unwrap_or_default(),
        }
    }

    vec![
        preset(
            "claude_code",
            "Claude Code",
            &["claude", "claude-code"],
            10,
            Some(("Claude Code Docs", "https://claude.ai/code")),
        ),
        preset(
            "codex",
            "Codex",
            &["codex"],
            20,
            Some(("Codex on GitHub", "https://github.com/openai/codex")),
        ),
        preset(
            "opencode",
            "OpenCode",
            &["opencode"],
            30,
            Some(("OpenCode Docs", "https://opencode.ai")),
        ),
    ]
}

/// Every distinct Discord art-asset key the presets reference, for the `init` wizard's upload list.
pub fn preset_asset_keys() -> Vec<String> {
    let mut keys = vec!["devsignal".to_string()];
    for agent in agent_presets() {
        if let Some(key) = agent.large_image {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
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
        parse_numeric_id(&self.discord.client_id).context("discord.client_id")?;
        anyhow::ensure!(
            !self.agents.is_empty(),
            "at least one [[agents]] entry is required"
        );

        let mut seen_ids: Vec<&str> = Vec::new();
        for agent in &self.agents {
            agent
                .validate()
                .with_context(|| format!("[[agents]] id={}", agent.id))?;
            anyhow::ensure!(
                !seen_ids.contains(&agent.id.as_str()),
                "duplicate [[agents]] id: {}",
                agent.id
            );
            seen_ids.push(&agent.id);
        }

        let mut seen_rules: Vec<&str> = Vec::new();
        for rule in &self.rules {
            rule.validate()
                .with_context(|| format!("[[rules]] name={}", rule.name))?;
            anyhow::ensure!(
                !seen_rules.contains(&rule.name.as_str()),
                "duplicate [[rules]] name: {}",
                rule.name
            );
            seen_rules.push(&rule.name);
        }
        Ok(())
    }
}

impl AgentRule {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.id.trim().is_empty(), "agent id must not be empty");
        // `process_matches_rule` requires a `process_names` hit before it even looks at
        // `argv_substrings`, so an entry without one can never match anything.
        anyhow::ensure!(
            !self.process_names.is_empty(),
            "agent {:?} has no process_names, so it can never match a process",
            self.id
        );
        anyhow::ensure!(
            self.buttons.len() <= MAX_BUTTONS,
            "agent {:?} declares {} buttons; Discord shows at most {}. \
             Remove the extras rather than relying on truncation.",
            self.id,
            self.buttons.len(),
            MAX_BUTTONS
        );
        for button in &self.buttons {
            button.validate()?;
        }
        Ok(())
    }
}

impl TimeWindow {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            parse_hhmm_minutes(&self.start).is_some(),
            "time window start {:?} is not HH:MM (24-hour)",
            self.start
        );
        anyhow::ensure!(
            parse_hhmm_minutes(&self.end).is_some(),
            "time window end {:?} is not HH:MM (24-hour)",
            self.end
        );
        Ok(())
    }
}

impl PresenceRule {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty(), "rule name must not be empty");
        // `apply_rules` returns on the first `when` match regardless of `then`, so a rule that
        // changes nothing would silently shadow every rule after it.
        anyhow::ensure!(
            self.then.hide_host || self.then.state.is_some(),
            "rule {:?} does nothing: set then.hide_host and/or then.state. \
             An empty `then` still matches and would block every later rule.",
            self.name
        );
        if let Some(state) = &self.then.state {
            anyhow::ensure!(
                !state.trim().is_empty(),
                "rule {:?} has an empty then.state",
                self.name
            );
        }
        anyhow::ensure!(
            !(self.when.active_only && self.when.idle_only),
            "rule {:?} sets both active_only and idle_only, so it can never match",
            self.name
        );
        if let Some(window) = &self.when.time {
            window
                .validate()
                .with_context(|| format!("rule {:?}", self.name))?;
        }
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

/// Discord's documented RPC rate limit: 5 activity updates per 20 seconds, per client.
pub const RATE_LIMIT_MAX_SENDS: usize = 5;
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(20);

/// What the daemon wants to tell Discord this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceAction<'a> {
    Set(&'a PresenceView),
    /// `CLEAR_ACTIVITY` — used by `idle_mode = "clear"` and on shutdown.
    Clear,
}

/// What the debouncer last told Discord, for equality-based deduplication.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LastSent {
    Set(PresenceView),
    Clear,
}

#[derive(Debug, Clone)]
pub struct Debouncer {
    min_interval: Duration,
    max_sends: usize,
    window: Duration,
    last_sent: Option<LastSent>,
    last_push: Option<Instant>,
    /// Timestamps of sends inside the current window, oldest first.
    recent: std::collections::VecDeque<Instant>,
}

impl Debouncer {
    pub fn new(min_interval: Duration) -> Self {
        Self::with_limits(min_interval, RATE_LIMIT_MAX_SENDS, RATE_LIMIT_WINDOW)
    }

    /// Same as [`Debouncer::new`] with the rate-limit window overridden, so tests need not sleep for
    /// 20 seconds to exercise it.
    pub fn with_limits(min_interval: Duration, max_sends: usize, window: Duration) -> Self {
        Self {
            min_interval,
            max_sends,
            window,
            last_sent: None,
            last_push: None,
            recent: std::collections::VecDeque::new(),
        }
    }

    /// Adjust the minimum interval in place. Used by config hot-reload: rebuilding the debouncer
    /// would discard the dedupe state and the rate-limit window along with it.
    pub fn set_min_interval(&mut self, min_interval: Duration) {
        self.min_interval = min_interval;
    }

    pub fn min_interval(&self) -> Duration {
        self.min_interval
    }

    fn prune(&mut self, now: Instant) {
        while let Some(&front) = self.recent.front() {
            if now.duration_since(front) >= self.window {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }

    /// True when the sliding window is full. Checked even for forced sends: `force` exists to keep
    /// agent transitions responsive, not to license unbounded IPC traffic.
    pub fn rate_limited(&mut self, now: Instant) -> bool {
        self.prune(now);
        self.recent.len() >= self.max_sends
    }

    fn record(&mut self, sent: LastSent, now: Instant) {
        self.last_sent = Some(sent);
        self.last_push = Some(now);
        self.recent.push_back(now);
    }

    /// Whether to send `action` to Discord now.
    ///
    /// `force` (an agent transition, or the first tick) skips the equality check and the
    /// `min_push_interval_secs` wait, but **not** the rate limit. Without that last part, an agent
    /// process that flaps in and out on alternate polls makes every tick a transition, and the daemon
    /// writes to Discord every `poll_interval_secs` indefinitely.
    pub fn should_send(&mut self, action: PresenceAction<'_>, force: bool) -> bool {
        let now = Instant::now();
        if self.rate_limited(now) {
            return false;
        }

        let next = match action {
            PresenceAction::Set(view) => LastSent::Set(view.clone()),
            PresenceAction::Clear => LastSent::Clear,
        };

        if force {
            self.record(next, now);
            return true;
        }
        if self.last_sent.as_ref() == Some(&next) {
            return false;
        }
        if let Some(t) = self.last_push {
            if now.duration_since(t) < self.min_interval {
                return false;
            }
        }
        self.record(next, now);
        true
    }

    /// Convenience wrapper for the common case.
    pub fn should_push(&mut self, next: &PresenceView, force: bool) -> bool {
        self.should_send(PresenceAction::Set(next), force)
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

    /// A config that passes `validate()`, for tests that mutate one thing and expect a rejection.
    fn valid_config() -> Config {
        let mut cfg = sample_config();
        cfg.agents = vec![AgentRule {
            id: "claude_code".into(),
            label: None,
            process_names: vec!["claude".into()],
            argv_substrings: vec![],
            large_image: None,
            priority: 10,
            small_image: None,
            small_text: None,
            buttons: vec![],
        }];
        cfg
    }

    fn err_of(cfg: &Config) -> String {
        format!(
            "{:#}",
            cfg.validate().expect_err("expected validation error")
        )
    }

    #[test]
    fn valid_config_passes_validation() {
        valid_config().validate().expect("should be valid");
    }

    #[test]
    fn parse_numeric_id_rejects_non_digits() {
        assert!(parse_numeric_id("abc").is_err());
        assert!(parse_numeric_id("123a").is_err());
        assert!(parse_numeric_id("").is_err());
        assert_eq!(parse_numeric_id("123").unwrap(), "123");
        assert_eq!(parse_numeric_id("  123  ").unwrap(), "123");
    }

    #[test]
    fn validate_rejects_non_numeric_client_id() {
        let mut cfg = valid_config();
        // The value config.example.toml ships with, so this is the first thing a new user hits.
        cfg.discord.client_id = "YOUR_DISCORD_APPLICATION_ID".into();
        let msg = err_of(&cfg);
        assert!(msg.contains("client_id"), "got {msg}");
        assert!(msg.contains("numeric"), "got {msg}");
    }

    #[test]
    fn validate_requires_at_least_one_agent() {
        let mut cfg = valid_config();
        cfg.agents.clear();
        assert!(err_of(&cfg).contains("[[agents]]"));
    }

    #[test]
    fn validate_rejects_agent_without_process_names() {
        let mut cfg = valid_config();
        cfg.agents[0].process_names.clear();
        let msg = err_of(&cfg);
        assert!(msg.contains("process_names"), "got {msg}");
        assert!(msg.contains("never match"), "got {msg}");
    }

    #[test]
    fn validate_rejects_duplicate_agent_ids() {
        let mut cfg = valid_config();
        let dup = cfg.agents[0].clone();
        cfg.agents.push(dup);
        assert!(err_of(&cfg).contains("duplicate"));
    }

    #[test]
    fn validate_rejects_more_than_two_buttons() {
        let mut cfg = valid_config();
        cfg.agents[0].buttons = (0..3)
            .map(|i| ButtonConfig {
                label: format!("b{i}"),
                url: "https://example.com".into(),
            })
            .collect();
        let msg = err_of(&cfg);
        assert!(msg.contains("3 buttons"), "got {msg}");
        assert!(msg.contains("truncation"), "got {msg}");
    }

    #[test]
    fn validate_rejects_overlong_button_label() {
        let mut cfg = valid_config();
        cfg.agents[0].buttons = vec![ButtonConfig {
            label: "x".repeat(BUTTON_LABEL_MAX_CHARS + 1),
            url: "https://example.com".into(),
        }];
        assert!(err_of(&cfg).contains("characters"));

        // Exactly at the limit is fine.
        let mut ok = valid_config();
        ok.agents[0].buttons = vec![ButtonConfig {
            label: "x".repeat(BUTTON_LABEL_MAX_CHARS),
            url: "https://example.com".into(),
        }];
        ok.validate().expect("boundary label should be accepted");
    }

    #[test]
    fn button_label_limit_counts_characters_not_bytes() {
        // 32 multi-byte characters is 96 bytes but a legal Discord label.
        let label = "é".repeat(BUTTON_LABEL_MAX_CHARS);
        assert!(label.len() > BUTTON_LABEL_MAX_CHARS);
        let mut cfg = valid_config();
        cfg.agents[0].buttons = vec![ButtonConfig {
            label,
            url: "https://example.com".into(),
        }];
        cfg.validate()
            .expect("32 chars should pass regardless of byte length");
    }

    #[test]
    fn validate_rejects_bad_button_urls() {
        for bad in [
            "",
            "example.com",
            "ftp://example.com",
            "javascript:alert(1)",
        ] {
            let mut cfg = valid_config();
            cfg.agents[0].buttons = vec![ButtonConfig {
                label: "Docs".into(),
                url: bad.into(),
            }];
            assert!(
                cfg.validate().is_err(),
                "url {bad:?} should have been rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_overlong_button_url() {
        let mut cfg = valid_config();
        let long = format!("https://example.com/{}", "a".repeat(BUTTON_URL_MAX_CHARS));
        cfg.agents[0].buttons = vec![ButtonConfig {
            label: "Docs".into(),
            url: long,
        }];
        assert!(err_of(&cfg).contains("at most"));
    }

    fn rule_named(name: &str) -> PresenceRule {
        PresenceRule {
            name: name.into(),
            when: RuleWhen::default(),
            then: RuleThen {
                hide_host: true,
                state: None,
            },
        }
    }

    #[test]
    fn validate_rejects_rule_with_empty_then() {
        let mut cfg = valid_config();
        cfg.rules = vec![PresenceRule {
            name: "noop".into(),
            when: RuleWhen::default(),
            then: RuleThen::default(),
        }];
        let msg = err_of(&cfg);
        assert!(msg.contains("does nothing"), "got {msg}");
        assert!(msg.contains("block every later rule"), "got {msg}");
    }

    #[test]
    fn validate_rejects_rule_that_is_both_active_and_idle_only() {
        let mut cfg = valid_config();
        let mut rule = rule_named("impossible");
        rule.when.active_only = true;
        rule.when.idle_only = true;
        cfg.rules = vec![rule];
        assert!(err_of(&cfg).contains("never match"));
    }

    #[test]
    fn validate_rejects_unparseable_time_windows() {
        // Each of these previously produced a rule that silently never matched.
        for (start, end) in [
            ("9", "17"),
            ("0900", "1700"),
            ("25:00", "26:00"),
            ("09:60", "17:00"),
            ("", ""),
        ] {
            let mut cfg = valid_config();
            let mut rule = rule_named("window");
            rule.when.time = Some(TimeWindow {
                start: start.into(),
                end: end.into(),
            });
            cfg.rules = vec![rule];
            assert!(
                cfg.validate().is_err(),
                "time window {start:?}-{end:?} should have been rejected"
            );
        }
    }

    #[test]
    fn validate_accepts_overnight_time_window() {
        let mut cfg = valid_config();
        let mut rule = rule_named("after_hours");
        rule.when.time = Some(TimeWindow {
            start: "22:00".into(),
            end: "06:00".into(),
        });
        cfg.rules = vec![rule];
        cfg.validate().expect("overnight windows are legal");
    }

    #[test]
    fn validate_rejects_duplicate_rule_names_and_empty_state() {
        let mut cfg = valid_config();
        cfg.rules = vec![rule_named("dup"), rule_named("dup")];
        assert!(err_of(&cfg).contains("duplicate"));

        let mut cfg2 = valid_config();
        cfg2.rules = vec![PresenceRule {
            name: "blank".into(),
            when: RuleWhen::default(),
            then: RuleThen {
                hide_host: false,
                state: Some("   ".into()),
            },
        }];
        assert!(err_of(&cfg2).contains("empty then.state"));
    }

    #[test]
    fn validation_errors_name_the_offending_entry() {
        let mut cfg = valid_config();
        cfg.agents[0].id = "codex".into();
        cfg.agents[0].buttons = vec![ButtonConfig {
            label: "Docs".into(),
            url: "nope".into(),
        }];
        let msg = err_of(&cfg);
        assert!(
            msg.contains("codex"),
            "error should locate the agent: {msg}"
        );
    }

    #[test]
    fn every_preset_passes_validation() {
        let mut cfg = sample_config();
        cfg.agents = agent_presets();
        cfg.validate()
            .expect("shipped presets must be a valid config");
    }

    /// The shipped table is deliberately limited to agents whose process names have been confirmed on
    /// a real machine. Adding one here is a claim that it was verified with `devsignal detect`;
    /// unconfirmed agents belong in `docs/community-presets.md`.
    #[test]
    fn shipped_presets_are_the_confirmed_set() {
        let ids: Vec<String> = agent_presets().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, vec!["claude_code", "codex", "opencode"]);
    }

    #[test]
    fn presets_have_unique_ids_and_priorities() {
        let presets = agent_presets();
        assert!(!presets.is_empty(), "at least one preset must ship");

        let mut ids: Vec<&str> = presets.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "preset ids must be unique");

        let mut prios: Vec<i32> = presets.iter().map(|a| a.priority).collect();
        prios.sort_unstable();
        let before = prios.len();
        prios.dedup();
        assert_eq!(
            before,
            prios.len(),
            "preset priorities must be unique so agent selection is deterministic"
        );
    }

    #[test]
    fn presets_are_well_formed() {
        for agent in agent_presets() {
            assert!(
                !agent.process_names.is_empty(),
                "{} has no process_names",
                agent.id
            );
            assert!(agent.label.is_some(), "{} has no label", agent.id);
            // Lowercase, underscore-separated ids keep `agents disable <id>` predictable.
            assert!(
                agent
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{} is not a lowercase snake_case id",
                agent.id
            );
            for name in &agent.process_names {
                assert_eq!(
                    name.trim(),
                    name.as_str(),
                    "{}: process name {name:?} has surrounding whitespace",
                    agent.id
                );
                assert!(
                    name.len() >= 3,
                    "{}: process name {name:?} is short enough to collide with unrelated binaries",
                    agent.id
                );
            }
        }
    }

    #[test]
    fn preset_asset_keys_include_devsignal_and_are_unique() {
        let keys = preset_asset_keys();
        assert_eq!(keys.first().map(String::as_str), Some("devsignal"));
        let mut sorted = keys.clone();
        sorted.sort();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "asset keys must be unique");
        for agent in agent_presets() {
            if let Some(key) = &agent.large_image {
                assert!(keys.contains(key), "{key} missing from the upload list");
            }
        }
    }

    /// Drift guard: the annotated example config is hand-written, but it must not fall behind the
    /// preset table the wizard uses. Historically these were two independent hardcoded copies.
    #[test]
    fn config_example_covers_every_preset_id() {
        let raw = include_str!("../../../config.example.toml");
        let cfg: Config = toml::from_str(raw).expect("config.example.toml must parse");
        cfg.validate()
            .expect_err("the shipped example has a placeholder client_id, so it must not validate");

        let example_ids: Vec<&str> = cfg.agents.iter().map(|a| a.id.as_str()).collect();
        for preset in agent_presets() {
            assert!(
                example_ids.contains(&preset.id.as_str()),
                "config.example.toml is missing preset {:?}",
                preset.id
            );
        }
        for id in &example_ids {
            assert!(
                agent_presets().iter().any(|p| p.id == *id),
                "config.example.toml has agent {id:?} with no matching preset"
            );
        }
    }

    /// Extract every ```toml fenced block from a markdown document.
    fn toml_fences(markdown: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut current: Option<String> = None;
        for line in markdown.lines() {
            match (&mut current, line.trim_start()) {
                (None, l) if l.starts_with("```toml") => current = Some(String::new()),
                (Some(_), l) if l.starts_with("```") => {
                    out.push(current.take().expect("in a fence"));
                }
                (Some(buf), _) => {
                    buf.push_str(line);
                    buf.push('\n');
                }
                _ => {}
            }
        }
        out
    }

    /// The community presets are unverified *process names* — that is the user's to confirm. A
    /// snippet that does not even parse, or that violates the config rules, is ours. This stops the
    /// doc from rotting as validation tightens.
    #[test]
    fn community_preset_snippets_are_valid_config() {
        let doc = include_str!("../../../docs/community-presets.md");
        let fences = toml_fences(doc);
        assert!(
            fences.len() >= 10,
            "expected a snippet per community preset, found {}",
            fences.len()
        );

        let shipped: Vec<String> = agent_presets().into_iter().map(|a| a.id).collect();
        let mut seen_ids = Vec::new();

        for (idx, fence) in fences.iter().enumerate() {
            // Each fence is an [[agents]] fragment; give it the minimum surrounding config.
            let full = format!("[discord]\nclient_id = \"1\"\n\n{fence}");
            let cfg: Config = toml::from_str(&full)
                .unwrap_or_else(|e| panic!("snippet {idx} does not parse: {e}\n---\n{fence}"));
            cfg.validate().unwrap_or_else(|e| {
                panic!("snippet {idx} is not a valid config: {e:#}\n---\n{fence}")
            });

            for agent in cfg.agents {
                // A community preset must not collide with a shipped one, or pasting it in produces a
                // duplicate-id error the user did not cause.
                assert!(
                    !shipped.contains(&agent.id),
                    "snippet {idx} reuses shipped preset id {:?}",
                    agent.id
                );
                // Priorities must stay clear of the shipped band so ordering is predictable.
                assert!(
                    agent.priority >= 100,
                    "snippet {idx} ({}) uses priority {} — community presets start at 100 to avoid \
                     colliding with shipped presets",
                    agent.id,
                    agent.priority
                );
                seen_ids.push(agent.id);
            }
        }

        // The button example intentionally repeats an id; ignore duplicates from that section by
        // checking only that every documented agent id appears at least once.
        for expected in [
            "gemini_cli",
            "amp",
            "cursor_agent",
            "copilot_cli",
            "aider",
            "crush",
            "qwen_code",
            "droid",
            "cline",
            "goose",
        ] {
            assert!(
                seen_ids.iter().any(|id| id == expected),
                "docs/community-presets.md is missing a snippet for {expected}"
            );
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

    fn view_named(details: &str) -> PresenceView {
        PresenceView {
            details: details.into(),
            state: "s".into(),
            large_image: "x".into(),
            large_text: String::new(),
            small_image: None,
            small_text: None,
            buttons: vec![],
            start_timestamp_unix: None,
        }
    }

    /// The regression: `force` used to return `true` before any rate check, so a flapping agent
    /// produced one Discord write per poll tick forever.
    #[test]
    fn rate_limit_applies_even_to_forced_sends() {
        // 3 sends per long window, so the cap is hit deterministically without sleeping.
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 3, Duration::from_secs(60));

        for i in 0..3 {
            assert!(
                d.should_push(&view_named(&format!("v{i}")), true),
                "send {i} should be allowed"
            );
        }
        // Cap reached: further forced sends are refused despite `force`.
        assert!(!d.should_push(&view_named("v3"), true));
        assert!(!d.should_push(&view_named("v4"), true));
        // And a non-forced one too.
        assert!(!d.should_push(&view_named("v5"), false));
    }

    #[test]
    fn rate_limit_window_slides() {
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 2, Duration::from_millis(300));
        assert!(d.should_push(&view_named("a"), true));
        assert!(d.should_push(&view_named("b"), true));
        assert!(!d.should_push(&view_named("c"), true), "cap of 2 reached");

        // Once the window rolls past the earlier sends, capacity returns.
        std::thread::sleep(Duration::from_millis(350));
        assert!(d.should_push(&view_named("c"), true));
    }

    /// `idle_mode = "clear"` used to bypass the debouncer entirely — it was not merely exempt from
    /// the rate limit, it never consulted it.
    #[test]
    fn clears_go_through_the_same_limiter_as_sets() {
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 2, Duration::from_secs(60));
        assert!(d.should_send(PresenceAction::Clear, true));
        assert!(d.should_send(PresenceAction::Clear, true));
        assert!(
            !d.should_send(PresenceAction::Clear, true),
            "clears must consume rate-limit budget too"
        );
    }

    #[test]
    fn a_repeated_clear_is_deduplicated() {
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 10, Duration::from_secs(60));
        assert!(d.should_send(PresenceAction::Clear, false));
        assert!(
            !d.should_send(PresenceAction::Clear, false),
            "an unchanged clear should not be resent"
        );
    }

    /// Set-then-clear-then-set must not be deduplicated away: the actions differ.
    #[test]
    fn alternating_set_and_clear_are_distinct_payloads() {
        let mut d = Debouncer::with_limits(Duration::ZERO, 10, Duration::from_secs(60));
        let v = view_named("a");
        assert!(d.should_send(PresenceAction::Set(&v), false));
        assert!(d.should_send(PresenceAction::Clear, false));
        assert!(d.should_send(PresenceAction::Set(&v), false));
    }

    #[test]
    fn set_min_interval_preserves_dedupe_and_window_state() {
        let mut d = Debouncer::with_limits(Duration::from_secs(60), 5, Duration::from_secs(60));
        let v = view_named("a");
        assert!(d.should_push(&v, true));
        assert_eq!(d.min_interval(), Duration::from_secs(60));

        // Hot-reload lowering the interval must not forget what was already sent.
        d.set_min_interval(Duration::ZERO);
        assert_eq!(d.min_interval(), Duration::ZERO);
        assert!(
            !d.should_push(&v, false),
            "the identical payload should still be deduplicated after a reload"
        );
        assert!(d.should_push(&view_named("b"), false));
    }

    #[test]
    fn default_limits_match_discords_documented_rate() {
        assert_eq!(RATE_LIMIT_MAX_SENDS, 5);
        assert_eq!(RATE_LIMIT_WINDOW, Duration::from_secs(20));
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
