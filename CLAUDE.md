# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`devsignal` is a macOS-only daemon that publishes unified Discord Rich Presence for AI coding CLIs
(Claude Code, Codex, OpenCode, …). It polls running processes, detects which agent CLI is active and
which editor/terminal is frontmost, and pushes a debounced presence payload to the Discord desktop
app over Unix-socket IPC. Everything is local; no network calls beyond Discord IPC.

## Commands

```bash
# Build
cargo build --workspace --release

# Run daemon from source
cargo run --release -p devsignal-daemon -- run

# Lint — Linux can only do core + discord (macOS SDK needed for the rest)
cargo fmt --all -- --check
cargo clippy -p devsignal-core -p devsignal-discord --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings   # macOS only

# Tests (full workspace requires macOS; core tests run anywhere)
cargo test --workspace
cargo test -p devsignal-core
cargo test -p devsignal-core <test_name>

# Debug helpers (both print/validate without touching Discord)
cargo run -p devsignal-daemon -- validate --config ~/.config/devsignal/config.toml
cargo run -p devsignal-daemon -- once     --config ~/.config/devsignal/config.toml

# Guided setup wizard (writes config, optionally installs LaunchAgent)
cargo run -p devsignal-daemon -- init

# Initial config without the wizard
./scripts/setup-local-config.sh
```

CI (`.github/workflows/ci.yml`): a Linux `lint` job runs `cargo fmt --check` plus clippy scoped to
`devsignal-core` + `devsignal-discord`, and a `macos` job runs workspace clippy, `cargo test`, and a
release build. MSRV is Rust 1.74 (`workspace.package.rust-version`). `Cargo.lock` is committed.

`.github/workflows/release.yml` builds a universal macOS binary (`lipo` of aarch64 + x86_64) on `v*`
tags. `packaging/` holds the LaunchAgent plist template, `install.sh`, and a Homebrew formula.

## Architecture

Rust workspace, 4 crates under `crates/`:

| Crate | Role |
|---|---|
| `devsignal-core` | Config/TOML, agent matching, host labels, presence rules, `PresenceView`, `Debouncer` — no OS or Discord deps |
| `devsignal-macos` | `frontmost_bundle_id()` via AppKit `NSWorkspace` (`objc2`); falls back to `osascript` after two consecutive native misses |
| `devsignal-discord` | `PresenceSession` wrapping `discord-rich-presence`; `set_presence_resilient` / `clear_presence_resilient` (one reconnect on failure) |
| `devsignal-daemon` | Binary `devsignal`: hand-rolled CLI parsing, `init` wizard, config-edit subcommands, poll loop |

`devsignal-daemon` modules:

- `main.rs` — CLI enum + parsing, `run_daemon`/`run_forever` poll loop, `build_policy_view`
- `init.rs` — interactive `devsignal init` wizard (`dialoguer` + `console`): Discord app ID, privacy
  preset, agent multi-select, host multi-select, optional rule presets, then optional copy to
  `~/bin/devsignal` + LaunchAgent write + `launchctl bootstrap`/`kickstart`
- `config_edit.rs` — non-interactive config mutation for `hosts` / `agents` / `rules` subcommands

### Main loop (every `poll_interval_secs`, `run_forever` in `main.rs`)

1. `sysinfo` refreshes all processes.
2. `collect_matches` runs `process_matches_rule` for each process × each `[[agents]]` rule
   (case-insensitive process name **or** `basename(argv[0])`, plus optional `argv_substrings`
   against the joined command line). Rules disabled via `platforms.disabled_agents` are skipped
   here by `agent_allowed`.
3. `select_active_agent` picks the winner by lowest `priority`, tie-breaking on lowest PID; returns
   the `ActiveAgent` plus its PID.
4. Agent-id change (`transition`) resets `session_start_unix` to now, so the Discord elapsed timer
   restarts per agent session.
5. If no agent matched and `idle_mode = "clear"`, the loop clears presence (only on transition or
   first tick) and skips the rest of the tick.
6. `show_cwd_basename` optionally resolves the winning PID's CWD through `redact_cwd_basename`.
7. `devsignal_macos::frontmost_bundle_id()` gets the focused app's bundle ID.
8. `build_policy_view` → `apply_rules` (first matching `[[rules]]` wins) → `build_presence_view`,
   then applies the override: `hide_host` drops the host label (falling back to
   `hidden_host_state`, e.g. `Working · myrepo`), and `then.state` replaces `state` outright.
   `platforms.disabled_hosts` forces `hide_host` too.
9. `Debouncer::should_push` suppresses the Discord call when the payload is byte-identical or
   `min_push_interval_secs` hasn't elapsed; `force` is set on agent transitions and the first tick.
10. `set_presence_resilient` pushes over IPC.

On SIGINT/SIGTERM the `RUNNING` atomic flips false → loop exits → `clear_presence_resilient` runs
before exit. `connect_with_wait` retries IPC with exponential backoff up to 30s when
`--wait-for-discord` (the default) is set.

### CLI surface

