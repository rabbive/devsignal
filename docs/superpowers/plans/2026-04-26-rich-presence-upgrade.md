# Rich Presence Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend Discord Rich Presence output with `small_image`, `small_text`, and up to 2 `buttons` per agent, configured in `config.toml` and surfaced in the Discord client.

**Architecture:** Add new optional fields to `AgentRule` (config) and `PresenceView` (data), thread them through `build_presence_view()` in `devsignal-core`, then wire them into the `Activity` builder in `devsignal-discord`. The global `[discord]` section also gets optional `small_image`/`small_text` defaults used during idle mode.

**Tech Stack:** Rust, `discord-rich-presence` v1.1.0 (`activity::Assets::small_image()`, `activity::Assets::small_text()`, `activity::Button::new()`, `activity::Activity::buttons(Vec<Button>)`), `serde`/`toml` for config.

---

## File Map

| File | Change |
|---|---|
| `crates/devsignal-core/src/lib.rs` | Add `ButtonConfig`, extend `AgentRule` + `DiscordSection` + `PresenceView`, update `build_presence_view()`, add tests |
| `crates/devsignal-discord/src/lib.rs` | Wire `small_image`, `small_text`, buttons into `set_presence()` |
| `config.example.toml` | Add `small_image`, `small_text`, `[[agents.buttons]]` to all 3 agents |

No new files. No changes to `devsignal-macos`, `devsignal-daemon`, or CI.

---

## Task 1: Commit CLAUDE.md

**Files:**
- Stage: `CLAUDE.md` (untracked)

- [ ] **Step 1: Stage and commit CLAUDE.md**

```bash
cd /Users/ashwanthkumaravel/Documents/GitHub/devsignal
git add CLAUDE.md
git commit -m "docs: add CLAUDE.md with project context for AI coding assistants"
```

Expected: commit created, clean `git status`.

---

## Task 2: Extend `AgentRule` and `DiscordSection` in core

**Files:**
- Modify: `crates/devsignal-core/src/lib.rs`

- [ ] **Step 1: Write failing tests for new config fields**

In `crates/devsignal-core/src/lib.rs`, add these tests inside the existing `#[cfg(test)] mod tests { ... }` block, before the closing `}`:

```rust
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
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cd /Users/ashwanthkumaravel/Documents/GitHub/devsignal
cargo test -p devsignal-core agent_rule_deserializes 2>&1 | tail -20
cargo test -p devsignal-core discord_section_deserializes 2>&1 | tail -20
```

Expected: compile error — fields `small_image`, `small_text`, `buttons` do not exist on `AgentRule`.

- [ ] **Step 3: Add `ButtonConfig` struct and extend `AgentRule` and `DiscordSection`**

In `crates/devsignal-core/src/lib.rs`, add the `ButtonConfig` struct directly after the `AgentRule` struct definition (around line 79):

```rust
/// A Discord Rich Presence button (label + URL). Maximum 2 per presence payload.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ButtonConfig {
    /// Displayed on the button in Discord (1–32 characters).
    pub label: String,
    /// URL opened when the button is clicked (1–512 characters).
    pub url: String,
}
```

Extend `AgentRule` by adding three fields after `priority`:

```rust
/// Discord asset key for the small (corner) image for this agent.
#[serde(default)]
pub small_image: Option<String>,
/// Tooltip shown on hover over the small image.
#[serde(default)]
pub small_text: Option<String>,
/// Up to 2 clickable buttons shown in the Discord presence panel.
#[serde(default)]
pub buttons: Vec<ButtonConfig>,
```

Extend `DiscordSection` by adding two fields after `large_text`:

```rust
/// Fallback small image key used during idle mode (optional).
#[serde(default)]
pub small_image: Option<String>,
/// Fallback small image tooltip used during idle mode (optional).
#[serde(default)]
pub small_text: Option<String>,
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test -p devsignal-core agent_rule_deserializes 2>&1 | tail -10
cargo test -p devsignal-core discord_section_deserializes 2>&1 | tail -10
```

Expected: `test agent_rule_deserializes_small_image_and_buttons ... ok` and `test discord_section_deserializes_small_image_defaults ... ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/devsignal-core/src/lib.rs
git commit -m "feat(core): add ButtonConfig, small_image/small_text to AgentRule and DiscordSection"
```

---

## Task 3: Extend `PresenceView` and `build_presence_view()`

**Files:**
- Modify: `crates/devsignal-core/src/lib.rs`

- [ ] **Step 1: Write failing tests for extended PresenceView**

Add these tests inside `#[cfg(test)] mod tests { ... }`:

```rust
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
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p devsignal-core build_presence_view_propagates 2>&1 | tail -20
```

