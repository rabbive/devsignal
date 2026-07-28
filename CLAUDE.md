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

# Lint — only devsignal-macos needs the macOS SDK
cargo fmt --all -- --check
cargo clippy -p devsignal-core -p devsignal-discord -p devsignal-daemon --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings   # macOS only

# Tests (only devsignal-macos requires macOS)
cargo test -p devsignal-core -p devsignal-discord -p devsignal-daemon
cargo test --workspace
cargo test -p devsignal-core <test_name>

# Debug helpers (none of these touch Discord)
cargo run -p devsignal-daemon -- validate --config ~/.config/devsignal/config.toml
cargo run -p devsignal-daemon -- once     --config ~/.config/devsignal/config.toml
cargo run -p devsignal-daemon -- detect   --config ~/.config/devsignal/config.toml
cargo run -p devsignal-daemon -- detect --unmatched   # find an agent's process name
cargo run -p devsignal-daemon -- watch    --config ~/.config/devsignal/config.toml

# Check the MSRV claim (CI does this too; `stable` will not catch a violation)
cargo +1.87 check -p devsignal-core -p devsignal-discord -p devsignal-daemon --all-targets

# Lint shell scripts (CI installs shellcheck)
shellcheck packaging/macos/install.sh packaging/macos/uninstall.sh scripts/*.sh

# Regenerate the presence art in assets/discord/ (needs: pip install pillow cairosvg)
python3 scripts/build-discord-assets.py
python3 scripts/build-discord-assets.py --check   # fail if the committed PNGs drifted

# Uninstall everything (unloads the LaunchAgent first, so presence clears)
./packaging/macos/uninstall.sh            # asks before deleting the config
./packaging/macos/uninstall.sh --purge    # config too

# Guided setup wizard (writes config, optionally installs LaunchAgent)
cargo run -p devsignal-daemon -- init

# Initial config without the wizard
./scripts/setup-local-config.sh
```

CI (`.github/workflows/ci.yml`) has four jobs: `lint` (Linux) runs `cargo fmt --check`, clippy, tests,
and `shellcheck` for every crate except `devsignal-macos`; `msrv` builds against exactly the declared
`rust-version` so that claim stays checked; `macos` runs workspace clippy, `cargo test`, and a release
build; `audit` runs `cargo audit` with `continue-on-error`, so a new advisory against a transitive
dependency does **not** block merge — but it still reports a red check, which is the point: advisories
appear on their own schedule and can turn this red on a PR that changed nothing. All jobs use
`Swatinem/rust-cache` and pass `--locked`, so lockfile drift fails everywhere rather than only in `msrv`.
MSRV is Rust **1.87** (`workspace.package.rust-version`) — determined empirically; the floor comes
from the dependency graph as much as from our code. `Cargo.lock` is committed.

`.github/workflows/release.yml` builds a universal macOS binary (`lipo` of aarch64 + x86_64) on `v*`
tags, signing/notarizing when Apple credentials are present and publishing `SHA256SUMS`.
`packaging/macos/` holds the LaunchAgent plist template, `install.sh`, and `uninstall.sh`.

## Architecture

Rust workspace, 4 crates under `crates/`:

| Crate | Role |
|---|---|
| `devsignal-core` | Config/TOML, agent matching, host labels + icons, presence rules, line layout, image resolution, `PresenceView`, `Debouncer`, `RetryBackoff` — no OS or Discord deps |
| `devsignal-macos` | `frontmost_bundle_id()` via AppKit `NSWorkspace` (`objc2`); falls back to `osascript` after two consecutive native misses |
| `devsignal-discord` | `PresenceSession` wrapping `discord-rich-presence`; `set_presence_resilient` / `clear_presence_resilient` (**returning `Result`**) recover via `reestablish` — one `reconnect()`, then a plain `connect()`, because `reconnect()` cannot revive a client that never connected — all behind a `PresenceIpc` trait so the policy is testable without a Discord client |
| `devsignal-daemon` | Binary `devsignal`: hand-rolled CLI parsing, `init` wizard, config-edit subcommands, poll loop |

`devsignal-daemon` modules:

- `main.rs` — command dispatch, `run_loop`/`run_forever`/`tick`/`push`, `build_policy_view`,
  `collect_matches`, `maybe_reload_config`, and the `validate`/`once`/`detect`/`watch` commands
- `cli.rs` — argument parsing and help/version text; no platform gating, so CI lints and tests it
- `sink.rs` — `PresenceSink` (both methods return `Result`) with `DiscordSink` (real IPC),
  `StdoutSink` (backs `watch`), and `ScriptedSink` (test-only, scripted failures); the seam that makes
  the poll loop — shutdown-and-clear and the retry paths alike — testable without a Discord client
- `lockfile.rs` — advisory `flock` beside the config so a second `devsignal run` refuses to start
  rather than fighting the first over the Discord card
- `config_io.rs` — `write_config_atomic`: serialize → round-trip → validate → temp file → rename
- `init.rs` — interactive `devsignal init` wizard (`dialoguer` + `console`): Discord app ID, privacy
  preset, agent multi-select, host multi-select, optional rule presets, then optional copy to
  `~/bin/devsignal` + LaunchAgent write + `launchctl bootstrap`/`kickstart`
- `config_edit.rs` — non-interactive config mutation for `hosts` / `agents` / `rules` subcommands

### Main loop (every `poll_interval_secs`, `run_forever` in `main.rs`)

0. `maybe_reload_config` stats the config file and reloads on an mtime change. A config that no
   longer loads is warned about and **ignored**, keeping the previous one — a bad edit must not kill a
   running daemon. A reload forces the next push so the change is visible immediately. A changed
   `discord.client_id` cannot be applied without reconnecting, so it warns instead.
1. `refresh_processes` calls `refresh_processes_specifics(ProcessesToUpdate::All, true, …)`. That
   `true` is `remove_dead_processes`; `refresh_specifics` passes `false`, which is why exited agents
   used to linger in the snapshot and presence kept showing a CLI you had quit. It requests only
   `cmd` (plus `cwd` when `show_cwd_basename` is set) — name is always provided. Deliberately not
   `everything()`, which also refreshed `environ`, `exe`, memory, CPU, and disk for every process.
2. `collect_matches` runs `process_matches_rule` for each process × each `[[agents]]` rule
   (case-insensitive process name **or** `basename(argv[0])`, plus optional `argv_substrings`
   against the joined command line). Rules disabled via `platforms.disabled_agents` are skipped
   here by `agent_allowed`. Threads are skipped (`thread_kind().is_some()`): on Linux sysinfo lists
   them as processes sharing the CLI's argv[0], so one agent would match many times. No-op on macOS.
3. `select_active_agent` picks the winner by lowest `priority`, tie-breaking on lowest PID; returns
   the `ActiveAgent` plus its PID.
4. Agent-id change (`transition`) resets `session_start_unix` to now, so the Discord elapsed timer
   restarts per agent session.
5. If no agent matched and `idle_mode = "clear"`, the loop clears presence — through the same `push`
   helper as the `Set` path, which is how both share the debouncer and the retry backoff. This branch
   used to bypass the debouncer entirely, and later had its own copy of the wedge in step 9.
6. `show_cwd_basename` optionally resolves the winning PID's CWD through `redact_cwd_basename`.
7. `devsignal_macos::frontmost_bundle_id()` gets the focused app's bundle ID. AppKit first; after two
   consecutive misses it falls back to a **time-boxed** `osascript` (2s, killed on timeout) with
   exponential backoff on repeated failure. Untimed, a wedged `System Events` blocked the whole loop —
   and therefore shutdown and the final clear.
8. `build_policy_view` → `apply_rules` (first matching `[[rules]]` wins) → `build_presence_view`
   (via `PresenceInputs`), then applies the override: `then.state` replaces `state` outright.
   `hide_host` — set by a rule or by `platforms.disabled_hosts` — is passed *into* core as
   `PresenceInputs::hide_host`, where the `Host` line slot renders the neutral fallback
   (`Working · myrepo`) and the host icon is suppressed. Returns the matched rule name alongside the
   view — **not** on `PresenceView`, which is the debouncer's equality key.
9. `push` makes one attempt, in this order — and the order is the whole point:
   1. `RetryBackoff::ready` — skip entirely while waiting out a previous failure.
   2. `Debouncer::may_send(action, force)` — suppresses an unchanged payload or one inside
      `min_push_interval_secs`, and enforces a sliding window of 5 sends per 20s (Discord's documented
      limit) **even when `force` is set**. `force` is set on agent transitions, the first tick, after a
      config reload, and — critically — whenever `backoff.consecutive_failures() > 0`, because an
      unacknowledged push means the dedupe key no longer describes what Discord is showing. `may_send`
      does **not** record.
   3. The `PresenceSink`, which now returns `Result`: Discord IPC under `run`, stdout under `watch`.
   4. On `Ok`, `Debouncer::record_sent` **and** `RetryBackoff::record_success`; on `Err`,
      `RetryBackoff::record_failure` and nothing recorded in the debouncer.

   Step 4 is the fix for the wedge that made quitting Discord kill presence permanently: recording at
   step 2 marked a failed push as delivered, so the next tick deduplicated against a payload Discord
   never received, the sink was never called again, and the reconnect inside it never fired. Leaving
   the dedupe key untouched on failure is what lets the next tick retry; `RetryBackoff` (400ms → 60s,
   doubling) is what stops that retry from reopening a dead socket every `poll_interval_secs`.
   Recovery needs no separate code path — the current view is simply what lands once a send succeeds.
10. Failures log on **transition only**: `warn!` on the first, `debug!` after, `info!` on recovery. The
    launchd log has no rotation, so a day with Discord closed would otherwise fill it.

One tick is `tick()`, extracted from `run_forever` so these failure paths are unit-testable without
subprocesses or signals. On SIGINT/SIGTERM the `RUNNING` atomic flips false → loop exits → the sink
clears. That final clear is the one deliberate bypass of both the debouncer *and* the backoff — a skip
or a retry there would delay exit. `sleep_interruptible` sleeps in 200ms slices so shutdown does not
wait out `poll_interval_secs` (which has no upper bound). `connect_with_wait` retries IPC with
exponential backoff up to 30s when `--wait-for-discord` (the default) is set, and a timeout there is
**not** fatal: launchd starts devsignal before Discord finishes launching at login, so exiting 1 meant
`KeepAlive` respawning every 10s forever. The loop retries instead. `--no-wait-for-discord` still fails
fast for scripts.

`run` also takes an advisory `flock` beside its config (`lockfile.rs`) and refuses to start if another
daemon holds it — two daemons would both push and fight over the card. `watch` deliberately does not.

Three integration tests cover this loop, all running on Linux: `tests/shutdown.rs` (SIGTERM and SIGINT
each exit 0 *and* emit a clear), `tests/detection.rs` (an agent is released when it exits), and
`tests/hot_reload.rs` (an invalid edit is ignored without killing the daemon). The retry paths are
covered by unit tests driving `tick()` with a `ScriptedSink` instead, because a sink reached through
the shipped binary cannot be made to fail without a production test hook. Each was verified to fail
when its fix is reverted — reverting the record-on-confirmation split drops
`a_failed_push_is_retried_on_a_later_tick` from 3 attempts to 1.

### CLI surface

```
devsignal [run] [-c path] [--wait-for-discord | --no-wait-for-discord]
devsignal init     [-c path]     # interactive wizard
devsignal validate [-c path]     # parse + validate, print agents/rules/platforms
devsignal once     [-c path]     # print the PresenceView as JSON, no IPC
devsignal detect   [-c path]     # matching processes, the winner, the matched rule
devsignal detect --unmatched     # processes no rule matched (--all skips the filter)
devsignal watch    [-c path]     # the real poll loop, printing instead of using Discord
devsignal version | --version | -V
devsignal hosts  list | enable <bundle_id> | disable <bundle_id>
devsignal agents list | enable <id> | disable <id> | remove <id> |
                 add --id <id> --process-name <name> [--label t] [--priority n]
                     [--large-image k] [--small-image k] [--small-text t]
                     [--button "Label=URL"]
devsignal rules  list | remove <name> | add --name <n> [--host id] [--agent id]
                                            [--project name] [--time HH:MM-HH:MM]
                                            [--active-only] [--idle-only]
                                            [--hide-host] [--state text]
```

Argument parsing is hand-rolled (no `clap`) — `parse_cli` in `cli.rs`, `take_config` +
`parse_*_command` in `config_edit.rs`. A bare `devsignal --config foo` still works as legacy `run`.
`cli.rs` is deliberately free of `cfg(target_os)` so it stays lintable and testable everywhere;
`--help`/`--version` return variants rather than calling `process::exit`, and explicit `--help` goes
to stdout. Platform gating is a runtime check in `require_macos`, applied only to `run` and `init` —
`validate`, `once`, `detect`, and the config-edit subcommands work anywhere.

## Key design decisions

- `devsignal-core` is platform-free, so Linux CI can lint and test it without the macOS SDK. Keep
  OS-specific code out of it.
- Agent matching checks both `proc.name()` and `basename(argv[0])` so wrapped Node CLIs
  (e.g. `node .../codex`) still match `codex`.
- `Debouncer` compares the last sent action by value equality — no hashing, just the derived
  `PartialEq`. Any new `PresenceView` field automatically participates in debouncing, which is exactly
  why `matched_rule_name` is **not** a field on it: a rule-name change alone would trigger a Discord
  write with identical visible text. It also owns the 5-per-20s rate limit, which applies to forced
  sends too — `force` keeps transitions responsive, it does not license unbounded IPC traffic.
- Deduplication **expires** after `REASSERT_INTERVAL` (60s), so an unchanged payload is re-sent rather
  than suppressed forever. Dedupe assumes Discord still shows the last confirmed payload, and nothing
  signals when that stops being true — a Discord that quit and reopened has no activity and no way to say
  so. Without the expiry a user with a static view (one terminal, unchanged host label) never got
  presence back, and the daemon never even attempted a send to *discover* Discord was gone. A re-assert
  still respects `min_push_interval_secs`, so the effective period is `max(60s, min_interval)`.
- The dedupe key is also bypassed while a failure is outstanding (see main-loop step 9.2). Trusting it
  during an outage reopens the wedge through a narrower door: send A, fail on B, let the view return to
  A, and the retry is deduplicated against a payload Discord may never have received.
- The debouncer is **two-phase**: `may_send` asks, `record_sent` confirms, and only the daemon's success
  branch calls the second. Recording at check time is a silent-wedge generator — the payload is marked
  delivered whether or not it arrived. A failed send therefore records *nothing*, not even a rate-limit
  slot: it consumed none of Discord's budget. A caller that forgets `record_sent` loses dedupe entirely
  and writes every tick, which is why `a_successful_push_is_still_debounced` exists alongside the retry
  tests.
- `show_cwd_basename` redacts to the last path segment only (`redact_cwd_basename`, which also
  rejects `.` and single-component roots); full paths never reach Discord.
- Discord's card is three lines: **activity name**, `details`, `state`. The name line is the Discord
  *application's* name unless the payload sets `name`, which is why the agent used to be stuck on
  line 2. `[presence]` assigns one of `agent | host | project | brand | off` to each slot
  (`PresenceLine`), rendered by `render_line` in core; the defaults (`off` / `agent` / `host`)
  reproduce the pre-0.4 layout byte for byte, including the idle and hidden-host strings. Whether
  Discord honours the `name` override is client-dependent and **unverified on a real client** — the
  documented fallback is renaming the application in the Developer Portal.
- Presence images are resolved by `ImagesConfig::resolve`, not passed through raw: `mode = "url"`
  expands a bare name to `{base_url}/{agents|hosts}/{name}.png` (Discord accepts an image URL wherever
  it accepts an art-asset key), `mode = "key"` leaves it alone, and an absolute `http(s)` value is
  always passed through. `assets/discord/` holds the PNGs, generated by
  `scripts/build-discord-assets.py`; `every_host_icon_name_has_a_png` and
  `every_preset_agent_image_has_a_png` fail if a name in the code has no file, because the runtime
  symptom is a blank tile with nothing in the logs. **`mode = "url"` is likewise unverified against a
  real Discord client** — it cannot be worse than an un-uploaded key, which renders blank too.
- The small (corner) image is the **host** icon when `images.host_icon` is on, falling back to the
  agent's `small_image`. It follows the host *label*: anything that hides the label hides the icon, or
  `hide_host` would leak the app through its image.
- `[[rules]]` are **first-match-wins**, evaluated in file order (`apply_rules`). `RuleWhen` fields
  are ANDed; within a field the list is ORed, and all string comparisons are case-insensitive.
- `TimeWindow::matches_minutes` handles overnight windows (`start > end`) by wrapping.
- `Config::validate` enforces Discord's real limits at load time (button label ≤32 chars, url ≤512
  and `http(s)`, ≤2 buttons, numeric `client_id`, parseable `TimeWindow`) and rejects rules that can
  never match or that would shadow later rules. The failure mode it prevents is silent: Discord
  rejects an oversized payload wholesale and presence just stops updating, with one `warn!` in a log
  file. `devsignal-discord` still has `.take(2)` as a backstop, but >2 is a config error now.
- Config writes go through `config_io::write_config_atomic` — validate before replacing, temp file
  plus `rename`, never a bare `fs::write`. Comments and key order are still lost when a config-edit
  subcommand rewrites a hand-edited file, and the subcommands now say so.
- Agent presets live in `devsignal-core::agent_presets()`, consumed by the `init` wizard; a core test
  asserts `config.example.toml` and the preset table describe the same agent ids in both directions.
  **Only agents with process names confirmed on a real machine belong there** — adding one is a claim
  that `devsignal detect` was run against it. Unconfirmed agents go in `docs/community-presets.md`,
  whose TOML snippets are themselves validated by a core test. A preset that never matches makes the
  daemon look broken with no way to tell why, which is the failure mode all the validation work exists
  to prevent.
- Platform gating is a runtime check (`require_macos`), not `cfg`, so CI can lint and test everything
  except `devsignal-macos`. Only `run` and `init` are gated.
- Presence assets are Discord art-asset **keys**, not URLs — they must be uploaded in the Discord
  Developer Portal. Each preset's key is its id, so the defaults are `devsignal`, `claude_code`,
  `codex`, `opencode`, `cursor_agent` — `preset_asset_keys()` is the authoritative list and is what
  `init` prints.

## Config

Default path `~/.config/devsignal/config.toml` (`Config::default_path` prefers `$HOME/.config` over
`dirs::config_dir()` so it matches the docs and scripts). Everything except `discord.client_id` and
`[[agents]]` has a serde default. See `config.example.toml` for the annotated reference — note the
shipped example has a placeholder `client_id`, so it deliberately fails `validate` until edited.

`validate()` rejects: a non-numeric `client_id`; an empty `agents` list; an agent with no
`process_names` (it could never match); duplicate agent ids or rule names; more than 2 buttons; a
button label over 32 chars, a url over 512 chars, or a url without an `http(s)` scheme; a
`TimeWindow` that is not `HH:MM`; a rule with both `active_only` and `idle_only`; a rule whose
`then` does nothing; two `[presence]` slots carrying the same value; a `project` slot without
`show_cwd_basename`; an empty `brand_text` behind a `brand` slot; and (in url mode only) a
`base_url` that is empty, not `http(s)`, or over 400 chars.

Shape:

```toml
poll_interval_secs     = 2       # default 2
min_push_interval_secs = 20      # default 20
idle_mode              = "status"  # "status" | "clear"
show_cwd_basename      = false

[presence]              # which line shows what; "agent"|"host"|"project"|"brand"|"off"
name                   = "off"     # line 1 — "off" leaves Discord's application name
details                = "agent"   # line 2
state                  = "host"    # line 3
brand_text             = "devsignal"

[images]
mode                   = "key"     # "key" = uploaded art assets, "url" = hosted PNGs
base_url               = "https://raw.githubusercontent.com/rabbive/devsignal/main/assets/discord"
host_icon              = false     # frontmost editor/terminal as the small image

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

`HOST_APPS` in `devsignal-core` is the canonical bundle-ID → (label, icon) table (editors, JetBrains
SKUs, terminals) and also drives the `init` host multi-select and `hosts list`. Add new host apps
there **with an icon in `assets/discord/hosts/`**, plus a test asserting the entry when it matters.
Unknown bundle IDs fall through to `JetBrains` / `Android Studio` prefix heuristics, then to the raw
bundle ID for the label and `macos` for the icon (`host_image_for_bundle` never returns nothing — a
raw bundle id would be a 404 in url mode).

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
  `config.example.toml`, update `init.rs`'s `generate_config`, update `agent_presets()` in core if
  agents are affected, and update the core test fixtures (`sample_config`/`valid_config`) — or the
  workspace won't compile and the `config_example_covers_every_preset_id` drift test will fail.
- Adding an agent preset means editing `agent_presets()` **and** `config.example.toml` **and** adding
  `assets/discord/agents/<id>.png` (via the `AGENTS` table in `scripts/build-discord-assets.py`); the
  drift tests check every direction, so none can be forgotten.
- Conventional-commit style messages (`feat:`, `fix:`, `docs:`, `chore:`, `test:`, with scopes like
  `feat(init):`).
- `docs/superpowers/plans/` holds dated design plans for larger features; `README.md` carries the
  user-facing architecture and install docs and should be kept in sync with structural changes.
- **v0.3.0 shipped** on 2026-07-25 (tag `v0.3.0`, GitHub release with a universal tarball,
  `SHA256SUMS`, and the plist). It was published **unsigned**: the release workflow took its
  zero-secrets branch, and the shipped binary's x86_64 slice carries no `LC_CODE_SIGNATURE` while the
  arm64 slice carries only the linker's mandatory ad-hoc one. Treat unsigned as the shipping reality
  until the five secrets below exist.
- These remain **unverified** because each needs a Mac with a real Discord client, and none should be
  assumed done:
  1. **Discord restart recovery** (0.4.0's headline fix). Quit Discord while the daemon runs, wait past
     `min_push_interval_secs`, reopen it → presence must come back on its own. No longer depends on
     `DiscordIpcClient::reconnect()` behaving: `reestablish` in `devsignal-discord` falls back to a
     plain `connect()` when `reconnect()` fails. That fallback is **required**, not belt-and-braces —
     `reconnect()` starts with `close()`, which returns `Err(NotConnected)` when there is no socket, so
     it can never recover a client that never connected.
  2. **Login with Discord not running** → the daemon stays up and connects once Discord appears, with
     no respawn loop in the log.
  3. `launchctl bootout gui/$(id -u)/com.devsignal.daemon` → presence clears in the actual client. The
     launchd → Discord loop is **half confirmed**: presence *appearing* was seen on a 0.3.0 build
     (`devsignal / Claude Code / In Ghostty`, which is what prompted the `[presence]` work); clearing
     was not.
  4. **Two instances**: `devsignal run` while the LaunchAgent is loaded → refused, message names the
     holder's pid. Only the single-process flock semantics are covered by tests, because `run` is
     `require_macos`-gated.
  5. **Whether Discord honours the activity `name` override.** `presence.name = "agent"` sets the
     activity `name` field, which `discord-rich-presence` documents as overriding the application
     name; no client has been checked. If line 1 still reads `devsignal`, the fallback is renaming the
     application in the Developer Portal, and the docs say so. Check with `devsignal run` and one
     glance at the card.
  6. **Whether Discord renders `mode = "url"` images**, for the large slot and the small one. External
     URLs are documented and used by comparable tools, but blank tiles are the failure mode either
     way, so this cannot regress an un-uploaded key setup. Verify both slots before recommending url
     mode as *the* path in the README rather than one of two.
  7. `install.sh` against a real release on a machine that has never run the binary (the Gatekeeper
     path), then `uninstall.sh`.
  8. The ten process names in `docs/community-presets.md` are inferred and have never been run.
     Confirm with `devsignal detect --unmatched` while each CLI is running, then promote into
     `agent_presets()` **and** `config.example.toml` **and** `assets/discord/agents/<id>.png` — the
     drift tests check every direction, so none can be forgotten.
  9. Signing and notarization have never executed. They need five repo secrets —
     `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD`, `APPLE_NOTARY_API_KEY`, `APPLE_NOTARY_KEY_ID`,
     `APPLE_NOTARY_ISSUER_ID` — all five or none, since a partial set is a hard error by design. Use
     the release workflow's `workflow_dispatch` trigger to dry-run the whole pipeline first, so the
     first real tag is not also signing's first execution.

  To cut a release: date the `CHANGELOG.md` heading and bump `Cargo.toml` together — the release
  workflow fails if the tag disagrees with the crate version.
- Settled during 0.4.0; do not re-litigate:
  - launchd's `ThrottleInterval` **defaults to 10s**, so a missing key was a respawn every 10s, not an
    unthrottled storm.
  - `Instant` on macOS does **not** advance across sleep, so the debouncer's rate-limit window looks
    *fresh* after a wake rather than ancient, and self-corrects within one window. Nothing to fix.
  - `session_start_unix` does over-count across sleep (it is wall clock), but fixing it needs IOKit
    sleep/wake notifications that cannot live in platform-free `devsignal-core`. Cosmetic; skipped.
  - `KeepAlive` stays `true` rather than a `SuccessfulExit` dict: a Rust panic exits 101, so exit codes
    cannot separate "crashed, restart" from "misconfigured, don't". `install.sh` validates the config
    before bootstrapping instead.

## Learned preferences

- Run builds, checks, and git/GH CLI steps locally rather than just describing commands.
- When a plan is attached and todos already exist, execute the plan and update existing todo
  statuses — don't edit the plan file or recreate todos.
- Caveman mode: follow terse style when the caveman skill is active; revert on "stop caveman" /
  "normal mode".
