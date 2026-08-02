//! Core types, configuration, and presence snapshot building for `devsignal`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

/// Top-level config loaded from `~/.config/devsignal/config.toml` (or `--config`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    /// Which of Discord's three text lines carries what.
    #[serde(default)]
    pub presence: PresenceSection,
    /// How image values resolve: uploaded art-asset keys, or hosted PNG URLs.
    #[serde(default)]
    pub images: ImagesConfig,
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
#[serde(deny_unknown_fields)]
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

/// Which piece of information a visible presence line carries.
///
/// Discord's card is three lines: the **activity name**, then `details`, then `state`. The name
/// line defaults to the Discord *application's* name (`devsignal`), which is why the agent used to
/// land on line 2 — see [`PresenceSection`].
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceLine {
    /// The agent label, e.g. `Claude Code`; `Idle` when no agent is detected.
    Agent,
    /// The frontmost editor or terminal, e.g. `In Ghostty`.
    Host,
    /// The project basename. Requires `show_cwd_basename = true`.
    Project,
    /// [`PresenceSection::brand_text`], e.g. `devsignal`.
    Brand,
    /// Omit this line. For `name`, that means Discord falls back to the application name.
    #[default]
    Off,
}

impl PresenceLine {
    fn as_str(self) -> &'static str {
        match self {
            PresenceLine::Agent => "agent",
            PresenceLine::Host => "host",
            PresenceLine::Project => "project",
            PresenceLine::Brand => "brand",
            PresenceLine::Off => "off",
        }
    }
}

/// Line assignment for the presence card.
///
/// The defaults reproduce devsignal's original layout — `name` off (so Discord shows the
/// application name), agent in `details`, host in `state`:
///
/// ```text
/// devsignal            <- application name
/// Claude Code          <- details
/// In Ghostty           <- state
/// ```
///
/// Setting `name = "agent"`, `details = "host"`, `state = "brand"` puts the agent on top:
///
/// ```text
/// Claude Code
/// In Ghostty
/// devsignal
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PresenceSection {
    /// Line 1. `off` leaves Discord's default: the application's own name.
    #[serde(default)]
    pub name: PresenceLine,
    /// Line 2.
    #[serde(default = "default_details_line")]
    pub details: PresenceLine,
    /// Line 3.
    #[serde(default = "default_state_line")]
    pub state: PresenceLine,
    /// Text rendered by [`PresenceLine::Brand`].
    #[serde(default = "default_brand_text")]
    pub brand_text: String,
}

fn default_details_line() -> PresenceLine {
    PresenceLine::Agent
}

fn default_state_line() -> PresenceLine {
    PresenceLine::Host
}

fn default_brand_text() -> String {
    "devsignal".to_string()
}

impl Default for PresenceSection {
    fn default() -> Self {
        Self {
            name: PresenceLine::Off,
            details: default_details_line(),
            state: default_state_line(),
            brand_text: default_brand_text(),
        }
    }
}

impl PresenceSection {
    fn lines(&self) -> [PresenceLine; 3] {
        [self.name, self.details, self.state]
    }

    fn uses(&self, line: PresenceLine) -> bool {
        self.lines().contains(&line)
    }

    pub fn validate(&self, show_cwd_basename: bool) -> Result<()> {
        if self.uses(PresenceLine::Project) {
            anyhow::ensure!(
                show_cwd_basename,
                "a presence line is set to \"project\", but show_cwd_basename = false, so the \
                 project name is never resolved and the line would always be blank"
            );
        }
        if self.uses(PresenceLine::Brand) {
            anyhow::ensure!(
                !self.brand_text.trim().is_empty(),
                "a presence line is set to \"brand\", but presence.brand_text is empty"
            );
        }
        // Two slots showing the same thing is the exact complaint this section exists to fix, so
        // it is a config error rather than a cosmetic surprise at runtime.
        let slots = [
            ("name", self.name),
            ("details", self.details),
            ("state", self.state),
        ];
        for (i, (slot, line)) in slots.iter().enumerate() {
            if *line == PresenceLine::Off {
                continue;
            }
            for (other_slot, other) in slots.iter().skip(i + 1) {
                anyhow::ensure!(
                    line != other,
                    "presence.{slot} and presence.{other_slot} are both \"{}\"; \
                     the same text would appear on two lines",
                    line.as_str()
                );
            }
        }
        Ok(())
    }
}

/// Whether image values name an uploaded Discord art asset or a hosted PNG.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageMode {
    /// Art-asset keys uploaded under Developer Portal → Rich Presence → Art Assets.
    #[default]
    Key,
    /// `{base_url}/{agents|hosts}/{value}.png`, so nothing has to be uploaded. Discord accepts a
    /// plain `https://` image URL wherever it accepts an asset key.
    Url,
}

/// Where presence images come from.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagesConfig {
    #[serde(default)]
    pub mode: ImageMode,
    /// Folder that holds `agents/<id>.png` and `hosts/<slug>.png`. Only used by `mode = "url"`.
    #[serde(default = "default_image_base_url")]
    pub base_url: String,
    /// Show the frontmost editor or terminal as the small (corner) icon. Suppressed whenever the
    /// host label itself is hidden, so `hide_host` does not leak the app through its icon.
    #[serde(default)]
    pub host_icon: bool,
}

/// Discord's asset field is generous but not unbounded; keep resolved URLs comfortably inside it.
pub const IMAGE_BASE_URL_MAX_CHARS: usize = 400;

fn default_image_base_url() -> String {
    "https://raw.githubusercontent.com/rabbive/devsignal/main/assets/discord".to_string()
}

impl Default for ImagesConfig {
    fn default() -> Self {
        Self {
            mode: ImageMode::default(),
            base_url: default_image_base_url(),
            host_icon: false,
        }
    }
}

impl ImagesConfig {
    pub fn validate(&self) -> Result<()> {
        if self.mode == ImageMode::Url {
            let url = self.base_url.trim();
            anyhow::ensure!(
                !url.is_empty(),
                "images.mode = \"url\" needs images.base_url"
            );
            anyhow::ensure!(
                url.starts_with("http://") || url.starts_with("https://"),
                "images.base_url {:?} must start with http:// or https://",
                self.base_url
            );
            let len = url.chars().count();
            anyhow::ensure!(
                len <= IMAGE_BASE_URL_MAX_CHARS,
                "images.base_url is {} characters; keep it under {}",
                len,
                IMAGE_BASE_URL_MAX_CHARS
            );
        }
        Ok(())
    }

    /// Turn a configured image value into what Discord should receive.
    ///
    /// An absolute URL is passed through untouched, so a single agent can point at its own art
    /// without switching the whole config to `mode = "url"`.
    pub fn resolve(&self, folder: ImageFolder, value: &str) -> String {
        let value = value.trim();
        if value.starts_with("http://") || value.starts_with("https://") {
            return value.to_string();
        }
        match self.mode {
            ImageMode::Key => value.to_string(),
            ImageMode::Url => format!(
                "{}/{}/{}.png",
                self.base_url.trim().trim_end_matches('/'),
                folder.as_str(),
                value
            ),
        }
    }
}