```
devsignal [run] [-c path] [--wait-for-discord | --no-wait-for-discord]
devsignal init     [-c path]     # interactive wizard
devsignal validate [-c path]     # parse + validate, print agents
devsignal once     [-c path]     # print the PresenceView as JSON, no IPC
devsignal hosts  list | enable <bundle_id> | disable <bundle_id>
devsignal agents list | enable <agent_id>  | disable <agent_id>
devsignal rules  list | remove <name> | add --name <n> [--host id] [--agent id]
                                            [--project name] [--time HH:MM-HH:MM]
                                            [--active-only] [--idle-only]
                                            [--hide-host] [--state text]
```

Argument parsing is hand-rolled (no `clap`) — `parse_cli` in `main.rs`, `take_config` +
`parse_*_command` in `config_edit.rs`. A bare `devsignal --config foo` still works as legacy `run`.
Non-macOS builds of `main` exit 1 immediately; the daemon internals are `#[cfg(target_os = "macos")]`.

## Key design decisions

- `devsignal-core` is platform-free, so Linux CI can lint and test it without the macOS SDK. Keep
  OS-specific code out of it.
- Agent matching checks both `proc.name()` and `basename(argv[0])` so wrapped Node CLIs
  (e.g. `node .../codex`) still match `codex`.
- `Debouncer` compares the last pushed `PresenceView` by value equality — no hashing, just the
  derived `PartialEq`. Any new `PresenceView` field automatically participates in debouncing.
- `show_cwd_basename` redacts to the last path segment only (`redact_cwd_basename`, which also
  rejects `.` and single-component roots); full paths never reach Discord.
- `[[rules]]` are **first-match-wins**, evaluated in file order (`apply_rules`). `RuleWhen` fields
  are ANDed; within a field the list is ORed, and all string comparisons are case-insensitive.
- `TimeWindow::matches_minutes` handles overnight windows (`start > end`) by wrapping.
- Buttons are capped at 2 in `devsignal-discord` (`.take(2)`) — Discord's limit.
- Config writes (`init.rs`, `config_edit.rs`) serialize with `toml::to_string_pretty` and then
  re-load through `Config::load_from_path` to validate; comments in a hand-edited config are lost
  when a config-edit subcommand rewrites it.
- Presence assets are Discord art-asset **keys**, not URLs — they must be uploaded in the Discord
  Developer Portal (`devsignal`, `claude`, `codex`, `opencode` by default).

## Config

Default path `~/.config/devsignal/config.toml` (`Config::default_path` prefers `$HOME/.config` over
`dirs::config_dir()` so it matches the docs and scripts). `validate()` requires
`discord.client_id` to be non-empty and at least one `[[agents]]` entry; everything else has serde
defaults. See `config.example.toml` for the annotated reference.

Shape:

```toml
poll_interval_secs     = 2       # default 2
min_push_interval_secs = 20      # default 20
idle_mode              = "status"  # "status" | "clear"
show_cwd_basename      = false

[platforms]
disabled_hosts  = []   # bundle IDs to never show as host
disabled_agents = []   # agent ids to ignore entirely

[discord]
client_id   = "…"      # required
large_image = "devsignal"
large_text  = "devsignal"
# small_image / small_text — optional, used for the idle payload

[[agents]]              # ≥1 required
id = "claude_code"; label; process_names; argv_substrings
priority = 100          # lower wins
large_image; small_image; small_text
  [[agents.buttons]]    # max 2 reach Discord
  label; url

[[rules]]               # optional, first match wins
name = "…"
when = { host_bundle_ids, agent_ids, project_basenames, time = { start, end },
         active_only, idle_only }
then = { hide_host, state }
```

`HOST_BUNDLE_LABELS` in `devsignal-core` is the canonical bundle-ID → label table (editors,
JetBrains SKUs, terminals) and also drives the `init` host multi-select and `hosts list`. Add new
host apps there, with a test asserting the entry when it matters. Unknown bundle IDs fall through to
`JetBrains` / `Android Studio` prefix heuristics, then to the raw bundle ID.

## Conventions

- Errors use `anyhow` with `.context(...)`/`with_context(...)` describing the attempted action;
  subcommands return `Result<()>` and `main` maps them to exit codes (2 for CLI parse errors,
  1 for runtime failures).
- Logging is `tracing` + `tracing_subscriber` with `EnvFilter`, default `info`; override with
  `RUST_LOG`. Transient Discord IPC problems are `warn!` and swallowed, never fatal to the loop.
- Unit tests live in `#[cfg(test)] mod tests` at the bottom of the file under test; most logic lives
  in `devsignal-core` precisely so it is testable off-macOS. Config-schema changes need matching
  updates to the `sample_config()` / `rule()` helpers and the TOML round-trip tests there.
- Adding a field to `Config`/`AgentRule` means: `#[serde(default)]` (or a `default_*` fn), update
  `config.example.toml`, update `init.rs`'s `generate_config`/`default_agents`, and update the core
  test fixtures — all four, or the workspace won't compile or CI will fail.
- Conventional-commit style messages (`feat:`, `fix:`, `docs:`, `chore:`, `test:`, with scopes like
  `feat(init):`).
- `docs/superpowers/plans/` holds dated design plans for larger features; `README.md` carries the
  user-facing architecture and install docs and should be kept in sync with structural changes.

## Learned preferences

- Run builds, checks, and git/GH CLI steps locally rather than just describing commands.
- When a plan is attached and todos already exist, execute the plan and update existing todo
  statuses — don't edit the plan file or recreate todos.
- Caveman mode: follow terse style when the caveman skill is active; revert on "stop caveman" /
  "normal mode".
