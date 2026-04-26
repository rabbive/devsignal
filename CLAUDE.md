# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build --workspace --release

# Run daemon from source
cargo run --release -p devsignal-daemon -- run

# Lint (mirrors CI)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Tests (macOS only for full workspace)
cargo test --workspace

# Single test
cargo test -p devsignal-core <test_name>

# Debug helpers
cargo run -p devsignal-daemon -- validate --config ~/.config/devsignal/config.toml
cargo run -p devsignal-daemon -- once   --config ~/.config/devsignal/config.toml

# Initial config
./scripts/setup-local-config.sh
```

CI runs `fmt` + `clippy` on Linux (core + discord crates only), then full workspace build + test on macOS. MSRV is Rust 1.74.

## Architecture

Rust workspace with 4 crates under `crates/`:

| Crate | Role |
|---|---|
| `devsignal-core` | Config/TOML, agent matching, `PresenceView`, `Debouncer` — no OS or Discord deps |
| `devsignal-macos` | `frontmost_bundle_id()` via AppKit (`objc2`); falls back to `osascript` if AppKit returns nothing twice |
| `devsignal-discord` | `PresenceSession` wrapping `discord-rich-presence`; `set_presence_resilient` / `clear_presence_resilient` |
| `devsignal-daemon` | Binary entry point: CLI parsing, poll loop, wires the other three crates |

**Main loop** (every `poll_interval_secs`):
1. `sysinfo` refreshes all processes
2. `process_matches_rule` checks each process against each `[[agents]]` rule (case-insensitive name + argv0 basename + optional `argv_substrings`)
3. `select_active_agent` picks winner by lowest `priority`, tie-breaks on lowest PID
4. `devsignal_macos::frontmost_bundle_id()` gets the focused app bundle ID
5. `build_presence_view` assembles `PresenceView` (agent label, host label, optional CWD basename)
6. `Debouncer::should_push` suppresses Discord calls when payload is unchanged or `min_push_interval_secs` hasn't elapsed; agent transitions force a push
7. `set_presence_resilient` / `clear_presence_resilient` talk to Discord desktop over Unix socket IPC

On SIGINT/SIGTERM: `RUNNING` atomic flips false → loop exits → `clear_presence_resilient` called before exit.

## Key Design Decisions

- `devsignal-core` is platform-free; Linux CI can lint it without macOS SDK.
- Agent matching checks both `proc.name()` and `basename(argv[0])` so wrapped Node CLIs (e.g. `node .../codex`) still match `codex`.
- `Debouncer` tracks the last pushed `PresenceView` by value equality — no hashing, just `PartialEq` derive.
- `show_cwd_basename` redacts to the last path segment only (`redact_cwd_basename`); full paths never reach Discord.
- Release binary is a universal macOS binary (`lipo` of aarch64 + x86_64) produced by `.github/workflows/release.yml` on `v*` tags.

## Config

Default path: `~/.config/devsignal/config.toml`. Required fields: `discord.client_id`, at least one `[[agents]]` entry. See `config.example.toml`.

## Learned Preferences

- Run builds, checks, and git/GH CLI steps locally rather than just describing commands.
- Caveman mode: follow terse style when caveman skill is active; revert on "stop caveman" / "normal mode".