/// Which subfolder of `assets/discord/` an image lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFolder {
    Agents,
    Hosts,
}

impl ImageFolder {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFolder::Agents => "agents",
            ImageFolder::Hosts => "hosts",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    /// If any of these substrings appears in the command line, reject the process (case-insensitive).
    #[serde(default)]
    pub exclude_argv_substrings: Vec<String>,
    /// Large image for this agent, resolved through [`ImagesConfig`] (falls back to global).
    #[serde(default)]
    pub large_image: Option<String>,
    /// Lower number = higher priority when multiple agents match.
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Small (corner) image for this agent, resolved through [`ImagesConfig`]. Only used when
    /// `images.host_icon` is off or the host is hidden — otherwise the host icon owns that slot.
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct PlatformsConfig {
    #[serde(default)]
    pub disabled_hosts: Vec<String>,
    #[serde(default)]
    pub disabled_agents: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceRule {
    pub name: String,
    #[serde(default)]
    pub when: RuleWhen,
    #[serde(default)]
    pub then: RuleThen,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct RuleThen {
    #[serde(default)]
    pub hide_host: bool,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

/// Result of evaluating `[[rules]]`.
///
/// `matched_rule_name` is reported to the user by `detect` and `once`. It deliberately does **not**
/// live on [`PresenceView`]: that struct is the debouncer's equality key, so a rule-name change alone
/// would trigger a Discord write even when the visible text is identical.
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
/// way to tell a wrong `process_names` from "devsignal is not working". Nine further presets ship as
/// opt-in snippets in `docs/community-presets.md`, to be confirmed with `devsignal detect` and added
/// with `devsignal agents add`.
///
/// `process_names` matches the process name **or** the basename of `argv[0]`, case-insensitively, so
/// Node- and Python-wrapped CLIs are covered without extra entries.
///
/// `large_image` is an image **name**, resolved through [`ImagesConfig`]: a PNG stem under
/// `assets/discord/agents/` in url mode, an art-asset key you uploaded in key mode. Either way a name
/// with nothing behind it renders blank; `devsignal init` prints the list for the mode you chose.
/// Priorities are spaced by 10 so you can slot custom rules between presets.
pub fn agent_presets() -> Vec<AgentRule> {
    fn preset(
        id: &str,
        label: &str,
        process_names: &[&str],
        exclude_argv_substrings: &[&str],
        priority: i32,
        button: Option<(&str, &str)>,
    ) -> AgentRule {
        AgentRule {
            id: id.to_string(),
            label: Some(label.to_string()),
            process_names: process_names.iter().map(|s| s.to_string()).collect(),
            argv_substrings: vec![],
            exclude_argv_substrings: exclude_argv_substrings
                .iter()
                .map(|s| s.to_string())
                .collect(),
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
            &["/Applications/Claude.app/"],
            10,
            Some(("Claude Code Docs", "https://claude.ai/code")),
        ),
        preset(
            "codex",
            "Codex",
            &["codex"],
            &[],
            20,
            Some(("Codex on GitHub", "https://github.com/openai/codex")),
        ),
        preset(
            "opencode",
            "OpenCode",
            &["opencode"],
            &[],
            30,
            Some(("OpenCode Docs", "https://opencode.ai")),
        ),
        // Confirmed on macOS: the CLI is a bash wrapper that ends in
        // `exec -a "$0" node .../index.js`, so the process *name* is `node` while `argv[0]` keeps
        // the wrapper path. It matches on the argv[0] basename, not the name.
        preset(
            "cursor_agent",
            "Cursor Agent",
            &["cursor-agent"],
            &[],
            40,
            None,
        ),
    ]
}

/// Every distinct agent image name the presets reference — the `init` wizard's upload list in key
/// mode, and the file stems under `assets/discord/agents/` in url mode.
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
        self.presence
            .validate(self.show_cwd_basename)
            .context("[presence]")?;
        self.images.validate().context("[images]")?;
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
        for exclude in &self.exclude_argv_substrings {
            anyhow::ensure!(
                !exclude.trim().is_empty(),
                "agent {:?} has an empty exclude_argv_substrings entry",
                self.id
            );
            anyhow::ensure!(
                !self
                    .argv_substrings
                    .iter()
                    .any(|include| include.eq_ignore_ascii_case(exclude)),
                "agent {:?} includes and excludes the same argv substring {:?}",
                self.id,
                exclude
            );
        }
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
    /// Overrides the Discord application's name on line 1. `None` leaves Discord's default.
    pub name: Option<String>,
    /// Line 2. `None` omits it.
    pub details: Option<String>,
    /// Line 3. `None` omits it.
    pub state: Option<String>,
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
///
/// The payload is boxed so the enum is not sized by its larger variant — it is stored once per send,
/// so one allocation per accepted push is cheaper than carrying a `PresenceView` in every `Clear`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LastSent {
    Set(Box<PresenceView>),
    Clear,
}

/// How long an unchanged payload stays deduplicated before it is re-sent anyway.
///
/// Deduplication assumes Discord still shows what we last successfully sent, and nothing tells us when
/// that stops being true: a Discord that quit and reopened has no activity at all, and the IPC socket
/// offers no liveness signal. Without an expiry, an unchanged payload is never sent again — so a user
/// sitting in one terminal (a completely static view) would never get presence back after restarting
/// Discord, and the daemon would never even attempt a send to discover it was gone.
///
/// One write a minute against Discord's documented 15/minute budget, in exchange for bounding *any*
/// divergence — restart, sleep, a dropped socket — to a minute.
///
/// A re-assert is still subject to `min_push_interval_secs`, so the effective period is
/// `max(REASSERT_INTERVAL, min_interval)`. That is deliberate: someone who asked for at most one push
/// per five minutes should not get one per minute because nothing changed.
pub const REASSERT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Debouncer {
    min_interval: Duration,
    max_sends: usize,
    window: Duration,
    reassert_after: Duration,
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
            reassert_after: REASSERT_INTERVAL,
            last_sent: None,
            last_push: None,
            recent: std::collections::VecDeque::new(),
        }
    }

    /// Override the re-assert interval, so tests can exercise the expiry without sleeping a minute.
    pub fn with_reassert_after(mut self, reassert_after: Duration) -> Self {
        self.reassert_after = reassert_after;
        self
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

    /// Whether `action` is what we last told Discord. Compares in place rather than building a
    /// `LastSent`, because this now runs every tick whether or not anything is sent.
    fn matches_last(&self, action: PresenceAction<'_>) -> bool {
        match (self.last_sent.as_ref(), action) {
            (Some(LastSent::Set(prev)), PresenceAction::Set(view)) => prev.as_ref() == view,
            (Some(LastSent::Clear), PresenceAction::Clear) => true,
            _ => false,
        }
    }

    /// Whether `action` may be sent to Discord now.
    ///
    /// Does **not** record it — call [`Debouncer::record_sent`] once the sink confirms the send
    /// landed. Recording an unconfirmed send is what used to wedge the daemon: a failed push was
    /// marked as delivered, the next tick deduped against it, and the sink was never called again, so
    /// the reconnect inside it never fired. Quitting Discord killed presence until the agent changed.
    ///
    /// `force` (an agent transition, the first tick, or a config reload) skips the equality check and
    /// the `min_push_interval_secs` wait, but **not** the rate limit. Without that last part, an agent
    /// process that flaps in and out on alternate polls makes every tick a transition, and the daemon
    /// writes to Discord every `poll_interval_secs` indefinitely.
    pub fn may_send(&mut self, action: PresenceAction<'_>, force: bool) -> bool {
        let now = Instant::now();
        if self.rate_limited(now) {
            return false;
        }
        if force {
            return true;
        }
        if self.matches_last(action) && !self.reassert_due(now) {
            return false;
        }
        if let Some(t) = self.last_push {
            if now.duration_since(t) < self.min_interval {
                return false;
            }
        }
        true
    }

    /// Whether an unchanged payload is old enough to be re-sent anyway. See [`REASSERT_INTERVAL`].
    ///
    /// With no recorded send there is nothing to re-assert — `matches_last` is false in that case
    /// anyway, so this only matters for the belt-and-braces reading.
    fn reassert_due(&self, now: Instant) -> bool {
        match self.last_push {
            Some(t) => now.duration_since(t) >= self.reassert_after,
            None => false,
        }
    }

    /// Record a send the sink confirmed: advances the dedupe key, the minimum-interval clock, and the
    /// 5-per-20s window.
    ///
    /// A *failed* send deliberately records nothing. It never reached Discord, so it consumed none of
    /// Discord's budget, and leaving the dedupe key untouched is exactly what lets the next tick retry
    /// the same payload. The retry rate is bounded by [`RetryBackoff`] instead.
    pub fn record_sent(&mut self, action: PresenceAction<'_>) {
        let sent = match action {
            PresenceAction::Set(view) => LastSent::Set(Box::new(view.clone())),
            PresenceAction::Clear => LastSent::Clear,
        };
        let now = Instant::now();
        self.last_sent = Some(sent);
        self.last_push = Some(now);
        self.recent.push_back(now);
    }
}

/// Default retry pacing for a failing presence sink.
///
/// The base matches the start of `connect_with_wait`'s backoff, so the first retry is effectively the
/// next poll. The cap is much higher than that function's, because this one bounds a daemon's whole
/// lifetime rather than a 30-second startup wait: with Discord closed all day, one attempt a minute is
/// plenty to notice it reopening.
pub const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(400);
pub const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Paces retries after a presence send fails.
///
/// Necessary because a failed send records nothing (see [`Debouncer::record_sent`]), so the payload
/// passes [`Debouncer::may_send`] again on the very next tick. That is right for a Discord that just
/// restarted and wrong for one that is closed — without a gate the daemon would reopen a dead socket
/// every `poll_interval_secs` forever.
///
/// It gates *every* failure, not just "Discord is not running": a payload Discord actively rejects is
/// indistinguishable at this layer, and would otherwise be retried every couple of seconds while
/// consuming no rate-limit slot.
///
/// Deliberately the same doubling-with-cap shape as `FallbackGate` in `devsignal-macos`. It cannot
/// reuse that type, which lives in a macOS-only crate; this one has to run on Linux CI. `base` and
/// `max` are constructor-injected so tests can pass `Duration::ZERO` instead of sleeping.
#[derive(Debug, Clone)]
pub struct RetryBackoff {
    base: Duration,
    max: Duration,
    consecutive_failures: u32,
    next_attempt: Option<Instant>,
}

impl Default for RetryBackoff {
    fn default() -> Self {
        Self::new(RETRY_BACKOFF_BASE, RETRY_BACKOFF_MAX)
    }
}

impl RetryBackoff {
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            consecutive_failures: 0,
            next_attempt: None,
        }
    }