Expected: compile error — `ActiveAgent` has no `small_image`/`small_text`/`buttons` fields; `PresenceView` has no `small_image`/`small_text`/`buttons` fields.

- [ ] **Step 3: Extend `ActiveAgent` and `PresenceView`**

Replace the existing `ActiveAgent` struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAgent {
    pub id: String,
    pub label: String,
    pub large_image: String,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    pub buttons: Vec<ButtonConfig>,
}
```

Replace the existing `PresenceView` struct:

```rust
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
```

- [ ] **Step 4: Fix `select_active_agent` to populate the new fields**

Update the `select_active_agent` function body — replace the `ActiveAgent { ... }` construction:

```rust
let agent = ActiveAgent {
    id: rule.id.clone(),
    label,
    large_image: large,
    small_image: rule.small_image.clone(),
    small_text: rule.small_text.clone(),
    buttons: rule.buttons.clone(),
};
```

- [ ] **Step 5: Fix `build_presence_view` to populate new fields**

Replace the entire `build_presence_view` function:

```rust
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
```

- [ ] **Step 6: Fix existing tests that construct `ActiveAgent` directly**

The existing test `build_presence_view_agent_and_idle` in the test module constructs `ActiveAgent` with the old fields. Update it to include the new fields:

```rust
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
```

Also update `select_active_agent_priority_and_pid_tiebreak` — the `rule()` helper creates `AgentRule` structs without `buttons`. Add the missing fields to the `AgentRule { ... }` construction inside the `rule()` helper:

```rust
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
```

- [ ] **Step 7: Run all core tests**

```bash
cargo test -p devsignal-core 2>&1 | tail -30
```

Expected: all tests pass (should be ~13+ tests now).

- [ ] **Step 8: Commit**

```bash
git add crates/devsignal-core/src/lib.rs
git commit -m "feat(core): extend ActiveAgent and PresenceView with small_image, small_text, buttons"
```

---

## Task 4: Wire new fields into `devsignal-discord`

**Files:**
- Modify: `crates/devsignal-discord/src/lib.rs`

- [ ] **Step 1: Read the current `set_presence` implementation**

```bash
cat -n /Users/ashwanthkumaravel/Documents/GitHub/devsignal/crates/devsignal-discord/src/lib.rs
```

Confirm the `set_presence` method currently builds `assets` using only `large_image`/`large_text`, and `act` has no `buttons()` call.

- [ ] **Step 2: Update `set_presence` to wire small_image, small_text, and buttons**

Replace the body of `set_presence` (the `pub fn set_presence` method on `PresenceSession`) with:

```rust
pub fn set_presence(&mut self, view: &PresenceView) -> Result<()> {
    let mut assets = activity::Assets::new()
        .large_image(view.large_image.clone())
        .large_text(view.large_text.clone());

    if let Some(ref si) = view.small_image {
        assets = assets.small_image(si.clone());
    }
    if let Some(ref st) = view.small_text {
        assets = assets.small_text(st.clone());
    }

    let mut act = activity::Activity::new()
        .details(view.details.clone())
        .state(view.state.clone())
        .assets(assets);

    if let Some(ts) = view.start_timestamp_unix {
        act = act.timestamps(activity::Timestamps::new().start(ts as i64));
    }

    let btns: Vec<activity::Button> = view
        .buttons
        .iter()
        .take(2)
        .map(|b| activity::Button::new(b.label.as_str(), b.url.as_str()))
        .collect();
    if !btns.is_empty() {
        act = act.buttons(btns);
    }

    self.client
        .set_activity(act)
        .map_err(|e| anyhow::anyhow!(e))
}
```

- [ ] **Step 3: Run full workspace build and clippy to verify**

```bash
cargo build --workspace 2>&1 | tail -20
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: clean build, zero clippy warnings.

- [ ] **Step 4: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/devsignal-discord/src/lib.rs
git commit -m "feat(discord): wire small_image, small_text, and buttons into set_presence"
```

---

## Task 5: Update `config.example.toml`

**Files:**
- Modify: `config.example.toml`

- [ ] **Step 1: Read the current config.example.toml**

```bash
cat /Users/ashwanthkumaravel/Documents/GitHub/devsignal/config.example.toml
```

- [ ] **Step 2: Rewrite config.example.toml with new fields**

Replace the entire file contents with:

```toml
# devsignal example configuration
# Copy to ~/.config/devsignal/config.toml and set discord.client_id.