    /// Whether a send may be attempted now.
    pub fn ready(&self, now: Instant) -> bool {
        match self.next_attempt {
            Some(t) => now >= t,
            None => true,
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.next_attempt = None;
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        // Double per failure: 400ms, 800ms, 1.6s, … then capped.
        let shift = self.consecutive_failures.saturating_sub(1).min(16);
        let backoff = self.base.saturating_mul(1u32 << shift).min(self.max);
        self.next_attempt = Some(now + backoff);
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// How long until the next attempt is allowed, for the one log line that reports a failure.
    pub fn retry_delay(&self, now: Instant) -> Duration {
        match self.next_attempt {
            Some(t) => t.saturating_duration_since(now),
            None => Duration::ZERO,
        }
    }
}

/// A host app devsignal can name and illustrate.
///
/// `image` is the stem of the file in `assets/discord/hosts/`, which is also the art-asset key to
/// upload under `mode = "key"`. A core test asserts every stem here exists on disk, so adding a
/// host without its icon fails the build rather than showing a blank circle in Discord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostApp {
    pub bundle_id: &'static str,
    pub label: &'static str,
    pub image: &'static str,
}

const fn host(bundle_id: &'static str, label: &'static str, image: &'static str) -> HostApp {
    HostApp {
        bundle_id,
        label,
        image,
    }
}

/// Known bundle id → short label + icon (editors, terminals, JetBrains SKUs).
pub const HOST_APPS: &[HostApp] = &[
    host(
        "com.anthropic.claudefordesktop",
        "Claude Desktop",
        "claude_desktop",
    ),
    host("com.todesktop.230313mzl4w4u92", "Cursor", "cursor"),
    host("com.microsoft.VSCode", "VS Code", "vs_code"),
    host("com.vscodium", "VSCodium", "vscodium"),
    host("dev.zed.Zed", "Zed", "zed"),
    host("com.apple.dt.Xcode", "Xcode", "xcode"),
    host("com.sublimetext.4", "Sublime Text", "sublime_text"),
    host("com.sublimetext.3", "Sublime Text", "sublime_text"),
    host("com.panic.Nova", "Nova", "nova"),
    host("com.jetbrains.fleet", "Fleet", "fleet"),
    host("com.jetbrains.intellij", "IntelliJ IDEA", "intellij_idea"),
    host("com.jetbrains.pycharm", "PyCharm", "pycharm"),
    host("com.jetbrains.WebStorm", "WebStorm", "webstorm"),
    host("com.jetbrains.goland", "GoLand", "goland"),
    host("com.jetbrains.rubymine", "RubyMine", "rubymine"),
    host("com.jetbrains.clion", "CLion", "clion"),
    host("com.jetbrains.phpstorm", "PhpStorm", "phpstorm"),
    host("com.jetbrains.rustrover", "RustRover", "rustrover"),
    host("com.jetbrains.datagrip", "DataGrip", "datagrip"),
    host("com.jetbrains.aqua", "Aqua", "aqua"),
    host("com.apple.Terminal", "Terminal", "terminal"),
    host("com.googlecode.iterm2", "iTerm2", "iterm2"),
    host("dev.warp.Warp-Stable", "Warp", "warp"),
    host("com.mitchellh.ghostty", "Ghostty", "ghostty"),
    host("net.kovidgoyal.kitty", "Kitty", "kitty"),
    host("org.alacritty.Alacritty", "Alacritty", "alacritty"),
    host("co.zeit.hyper", "Hyper", "hyper"),
    host("com.raphaelamorim.tabby", "Tabby", "tabby"),
    host("com.github.wez.wezterm", "WezTerm", "wezterm"),
];

/// Icon used when the frontmost app is unknown — also the label `macOS` falls back to.
pub const UNKNOWN_HOST_IMAGE: &str = "macos";

/// Every distinct host icon stem, for the `init` wizard's upload list under `mode = "key"`.
///
/// Includes the two stems only [`host_image_for_bundle`]'s prefix heuristics can produce, which are
/// otherwise absent from [`HOST_APPS`].
pub fn host_image_keys() -> Vec<String> {
    let mut keys = vec![
        UNKNOWN_HOST_IMAGE.to_string(),
        "jetbrains".to_string(),
        "android_studio".to_string(),
    ];
    for app in HOST_APPS {
        if !keys.iter().any(|k| k == app.image) {
            keys.push(app.image.to_string());
        }
    }
    keys
}

fn host_app_for_bundle(bundle_id: &str) -> Option<&'static HostApp> {
    HOST_APPS.iter().find(|app| app.bundle_id == bundle_id)
}