poll_interval_secs     = 2    # How often to scan running processes (seconds).
min_push_interval_secs = 20   # Minimum time between Discord IPC pushes (reduces flicker).
idle_mode              = "status"  # "status" = show Idle line; "clear" = hide presence entirely.
show_cwd_basename      = false     # Show the basename of the agent's working directory. Off by default for privacy.

[discord]
# Your Discord Application ID from https://discord.com/developers/applications
client_id   = "YOUR_DISCORD_APPLICATION_ID"
large_image = "devsignal"   # Art asset key shown when no agent is detected (idle).
large_text  = "devsignal"   # Hover tooltip for the idle large image.
# small_image = "devsignal" # Optional: small corner icon shown during idle.
# small_text  = "Idle"      # Optional: tooltip for the idle small icon.

# ── Agent rules ────────────────────────────────────────────────────────────────
# process_names: case-insensitive match against the sysinfo process name OR
#                the basename of argv[0] (catches wrapped CLIs like "node .../codex").
# argv_substrings: narrow matches when non-empty — all must appear in the command line.
# priority: lower number wins when multiple agents match; ties break on lower PID.
# buttons: up to 2 clickable buttons in the Discord presence panel.

[[agents]]
id            = "claude_code"
label         = "Claude Code"
process_names = ["claude", "claude-code"]
priority      = 10
large_image   = "claude"
small_image   = "devsignal"
small_text    = "devsignal"

  [[agents.buttons]]
  label = "Claude Code Docs"
  url   = "https://claude.ai/code"

[[agents]]
id            = "codex"
label         = "Codex"
process_names = ["codex"]
priority      = 20
large_image   = "codex"
small_image   = "devsignal"
small_text    = "devsignal"

  [[agents.buttons]]
  label = "Codex on GitHub"
  url   = "https://github.com/openai/codex"

[[agents]]
id            = "opencode"
label         = "OpenCode"
process_names = ["opencode"]
priority      = 30
large_image   = "opencode"
small_image   = "devsignal"
small_text    = "devsignal"

  [[agents.buttons]]
  label = "OpenCode Docs"
  url   = "https://opencode.ai"
```

- [ ] **Step 3: Validate the new config parses correctly**

```bash
cd /Users/ashwanthkumaravel/Documents/GitHub/devsignal
cp config.example.toml /tmp/devsignal-test.toml
# Temporarily set a real-looking client_id so validate passes
sed -i '' 's/YOUR_DISCORD_APPLICATION_ID/123456789012345678/' /tmp/devsignal-test.toml
cargo run -p devsignal-daemon -- validate --config /tmp/devsignal-test.toml
```

Expected output: `OK: /tmp/devsignal-test.toml` followed by lines listing all 3 agents with `small_image` and `buttons` fields visible.

- [ ] **Step 4: Run full test suite one more time**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add config.example.toml
git commit -m "feat(config): add small_image, small_text, and buttons to all example agents"
```

---

## Task 6: Run `devsignal once` and verify JSON output

**Files:** none (verification only)

- [ ] **Step 1: Scaffold local config from updated example**

```bash
cp /Users/ashwanthkumaravel/Documents/GitHub/devsignal/config.example.toml /tmp/devsignal-once-test.toml
sed -i '' 's/YOUR_DISCORD_APPLICATION_ID/123456789012345678/' /tmp/devsignal-once-test.toml
```

- [ ] **Step 2: Run `once` subcommand and inspect output**

```bash
cd /Users/ashwanthkumaravel/Documents/GitHub/devsignal
cargo run --release -p devsignal-daemon -- once --config /tmp/devsignal-once-test.toml
```

Expected: pretty-printed JSON containing `small_image`, `small_text`, and `buttons` fields. Example (when no agent is running):

```json
{
  "details": "Idle",
  "state": "macOS · no agent CLI detected",
  "large_image": "devsignal",
  "large_text": "devsignal",
  "small_image": null,
  "small_text": null,
  "buttons": [],
  "start_timestamp_unix": null
}
```

If an agent (e.g. `claude`) is running, `buttons` will contain the configured entries.

- [ ] **Step 3: Tag and push**

```bash
cd /Users/ashwanthkumaravel/Documents/GitHub/devsignal
git tag v0.3.0
git push origin main --tags
```

Expected: CI passes, release workflow fires and produces `devsignal-0.3.0-macos-universal.tar.gz`.

---

## Verification Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` — all tests pass (13+ tests in `devsignal-core`)
- [ ] `devsignal once --config /tmp/devsignal-once-test.toml` outputs JSON with `small_image`, `small_text`, `buttons` fields
- [ ] `devsignal validate --config /tmp/devsignal-once-test.toml` prints OK with all 3 agents and their buttons
- [ ] Running `devsignal run` against Discord desktop shows small icon + buttons in the presence panel