/// Map common macOS bundle IDs to a short host label for Discord `state`.
/// Covers Tier A/B editors plus common terminals (Tier C).
pub fn host_label_for_bundle(bundle_id: &str) -> String {
    if let Some(app) = host_app_for_bundle(bundle_id) {
        return app.label.to_string();
    }
    if bundle_id.starts_with("com.jetbrains.") || bundle_id.contains("jetbrains") {
        return "JetBrains".to_string();
    }
    if bundle_id.starts_with("com.google.android.studio") {
        return "Android Studio".to_string();
    }
    bundle_id.to_string()
}

/// Icon stem for a host, following the same prefix heuristics as [`host_label_for_bundle`].
///
/// Unknown apps get [`UNKNOWN_HOST_IMAGE`] rather than nothing: a raw bundle id would resolve to a
/// 404 in url mode and a blank circle in key mode.
pub fn host_image_for_bundle(bundle_id: Option<&str>) -> &'static str {
    let Some(bundle_id) = bundle_id else {
        return UNKNOWN_HOST_IMAGE;
    };
    if let Some(app) = host_app_for_bundle(bundle_id) {
        return app.image;
    }
    if bundle_id.starts_with("com.jetbrains.") || bundle_id.contains("jetbrains") {
        return "jetbrains";
    }
    if bundle_id.starts_with("com.google.android.studio") {
        return "android_studio";
    }
    UNKNOWN_HOST_IMAGE
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
/// `argv_substrings` against the full command line (case-insensitive). Any matching
/// `exclude_argv_substrings` rejects the process after the include checks.
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
    let joined = cmd
        .iter()
        .map(|s| s.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let joined_l = joined.to_lowercase();
    if !rule.argv_substrings.is_empty()
        && !rule
            .argv_substrings
            .iter()
            .any(|needle| joined_l.contains(&needle.to_lowercase()))
    {
        return false;
    }
    !rule
        .exclude_argv_substrings
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

/// Everything outside the config that shapes one presence payload.
#[derive(Debug, Clone, Copy, Default)]
pub struct PresenceInputs<'a> {
    pub agent: Option<&'a ActiveAgent>,
    /// `None` when the frontmost app could not be determined; renders as `macOS`.
    pub host_bundle_id: Option<&'a str>,
    /// Suppress the host entirely — `platforms.disabled_hosts`, or a rule's `then.hide_host`. Also
    /// suppresses the host icon, so hiding the label does not leak the app through its image.
    pub hide_host: bool,
    pub session_start_unix: Option<u64>,
    pub cwd_basename: Option<&'a str>,
}

/// Render one line slot. `host` is `None` when the host is unknown *or* deliberately hidden; the
/// two cases differ only in that a hidden host still gets a neutral `Working`.
fn render_line(
    cfg: &Config,
    line: PresenceLine,
    input: &PresenceInputs<'_>,
    host: Option<&str>,
    project_has_own_line: bool,
) -> Option<String> {
    let active = input.agent.is_some();
    let project = input.cwd_basename.filter(|s| !s.is_empty());
    // The project rides along with the host label unless it already occupies a line of its own.
    let suffix = match project {
        Some(p) if !project_has_own_line => format!(" · {p}"),
        _ => String::new(),
    };

    match line {
        PresenceLine::Agent => Some(match input.agent {
            Some(a) => a.label.clone(),
            None => "Idle".to_string(),
        }),
        PresenceLine::Host => Some(match (host, active) {
            (Some(host), true) => format!("In {host}{suffix}"),
            (Some(host), false) => format!("{host} · no agent CLI detected"),
            // Hidden host: say something true without naming the app.
            (None, true) => format!("Working{suffix}"),
            (None, false) => "No agent CLI detected".to_string(),
        }),
        PresenceLine::Project => project.map(str::to_string),
        PresenceLine::Brand => {
            Some(cfg.presence.brand_text.trim().to_string()).filter(|brand| !brand.is_empty())
        }
        PresenceLine::Off => None,
    }
}

pub fn build_presence_view(cfg: &Config, input: &PresenceInputs<'_>) -> PresenceView {
    let host_label = if input.hide_host {
        None
    } else {
        Some(
            input
                .host_bundle_id
                .map(host_label_for_bundle)
                .unwrap_or_else(|| "macOS".to_string()),
        )
    };
    let project_has_own_line = cfg.presence.uses(PresenceLine::Project);
    let line = |slot: PresenceLine| {
        render_line(
            cfg,
            slot,
            input,
            host_label.as_deref(),
            project_has_own_line,
        )
    };

    // Agent art when an agent is active, the idle art otherwise.
    let (large_image, agent_small) = match input.agent {
        Some(a) => (
            a.large_image.clone(),
            (a.small_image.clone(), a.small_text.clone()),
        ),
        None => (
            cfg.discord.large_image.clone(),
            (
                cfg.discord.small_image.clone(),
                cfg.discord.small_text.clone(),
            ),
        ),
    };

    // The host icon owns the small slot when enabled, since it is the only place the editor or
    // terminal can appear as an image. The agent's own small image is the fallback.
    let host_icon = match (cfg.images.host_icon, host_label.as_deref()) {
        (true, Some(label)) => Some((
            cfg.images.resolve(
                ImageFolder::Hosts,
                host_image_for_bundle(input.host_bundle_id),
            ),
            label.to_string(),
        )),
        _ => None,
    };
    let (small_image, small_text) = match host_icon {
        Some((image, label)) => (Some(image), Some(label)),
        None => (
            agent_small
                .0
                .map(|key| cfg.images.resolve(ImageFolder::Agents, &key)),
            agent_small.1,
        ),
    };

    PresenceView {
        name: line(cfg.presence.name),
        details: line(cfg.presence.details),
        state: line(cfg.presence.state),
        large_image: cfg.images.resolve(ImageFolder::Agents, &large_image),
        large_text: cfg.discord.large_text.clone(),
        small_image,
        small_text,
        buttons: input.agent.map(|a| a.buttons.clone()).unwrap_or_default(),
        start_timestamp_unix: input.session_start_unix.filter(|_| input.agent.is_some()),
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
            presence: PresenceSection::default(),
            images: ImagesConfig::default(),
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
            exclude_argv_substrings: vec![],
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
    fn validate_rejects_empty_or_contradictory_argv_exclusions() {
        let mut empty = valid_config();
        empty.agents[0].exclude_argv_substrings = vec!["  ".into()];
        assert!(err_of(&empty).contains("empty exclude_argv_substrings"));

        let mut contradictory = valid_config();
        contradictory.agents[0].argv_substrings = vec!["codex".into()];
        contradictory.agents[0].exclude_argv_substrings = vec!["CODEX".into()];
        assert!(err_of(&contradictory).contains("includes and excludes"));
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
        assert_eq!(
            ids,
            vec!["claude_code", "codex", "opencode", "cursor_agent"]
        );
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

    fn assets_dir() -> PathBuf {
        // crates/devsignal-core -> repo root.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/discord")
            .canonicalize()
            .expect("assets/discord must exist")
    }

    /// Drift guard: url mode turns these names straight into `raw.githubusercontent.com` paths, so a
    /// name with no PNG behind it is a 404 in Discord — and in key mode, a blank circle. Either way
    /// the daemon looks broken with nothing in the logs, which is the failure mode all the other
    /// validation exists to avoid.
    #[test]
    fn every_host_icon_name_has_a_png() {
        let hosts = assets_dir().join("hosts");
        for name in host_image_keys() {
            let path = hosts.join(format!("{name}.png"));
            assert!(
                path.exists(),
                "missing {}; run python3 scripts/build-discord-assets.py",
                path.display()
            );
        }
        for app in HOST_APPS {
            assert!(
                hosts.join(format!("{}.png", app.image)).exists(),
                "host {} references icon {:?} with no PNG",
                app.bundle_id,
                app.image
            );
        }
    }

    #[test]
    fn every_preset_agent_image_has_a_png() {
        let agents = assets_dir().join("agents");
        for name in preset_asset_keys() {
            let path = agents.join(format!("{name}.png"));
            assert!(
                path.exists(),
                "missing {}; run python3 scripts/build-discord-assets.py",
                path.display()
            );
        }
    }

    /// The heuristic branches of `host_image_for_bundle` are the ones with no table entry to keep
    /// them honest, so they get asserted explicitly.
    #[test]
    fn host_image_falls_back_before_it_returns_nothing() {
        assert_eq!(
            host_image_for_bundle(Some("com.mitchellh.ghostty")),
            "ghostty"
        );
        assert_eq!(
            host_image_for_bundle(Some("com.jetbrains.unknown")),
            "jetbrains"
        );
        assert_eq!(
            host_image_for_bundle(Some("com.google.android.studio.preview")),
            "android_studio"
        );
        assert_eq!(host_image_for_bundle(Some("com.example.unknown")), "macos");
        assert_eq!(host_image_for_bundle(None), "macos");
    }

    #[test]
    fn url_mode_expands_names_and_passes_absolute_urls_through() {
        let images = ImagesConfig {
            mode: ImageMode::Url,
            base_url: "https://example.test/assets/".to_string(),
            host_icon: true,
        };
        // The trailing slash must not double up.
        assert_eq!(
            images.resolve(ImageFolder::Agents, "claude_code"),
            "https://example.test/assets/agents/claude_code.png"
        );
        assert_eq!(
            images.resolve(ImageFolder::Hosts, "ghostty"),
            "https://example.test/assets/hosts/ghostty.png"
        );
        // An agent pointing at its own art stays untouched, in either mode.
        assert_eq!(
            images.resolve(ImageFolder::Agents, "https://cdn.test/x.png"),
            "https://cdn.test/x.png"
        );
        let keys = ImagesConfig::default();
        assert_eq!(
            keys.resolve(ImageFolder::Agents, "claude_code"),
            "claude_code"
        );
        assert_eq!(
            keys.resolve(ImageFolder::Agents, "https://cdn.test/x.png"),
            "https://cdn.test/x.png"
        );
    }

    #[test]
    fn default_images_and_presence_preserve_the_original_payload() {
        let images = ImagesConfig::default();
        assert_eq!(images.mode, ImageMode::Key);
        assert!(!images.host_icon, "host icons need uploads in key mode");
        let presence = PresenceSection::default();
        assert_eq!(presence.name, PresenceLine::Off);
        assert_eq!(presence.details, PresenceLine::Agent);
        assert_eq!(presence.state, PresenceLine::Host);
    }

    #[test]
    fn validate_rejects_project_line_without_show_cwd_basename() {
        let mut cfg = valid_config();
        cfg.presence.state = PresenceLine::Project;
        cfg.show_cwd_basename = false;
        let msg = err_of(&cfg);
        assert!(msg.contains("show_cwd_basename"), "got {msg}");

        cfg.show_cwd_basename = true;
        cfg.validate()
            .expect("project line is fine with the flag on");
    }

    #[test]
    fn validate_rejects_duplicate_lines_and_empty_brand_text() {
        let mut cfg = valid_config();
        cfg.presence.name = PresenceLine::Agent;
        cfg.presence.details = PresenceLine::Agent;
        let msg = err_of(&cfg);
        assert!(msg.contains("two lines"), "got {msg}");

        let mut cfg = valid_config();
        cfg.presence.state = PresenceLine::Brand;
        cfg.presence.brand_text = "  ".into();
        assert!(err_of(&cfg).contains("brand_text"));
    }

    #[test]
    fn validate_rejects_a_base_url_that_is_not_http() {
        let mut cfg = valid_config();
        cfg.images.mode = ImageMode::Url;
        cfg.images.base_url = "assets/discord".into();
        let msg = err_of(&cfg);
        assert!(msg.contains("base_url"), "got {msg}");
        // Key mode never reads base_url, so it must not be validated there.
        cfg.images.mode = ImageMode::Key;
        cfg.validate().expect("key mode ignores base_url");
    }

    /// Every slot, in the agent-first arrangement plus a project line, so the interaction that
    /// matters is asserted: the project stops being appended to the host once it has its own line.
    #[test]
    fn project_line_replaces_the_host_suffix() {
        let mut cfg = sample_config();
        cfg.show_cwd_basename = true;
        cfg.presence = PresenceSection {
            name: PresenceLine::Agent,
            details: PresenceLine::Host,
            state: PresenceLine::Project,
            brand_text: "devsignal".into(),
        };
        let agent = ActiveAgent {
            id: "claude_code".into(),
            label: "Claude Code".into(),
            large_image: "claude_code".into(),
            small_image: None,
            small_text: None,
            buttons: vec![],
        };
        let v = build_presence_view(
            &cfg,
            &PresenceInputs {
                agent: Some(&agent),
                host_bundle_id: Some("com.mitchellh.ghostty"),
                cwd_basename: Some("devsignal"),
                ..PresenceInputs::default()
            },
        );
        assert_eq!(v.name.as_deref(), Some("Claude Code"));
        assert_eq!(v.details.as_deref(), Some("In Ghostty"));
        assert_eq!(v.state.as_deref(), Some("devsignal"));

        // Without a project line the basename rides along with the host, as it always has.
        cfg.presence.state = PresenceLine::Brand;
        let v = build_presence_view(
            &cfg,
            &PresenceInputs {
                agent: Some(&agent),
                host_bundle_id: Some("com.mitchellh.ghostty"),
                cwd_basename: Some("devsignal"),
                ..PresenceInputs::default()
            },
        );
        assert_eq!(v.details.as_deref(), Some("In Ghostty · devsignal"));
    }

    /// `off` must omit the line rather than send an empty string, which Discord renders as a blank
    /// row in the card.
    #[test]
    fn off_omits_the_line() {
        let mut cfg = sample_config();
        cfg.presence.details = PresenceLine::Off;
        cfg.presence.state = PresenceLine::Off;
        let v = build_presence_view(&cfg, &PresenceInputs::default());
        assert_eq!(v.name, None);
        assert_eq!(v.details, None);
        assert_eq!(v.state, None);
    }

    #[test]
    fn host_icon_takes_the_small_slot_and_names_the_host() {
        let mut cfg = sample_config();
        cfg.images = ImagesConfig {
            mode: ImageMode::Url,
            base_url: "https://example.test/a".into(),
            host_icon: true,
        };
        let agent = ActiveAgent {
            id: "claude_code".into(),
            label: "Claude Code".into(),
            large_image: "claude_code".into(),
            small_image: Some("devsignal".into()),
            small_text: Some("devsignal".into()),
            buttons: vec![],
        };
        let v = build_presence_view(
            &cfg,
            &PresenceInputs {
                agent: Some(&agent),
                host_bundle_id: Some("com.googlecode.iterm2"),
                ..PresenceInputs::default()
            },
        );
        assert_eq!(
            v.large_image,
            "https://example.test/a/agents/claude_code.png"
        );
        assert_eq!(
            v.small_image.as_deref(),
            Some("https://example.test/a/hosts/iterm2.png")
        );
        assert_eq!(v.small_text.as_deref(), Some("iTerm2"));

        // With host icons off, the agent's own small image comes back — resolved, not raw.
        cfg.images.host_icon = false;
        let v = build_presence_view(
            &cfg,
            &PresenceInputs {
                agent: Some(&agent),
                host_bundle_id: Some("com.googlecode.iterm2"),
                ..PresenceInputs::default()
            },
        );
        assert_eq!(
            v.small_image.as_deref(),
            Some("https://example.test/a/agents/devsignal.png")
        );
        assert_eq!(v.small_text.as_deref(), Some("devsignal"));
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

    /// A typo'd key used to parse fine and be silently ignored, so the setting simply never applied.
    #[test]
    fn unknown_config_keys_are_rejected() {
        let cases = [
            // Misspelled top-level key.
            r#"show_cwd_basemane = true
               [discord]
               client_id = "1"
               [[agents]]
               id = "a"
               process_names = ["a"]"#,
            // Misspelled table name.
            r#"[discord]
               client_id = "1"
               [platform]
               disabled_hosts = []
               [[agents]]
               id = "a"
               process_names = ["a"]"#,
            // Misspelled key inside a nested table.
            r#"[discord]
               client_id = "1"
               [platforms]
               disabled_host = []
               [[agents]]
               id = "a"
               process_names = ["a"]"#,
            // Misspelled agent field.
            r#"[discord]
               client_id = "1"
               [[agents]]
               id = "a"
               process_name = ["a"]"#,
            // Misspelled rule field.
            r#"[discord]
               client_id = "1"
               [[agents]]
               id = "a"
               process_names = ["a"]
               [[rules]]
               name = "r"
               then = { hide_host = true, stat = "x" }"#,
        ];
        for (idx, case) in cases.iter().enumerate() {
            let err = toml::from_str::<Config>(case)
                .expect_err(&format!("case {idx} should have been rejected"));
            let msg = format!("{err}");
            assert!(
                msg.contains("unknown field"),
                "case {idx} should name the unknown field, got: {msg}"
            );
        }
    }

    /// The corollary: every key the shipped example uses must still be accepted.
    #[test]
    fn the_example_config_has_no_unknown_keys() {
        let raw = include_str!("../../../config.example.toml");
        toml::from_str::<Config>(raw)
            .expect("config.example.toml must parse with deny_unknown_fields");
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
            exclude_argv_substrings: vec![],
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
        let v = view_named("A");
        assert!(push(&mut d, &v, true));
        assert!(!push(&mut d, &v, false));
    }

    #[test]
    fn debouncer_new_payload_before_min_interval_suppressed() {
        let mut d = Debouncer::new(Duration::from_millis(400));
        let a = view_named("A");
        let b = view_named("B");
        assert!(push(&mut d, &a, true));
        assert!(!push(&mut d, &b, false));
        std::thread::sleep(Duration::from_millis(450));
        assert!(push(&mut d, &b, false));
    }

    /// Check-then-record in one call, i.e. what a sink that always succeeds looks like.
    ///
    /// Most debouncer tests care about the sequence of accepted sends rather than the two-phase
    /// split, and read better through this. The tests that exist *because* the split matters call
    /// `may_send` / `record_sent` directly.
    fn send(d: &mut Debouncer, action: PresenceAction<'_>, force: bool) -> bool {
        let ok = d.may_send(action, force);
        if ok {
            d.record_sent(action);
        }
        ok
    }

    fn push(d: &mut Debouncer, view: &PresenceView, force: bool) -> bool {
        send(d, PresenceAction::Set(view), force)
    }

    fn view_named(details: &str) -> PresenceView {
        PresenceView {
            name: None,
            details: Some(details.into()),
            state: Some("s".into()),
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
                push(&mut d, &view_named(&format!("v{i}")), true),
                "send {i} should be allowed"
            );
        }
        // Cap reached: further forced sends are refused despite `force`.
        assert!(!push(&mut d, &view_named("v3"), true));
        assert!(!push(&mut d, &view_named("v4"), true));
        // And a non-forced one too.
        assert!(!push(&mut d, &view_named("v5"), false));
    }

    #[test]
    fn rate_limit_window_slides() {
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 2, Duration::from_millis(300));
        assert!(push(&mut d, &view_named("a"), true));
        assert!(push(&mut d, &view_named("b"), true));
        assert!(!push(&mut d, &view_named("c"), true), "cap of 2 reached");

        // Once the window rolls past the earlier sends, capacity returns.
        std::thread::sleep(Duration::from_millis(350));
        assert!(push(&mut d, &view_named("c"), true));
    }

    /// `idle_mode = "clear"` used to bypass the debouncer entirely — it was not merely exempt from
    /// the rate limit, it never consulted it.
    #[test]
    fn clears_go_through_the_same_limiter_as_sets() {
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 2, Duration::from_secs(60));
        assert!(send(&mut d, PresenceAction::Clear, true));
        assert!(send(&mut d, PresenceAction::Clear, true));
        assert!(
            !send(&mut d, PresenceAction::Clear, true),
            "clears must consume rate-limit budget too"
        );
    }

    #[test]
    fn a_repeated_clear_is_deduplicated() {
        let mut d = Debouncer::with_limits(Duration::from_millis(1), 10, Duration::from_secs(60));
        assert!(send(&mut d, PresenceAction::Clear, false));
        assert!(
            !send(&mut d, PresenceAction::Clear, false),
            "an unchanged clear should not be resent"
        );
    }

    /// Set-then-clear-then-set must not be deduplicated away: the actions differ.
    #[test]
    fn alternating_set_and_clear_are_distinct_payloads() {
        let mut d = Debouncer::with_limits(Duration::ZERO, 10, Duration::from_secs(60));
        let v = view_named("a");
        assert!(send(&mut d, PresenceAction::Set(&v), false));
        assert!(send(&mut d, PresenceAction::Clear, false));
        assert!(send(&mut d, PresenceAction::Set(&v), false));
    }

    #[test]
    fn set_min_interval_preserves_dedupe_and_window_state() {
        let mut d = Debouncer::with_limits(Duration::from_secs(60), 5, Duration::from_secs(60));
        let v = view_named("a");
        assert!(push(&mut d, &v, true));
        assert_eq!(d.min_interval(), Duration::from_secs(60));

        // Hot-reload lowering the interval must not forget what was already sent.
        d.set_min_interval(Duration::ZERO);
        assert_eq!(d.min_interval(), Duration::ZERO);
        assert!(
            !push(&mut d, &v, false),
            "the identical payload should still be deduplicated after a reload"
        );
        assert!(push(&mut d, &view_named("b"), false));
    }

    /// The core of the wedge fix: checking permission must not mark the payload as delivered. If it
    /// does, a failed send is indistinguishable from a successful one and the retry never happens.
    ///
    /// Reverting `may_send` to the old record-on-check behaviour fails this test.
    #[test]
    fn may_send_does_not_record_until_record_sent() {
        let mut d = Debouncer::with_limits(Duration::ZERO, 10, Duration::from_secs(60));
        let v = view_named("a");

        assert!(d.may_send(PresenceAction::Set(&v), false));
        assert!(
            d.may_send(PresenceAction::Set(&v), false),
            "an unconfirmed payload must still be sendable — this is the retry path"
        );

        d.record_sent(PresenceAction::Set(&v));
        assert!(
            !d.may_send(PresenceAction::Set(&v), false),
            "once confirmed, the identical payload is deduplicated"
        );
    }

    /// Deduplication assumes Discord still shows what we last sent, and nothing tells us when that
    /// stops being true — a Discord that quit and reopened has no activity and no way to say so. So an
    /// unchanged payload has to be re-sent eventually, or a user with a completely static view (one
    /// terminal, host label unchanged) never gets presence back, and the daemon never even attempts a
    /// send to discover Discord was gone.
    #[test]
    fn an_unchanged_payload_is_resent_after_the_reassert_interval() {
        let mut d = Debouncer::with_limits(Duration::ZERO, 100, Duration::from_secs(60))
            .with_reassert_after(Duration::from_millis(120));
        let v = view_named("a");

        assert!(push(&mut d, &v, true), "first send goes out");
        assert!(
            !push(&mut d, &v, false),
            "an unchanged payload is deduplicated straight away"
        );

        std::thread::sleep(Duration::from_millis(150));
        assert!(
            push(&mut d, &v, false),
            "once the re-assert interval passes, the same payload goes out again"
        );
    }

    /// The expiry must not degenerate into "no deduplication at all" — that was the pre-0.3.0 behaviour
    /// of writing to Discord every tick.
    #[test]
    fn an_unchanged_payload_is_still_deduped_inside_the_interval() {
        let mut d = Debouncer::with_limits(Duration::ZERO, 100, Duration::from_secs(60))
            .with_reassert_after(Duration::from_secs(60));
        let v = view_named("a");

        assert!(push(&mut d, &v, true));
        for i in 0..20 {
            assert!(
                !push(&mut d, &v, false),
                "tick {i} is well inside the interval and must be suppressed"
            );
        }
    }

    #[test]
    fn the_default_reassert_interval_is_a_minute() {
        assert_eq!(REASSERT_INTERVAL, Duration::from_secs(60));
        // Longer than the rate-limit window, so a re-assert can never be what exhausts the budget.
        assert!(REASSERT_INTERVAL > RATE_LIMIT_WINDOW);
    }

    /// A send that never reached Discord consumed none of Discord's budget, so it must not eat a slot.
    /// Otherwise a long Discord outage would exhaust the window and delay recovery once it came back.
    #[test]
    fn a_failed_send_does_not_consume_a_rate_limit_slot() {
        let mut d = Debouncer::with_limits(Duration::ZERO, 2, Duration::from_secs(60));
        let v = view_named("a");

        for i in 0..10 {
            assert!(
                d.may_send(PresenceAction::Set(&v), false),
                "unrecorded attempt {i} must not consume budget"
            );
        }

        d.record_sent(PresenceAction::Set(&v));
        d.record_sent(PresenceAction::Clear);
        assert!(
            !d.may_send(PresenceAction::Set(&view_named("b")), true),
            "two confirmed sends fill a cap of 2"
        );
    }

    #[test]
    fn retry_backoff_doubles_and_caps() {
        let base = Duration::from_millis(100);
        let max = Duration::from_millis(400);
        let mut b = RetryBackoff::new(base, max);
        let t0 = Instant::now();

        assert!(b.ready(t0), "a fresh backoff permits the first attempt");

        b.record_failure(t0);
        assert!(!b.ready(t0));
        assert!(b.ready(t0 + base), "100ms after the first failure");
        assert_eq!(b.consecutive_failures(), 1);

        b.record_failure(t0);
        assert!(!b.ready(t0 + base), "second failure doubles to 200ms");
        assert!(b.ready(t0 + 2 * base));

        // Past the cap it stops growing.
        for _ in 0..10 {
            b.record_failure(t0);
        }
        assert!(b.ready(t0 + max));
        assert_eq!(b.consecutive_failures(), 12);
    }

    #[test]
    fn retry_backoff_success_resets() {
        let mut b = RetryBackoff::new(Duration::from_secs(30), Duration::from_secs(60));
        let t0 = Instant::now();

        b.record_failure(t0);
        assert!(!b.ready(t0));
        assert_eq!(b.consecutive_failures(), 1);

        b.record_success();
        assert!(b.ready(t0), "recovery must not leave the gate closed");
        assert_eq!(b.consecutive_failures(), 0);
        assert_eq!(b.retry_delay(t0), Duration::ZERO);
    }

    /// `Duration::ZERO` is what the daemon tests use to exercise the retry path without sleeping, so
    /// it has to mean "always ready".
    #[test]
    fn a_zero_base_backoff_never_blocks() {
        let mut b = RetryBackoff::new(Duration::ZERO, Duration::ZERO);
        let t0 = Instant::now();
        for _ in 0..5 {
            b.record_failure(t0);
            assert!(b.ready(t0));
        }
    }

    #[test]
    fn default_limits_match_discords_documented_rate() {
        assert_eq!(RATE_LIMIT_MAX_SENDS, 5);
        assert_eq!(RATE_LIMIT_WINDOW, Duration::from_secs(20));
    }

    #[test]
    fn debouncer_force_always_pushes() {
        let mut d = Debouncer::new(Duration::from_secs(60));
        let v = view_named("A");
        assert!(push(&mut d, &v, true));
        assert!(push(&mut d, &v, true));
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
            &PresenceInputs {
                agent: Some(&agent),
                host_bundle_id: Some("com.microsoft.VSCode"),
                session_start_unix: Some(99),
                cwd_basename: Some("proj"),
                ..PresenceInputs::default()
            },
        );
        // The default layout leaves line 1 to Discord: the application's own name.
        assert_eq!(v.name, None);
        assert_eq!(v.details.as_deref(), Some("My Agent"));
        assert_eq!(v.state.as_deref(), Some("In VS Code · proj"));
        assert_eq!(v.large_image, "img");
        assert_eq!(v.start_timestamp_unix, Some(99));
        assert!(v.small_image.is_none());
        assert!(v.buttons.is_empty());

        let idle = build_presence_view(&cfg, &PresenceInputs::default());
        assert_eq!(idle.details.as_deref(), Some("Idle"));
        assert_eq!(idle.state.as_deref(), Some("macOS · no agent CLI detected"));
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

    /// Claude Code's own desktop host used to fall through to the raw bundle id, so presence read
    /// `In com.anthropic.claudefordesktop` — observed on a real machine before this entry existed.
    #[test]
    fn host_label_covers_claude_desktop() {
        assert_eq!(
            host_label_for_bundle("com.anthropic.claudefordesktop"),
            "Claude Desktop"
        );
    }

    /// `cursor-agent` is a bash wrapper ending in `exec -a "$0" node .../index.js`, so sysinfo
    /// reports the name as `node` and only the argv[0] basename identifies it. Confirmed with
    /// `devsignal detect --unmatched` on macOS; this asserts the shipped preset matches that shape.
    #[test]
    fn cursor_agent_preset_matches_node_wrapper() {
        let rule = agent_presets()
            .into_iter()
            .find(|p| p.id == "cursor_agent")
            .expect("cursor_agent preset must ship");
        assert!(process_matches_rule(
            "node",
            &[
                OsStr::new("/Users/someone/.local/bin/cursor-agent"),
                OsStr::new("--use-system-ca"),
            ],
            &rule
        ));
        assert!(!process_matches_rule("node", &[OsStr::new("node")], &rule));
    }

    #[test]
    fn claude_code_preset_excludes_claude_desktop() {
        let rule = agent_presets()
            .into_iter()
            .find(|p| p.id == "claude_code")
            .expect("claude_code preset must ship");
        assert!(process_matches_rule(
            "claude",
            &[OsStr::new("/opt/homebrew/bin/claude")],
            &rule
        ));
        assert!(!process_matches_rule(
            "Claude",
            &[
                OsStr::new("/Applications/Claude.app/Contents/MacOS/Claude"),
                OsStr::new("--no-sandbox"),
            ],
            &rule
        ));
    }

    #[test]
    fn cline_rule_excludes_background_hub_daemon() {
        let rule = AgentRule {
            id: "cline".into(),
            label: Some("Cline".into()),
            process_names: vec![".cline".into()],
            argv_substrings: vec![],
            exclude_argv_substrings: vec!["--cline-hub-daemon".into()],
            large_image: None,
            priority: 180,
            small_image: None,
            small_text: None,
            buttons: vec![],
        };
        assert!(process_matches_rule(
            ".cline",
            &[OsStr::new(".cline"), OsStr::new("run")],
            &rule
        ));
        assert!(!process_matches_rule(
            ".cline",
            &[OsStr::new(".cline"), OsStr::new("--cline-hub-daemon")],
            &rule
        ));
    }

    #[test]
    fn host_bundle_labels_include_hyper_tabby_wezterm() {
        assert!(HOST_APPS.iter().any(|a| a.bundle_id == "co.zeit.hyper"));
        assert!(HOST_APPS
            .iter()
            .any(|a| a.bundle_id == "com.github.wez.wezterm"));
    }

    #[test]
    fn process_matches_rule_name_and_argv_case_insensitive() {
        let r = AgentRule {
            id: "t".into(),
            label: None,
            process_names: vec!["node".into()],
            argv_substrings: vec!["CODEX".into()],
            exclude_argv_substrings: vec![],
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
            exclude_argv_substrings: vec![],
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
            exclude_argv_substrings: vec![],
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
            exclude_argv_substrings = ["desktop"]
            large_image = "test_icon"
            priority = 7
        "#;
        let cfg: Config = toml::from_str(toml_str).expect("parse failed");
        let rule = &cfg.agents[0];
        assert_eq!(rule.large_image.as_deref(), Some("test_icon"));
        assert_eq!(rule.priority, 7);
        assert_eq!(rule.exclude_argv_substrings, vec!["desktop"]);
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
        let v = build_presence_view(
            &cfg,
            &PresenceInputs {
                agent: Some(&agent),
                ..PresenceInputs::default()
            },
        );
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
        let v = build_presence_view(&cfg, &PresenceInputs::default());
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
        let v = build_presence_view(
            &cfg,
            &PresenceInputs {
                agent: Some(&agent),
                ..PresenceInputs::default()
            },
        );
        assert!(v.small_image.is_none());
        assert!(v.small_text.is_none());
        assert!(v.buttons.is_empty());
    }
}
