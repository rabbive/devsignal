# Changelog

All notable changes to devsignal. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semver.

## [Unreleased]

### Fixed

- Agent rules can exclude command-line substrings, preventing Claude Desktop from being detected as
  Claude Code and preventing Cline's background hub daemon from keeping a false presence alive.

## [0.4.0] - 2026-07-28

The presence-layout and art work had been sitting on `main` unreleased since 0.3.0. Shipping it turned
up one more instance of the failure mode 0.3.0 was mostly about: the daemon staying alive while
silently no longer doing its job.

### Fixed

- **Quitting and reopening Discord no longer kills presence permanently.** The debouncer recorded a
  payload as sent *before* the sink was called, and the sink returned `()`, so a failed push was
  indistinguishable from a delivered one. The next tick deduplicated against a payload Discord never
  received, the sink was never called again, and the reconnect inside `set_presence_resilient`
  therefore never fired — presence stayed dead until the active agent happened to change.
  `idle_mode = "clear"` had the same bug by the same mechanism. `Debouncer::should_send` is now
  `may_send` (which does not record) plus `record_sent`, called only once the sink confirms delivery.
  A failed send records nothing — not even a rate-limit slot, since it consumed none of Discord's
  budget — which is what lets the next tick retry.
- **Deduplication now expires after 60 seconds**, so an unchanged payload is re-asserted instead of
  suppressed forever. Dedupe assumed Discord still showed the last confirmed payload, and a Discord that
  quit and reopened has no activity and no way to say otherwise — so a user with a static view (sitting
  in one terminal, host label unchanged) got no presence back at all, and the daemon never even attempted
  a send to discover Discord had gone. This bounds recovery from *any* divergence — restart, sleep, a
  dropped socket — to a minute, at one write per minute against Discord's 15/minute budget. A re-assert
  still respects `min_push_interval_secs`.
- **A retry is no longer deduplicated against the last successful payload.** A failed send records
  nothing, so `last_sent` keeps the last *delivered* payload; if the view changed, failed, and then
  changed back, the retry matched that key and was suppressed, freezing the retry loop with the failure
  state never cleared. The dedupe check is now bypassed while a failure is outstanding.
- **A new `RetryBackoff` bounds that retry** (400ms doubling to 60s), so a closed Discord is not
  reopened every `poll_interval_secs`. It gates every failure, not just an absent client: a payload
  Discord actively rejects is indistinguishable at that layer.
- **A never-connected session can now recover.** `DiscordIpcClient::reconnect` begins with `close()`,
  which returns `Err(NotConnected)` when there is no socket, so it never reaches `connect_ipc` and cannot
  recover a client that never connected — exactly the state left by the non-fatal startup timeout below.
  The resilient helpers now fall back to a plain `connect()` when `reconnect()` fails. Without it the
  daemon would stay alive holding the instance lock and never publish anything.
- **A startup connect timeout is no longer fatal** under `--wait-for-discord` (the default). launchd
  starts devsignal before Discord finishes launching at login, so the old exit 1 meant `KeepAlive`
  respawning the daemon every 10 seconds indefinitely — the *normal* login sequence, not an edge case.
  `--no-wait-for-discord` keeps its fail-fast contract for scripts.
- **The LaunchAgent throttles respawns** (`ThrottleInterval` 60, against launchd's 10s default), and
  `install.sh` now runs `devsignal validate` before `launchctl bootstrap` so a config that cannot load
  is never installed as a job in the first place. Exit codes cannot tell launchd that a failure is
  permanent — a Rust panic exits 101 like anything else — so both ends are needed.
- **`install.sh` no longer requires `python3`.** The LaunchAgent template carries `--config` with a
  placeholder, so `sed` alone finishes the job. The old `plistlib` pass ran *after* `sed` had written
  the file, so on a machine without `python3` it failed leaving a plist with no `--config` at all.
- **`curl | bash` upgrades restart the running LaunchAgent.** With no TTY the script exited before the
  `launchctl` block, installing the new binary while the old one kept running until next login, with
  nothing saying so.
- Download failures get a retry and a message naming the release page instead of a bare `curl` error.

### Added

- **`devsignal run` refuses to start when another instance is running.** Two daemons both push to the
  same Discord client and fight over the card, and each one's debouncer believes its own view is what
  Discord is showing. An advisory `flock` beside the config detects it; the error names the lock path,
  the holder's pid, and how to stop it. Scoped to the config's directory, so running a second daemon
  against a different config stays possible on purpose. `watch` does not take the lock.
- **`packaging/macos/uninstall.sh`** — there was no uninstall path anywhere: no script, no subcommand,
  no docs. It unloads the LaunchAgent first, so the daemon clears Discord presence on its way out, then
  removes the plist, binary and logs. Your config is kept unless you pass `--purge`. If the agent is
  loaded but cannot be unloaded, it removes **nothing** and says so: deleting a running binary succeeds
  on Unix, so carrying on would leave the daemon publishing presence while the script claimed to have
  stopped it.
- **A Troubleshooting section in the README**, covering presence not showing, Discord not detected,
  blank image tiles, an undetected agent CLI, the Automation permission prompt, and where the logs are.
- `devsignal-discord` has tests for the first time, via a `PresenceIpc` trait seam — the crate was
  untestable because `PresenceSession` wraps a concrete `DiscordIpcClient` with no fake.

### Changed

- `PresenceSink::set`/`clear` and both `*_resilient` helpers return `Result`. The helpers report
  failures rather than logging and swallowing them, so the caller can act on them.
- Presence failures log on **transition only** — `warn!` on the first, `debug!` after, `info!` on
  recovery — and the LaunchAgent's baked-in `RUST_LOG` drops from `info` to `warn`. launchd appends to
  its log files forever with no rotation, so a day with Discord closed would otherwise fill them.
- `run_forever`'s body is extracted into `tick()`, and the duplicated `Set`/`Clear` debouncer blocks
  collapse into one `push()`, so both paths share the gate and the failure paths are unit-testable
  without subprocesses or signals.
- CI caches builds (`Swatinem/rust-cache`), passes `--locked` in every job rather than only `msrv`,
  tests `devsignal-discord`, shellchecks `uninstall.sh`, and runs `cargo audit` — 157 packages had
  nothing checking them for advisories. The audit job does not block merge (`continue-on-error`), but it
  does report a red check on a finding, since advisories can appear on a PR that changed nothing.
- The README no longer contradicts itself about code signing. Published releases are **unsigned**; the
  release workflow implements signing and notarization but is gated on five secrets that are not set.

### Added — the presence layout and art work, unreleased since 0.3.0

- **`[presence]`: choose what each line of the Discord card shows.** Discord draws three lines and the
  first is the *application's* name, which is why presence read `devsignal / Claude Code / In Ghostty`
  — the agent could never be on top. `presence.name`, `presence.details`, and `presence.state` each
  take `agent`, `host`, `project`, `brand`, or `off`; `name` overrides the activity name, so
  `name = "agent"`, `details = "host"`, `state = "brand"` yields
  `Claude Code / In Ghostty / devsignal`. Defaults reproduce the previous layout exactly. Two slots
  carrying the same value, `project` without `show_cwd_basename`, and an empty `brand_text` behind a
  `brand` slot are all load-time errors.
- **`off` omits a line** instead of sending an empty string, which Discord rendered as a blank row.
- **Presence art for every agent and host app** — 45 512×512 PNGs in `assets/discord/`, generated by
  `scripts/build-discord-assets.py` from CC0 simple-icons glyph data vendored in
  `assets/discord/sources.json`. Apps whose logos simple-icons does not carry (VS Code, Apple
  Terminal, Kitty, Tabby, RustRover, Fleet, Nova, Codex) get a monogram tile rather than a traced
  look-alike.
- **`[images]`: hosted images, so nothing has to be uploaded.** Discord accepts a plain `https://`
  image URL wherever it accepts an art-asset key, so `mode = "url"` expands every image name to
  `{base_url}/{agents|hosts}/{name}.png`. `mode = "key"` (the default) is the previous behaviour.
  An image value that is already an absolute `http(s)` URL is passed through in either mode.
- **`images.host_icon`: the frontmost editor or terminal as the small corner icon.** Host apps had no
  image at all before — the small slot was a second copy of the `devsignal` mark. Suppressed whenever
  the host label is hidden (`platforms.disabled_hosts`, a rule's `hide_host`), so hiding the label
  cannot leak the app through its icon.
- `devsignal validate` prints the line layout and image mode; `devsignal detect` prints all three
  lines in card order plus the resolved image URLs. `devsignal hosts list` gained an icon column.
- `devsignal init` asks for the line order and the image source, and prints the right asset list for
  the mode chosen.

### Changed — by that same presence work

- `HOST_BUNDLE_LABELS` is now `HOST_APPS`, a struct table carrying each host's icon name alongside its
  label. Two tests assert every icon name in the code has a PNG on disk, in both directions.
- `build_presence_view` takes a `PresenceInputs` struct, and the hidden-host fallback text (`Working`)
  moved from the daemon into core with the rest of the line rendering.
- `PresenceView.details` / `.state` are now `Option<String>`, joined by a new `.name`.
## [0.3.0] - 2026-07-25

First release since 0.2.0 (2026-04-19). Three features had been sitting on `main` unreleased for
months — anyone installing the documented way was getting the April build — and the work of shipping
them turned up a series of silent failures underneath.

### Breaking

- **Config validation is enforced at load.** Configs that Discord was already rejecting, or that
  relied on a third button being silently truncated, now fail to load with a specific error.
- **Unknown config keys are rejected** (`serde(deny_unknown_fields)`). A stray or misspelled key used
  to parse cleanly and be ignored; it is now an error naming the field.
- **Presence sends are rate-limited to 5 per 20 seconds**, Discord's documented RPC limit, including
  updates that were previously forced through unconditionally.

### Fixed

- **`cargo test` failed on Apple Silicon.** The detection integration test built its fake agent by
  copying `/bin/sleep`, but that binary is `arm64e`, and a copy of an Apple platform binary loses its
  platform trust outside its SIP-protected location — the kernel SIGKILLs it on exec, so no agent
  process ever existed and the test timed out after 25s. It now symlinks instead, exercising the
  argv[0] matching branch. Green on x86_64 CI the whole time, red on every M-series Mac.
- **Presence got stuck in Discord after a launchd shutdown.** `ctrlc` was declared without the
  `termination` feature, so only SIGINT was trapped. `launchctl bootout` and `kickstart -k` — exactly
  what `devsignal init` installs — send SIGTERM, so the daemon exited without clearing presence. The
  README claimed SIGTERM was handled in three places. Now covered by an integration test that asserts
  both a clean exit and that the clear actually runs.
- **Exited agents were never released.** `System::refresh_specifics` internally passes
  `remove_dead_processes: false`, so once an agent CLI was seen it stayed in the process snapshot for
  the daemon's lifetime: presence kept showing an agent you had quit, `idle_mode = "clear"` never
  fired, and the elapsed timer never reset. Measured directly — an agent flapping in and out over 24
  seconds produced exactly one transition; after the fix, twelve.
- **A hung `osascript` blocked shutdown, not just one poll tick.** The AppleScript host-detection
  fallback ran with no timeout on the daemon's only thread, and the loop checks its stop flag between
  ticks — so a wedged `System Events` (typically a pending Automation prompt) prevented both exit and
  the final clear. Now time-boxed to 2 seconds and killed on timeout.
- **The AppleScript fallback re-forked every other poll, forever.** The native-miss streak was reset
  *before* invoking the fallback, so the fallback's own failure never fed back into the decision. It
  now backs off exponentially (30s to a 300s cap), resets on success, and warns once when it engages,
  so a pending Automation prompt is discoverable rather than silent.
- **Forced updates ignored the rate limit, and clears skipped the debouncer entirely.** A flapping
  agent made every tick a transition, producing one Discord write per poll indefinitely; separately,
  the `idle_mode = "clear"` branch never consulted the debouncer at all. Both now share one limiter.
  Shutdown still bypasses it deliberately — clearing on exit must never be throttled away.
- **Shutdown waited out the poll interval.** `std::thread::sleep` is not interrupted by signal
  delivery, so `poll_interval_secs = 30` meant a 30-second Ctrl+C. The loop now sleeps in slices.
- **The declared MSRV was wrong**: `rust-version` said 1.74 while the workspace required 1.87, and
  every CI job used `stable` so nothing checked it. Determined empirically — 1.82 fails on
  `indexmap`'s edition2024, 1.85 on `discord-rich-presence`'s inherent `str::from_utf8` — and now
  enforced by an `msrv` job that reads the declared value and builds against exactly it.
- **Signing was gated on one of the five secrets it needs.** Setting the certificate without the
  notary credentials would sign successfully and then fail at notarization, after the artifact was
  built. A partial set is now a hard error: all five, or none.
- **The example LaunchAgent plist was not well-formed XML.** Its comment contained `--` (from
  `cargo build --release`), which XML forbids. `launchd` tolerated it; `plutil -lint` and every strict
  parser do not. Paths containing `&`, `<`, or `>` also produced a malformed plist.
- `devsignal once` now resolves the project folder name the way `run` does. It previously passed
  `None` unconditionally, so a rule using `when.project_basenames` could never match under `once` —
  the command could not reproduce what the daemon published.
- `devsignal hosts --help` and `devsignal rules add --help` print usage and exit 0; they used to exit
  2 or die with `unknown rules add flag: --help`. Explicit `--help` now goes to stdout so it pipes.
- Logs moved to stderr, so `once`'s JSON and `watch`'s output are pipeable, and ANSI colour is
  disabled when stderr is not a terminal — launchd log files were collecting escape codes.
- Threads are excluded from agent matching. On Linux sysinfo lists them as processes sharing the CLI's
  `argv[0]`, so one running agent matched a dozen times.
- `config.example.toml` and the README described `argv_substrings` as requiring *all* substrings to
  match; the implementation requires *any*, and always has.

### Added

- Rich presence **small assets and up to 2 buttons** per agent (built in April, unreleased until now).
- The interactive **`devsignal init` wizard** (built in April, unreleased until now).
- The **`[[rules]]` presence engine** and `[platforms]` toggles, with the `hosts`/`agents`/`rules`
  subcommands (built in April, unreleased until now).
- **`devsignal detect`** — every process matching an agent rule with its pid, name, and `argv0`, then
  which one wins and why. When nothing matches it lists the rules it searched. This is the answer to
  "why isn't my agent detected?"; `agents list` only ever showed *configured* agents.
- **`devsignal detect --unmatched`** (and `--all`) — running processes that matched no rule, filtered
  to plausible user-installed binaries. This is how you find an agent CLI's real process name.
- **`devsignal watch`** — the real poll loop, printing each payload instead of talking to Discord.
  Fills a debugging gap (`once` gives a single sample) and is what makes the shutdown path testable
  without a Discord client.
- **`devsignal agents add` / `agents remove`** — manage agent rules without hand-editing TOML.
  `remove` also drops any stale `disabled_agents` entry, which would otherwise silently suppress a
  later re-add.
- **Config hot-reload.** The daemon picks up an edited config on its next poll. An edit that no longer
  loads is reported and ignored, keeping the previous configuration — a typo must not take down a
  running daemon. A changed `discord.client_id` still needs a restart, and says so.
- **The matched rule is visible.** `detect` names it, `once` emits it as a JSON field, and the push
  path logs it. It was previously computed and read by nothing but one unit test.
- **`--version` / `-V` / `version`.** Nothing referenced `CARGO_PKG_VERSION` before, so there was no
  way to smoke-test an install.
- Releases publish a **`SHA256SUMS`** asset and are **signed and notarized** when the maintainer's
  Apple credentials are configured. Without them the workflow publishes an unsigned binary and says so
  in the run log. A `workflow_dispatch` trigger exercises the whole path without cutting a tag.
- The release workflow fails if the tag disagrees with the `Cargo.toml` version.
- `devsignal --help` documents all nine `rules add` flags, which previously appeared only in the README.
- **Cursor Agent ships as a default preset**, confirmed with `devsignal detect --unmatched` on macOS.
  Its CLI is a bash wrapper ending in `exec -a "$0" node …`, so the process name is `node` and only
  the argv[0] basename identifies it — the shape a regression test now pins.
- **Claude Desktop (`com.anthropic.claudefordesktop`) is a known host.** It previously fell through
  to the raw bundle id, so presence read `In com.anthropic.claudefordesktop` for anyone running
  Claude Code inside the desktop app.

### Changed

- **Presets ship only for agents with confirmed process names**: `claude_code`, `codex`, `opencode`.
  Ten others — Gemini CLI, Amp, Cursor Agent, Copilot CLI, Aider, Crush, Qwen Code, Droid, Cline,
  Goose — are opt-in snippets in [docs/community-presets.md](docs/community-presets.md), because a
  preset that silently never matches is worse than no preset: the daemon looks broken and the user
  cannot tell why. A test validates every snippet in that document.
- **Config writes are atomic**: serialize, round-trip, validate, write a sibling temp file, rename.
  Both writers previously used a plain `fs::write` and validated *afterwards*, so an interrupted write
  truncated the config and an invalid one was reported after the good file was gone.
- **Privacy presets are behaviourally distinct.** `Public/OSS` was advertised as "polished copy" but
  was byte-identical to `Minimal`. They are now Minimal (agent only, host hidden via a catch-all rule
  appended *after* any user-chosen rule so it cannot shadow one), Balanced, and Detailed.
- **`install.sh` works standalone.** It read `config.example.toml` and the plist from a repo clone, so
  `curl | bash` silently skipped both; it now fetches them for the release tag, verifies the download
  against `SHA256SUMS`, clears the Gatekeeper quarantine attribute, passes `--config` in the generated
  plist, and skips the interactive LaunchAgent prompt when there is no TTY.
- The process refresh asks only for `cmd` (plus `cwd` when `show_cwd_basename` is set) instead of
  `ProcessRefreshKind::everything()`, which refreshed `environ`, `exe`, `root`, memory, CPU, and disk
  usage for every process on the machine every 2 seconds.
- `devsignal validate` prints small assets, buttons, rules, and platform toggles. Missing-config
  errors consistently point at `devsignal init`.
- The `hosts`/`agents`/`rules` subcommands no longer tell you to restart the daemon — hot-reload makes
  that unnecessary — but do warn that comments are not preserved.
- CLI parsing moved to `cli.rs` with no platform gating, so CI lints and tests it. `validate`, `once`,
  `detect`, `watch`, and the config-edit subcommands run on any platform; `run` and `init` keep a
  runtime macOS check. The workspace is warning-free on Linux (was 58 warnings).
- Tests: 24 → 114, including the repo's first integration tests and its first tests for
  `devsignal-macos`. CI gained `msrv` and `shellcheck` jobs.

### Removed

- **The Homebrew formula.** It was a template pinned to the v0.2.0 tarball with no tap repo behind it,
  so `brew install` never worked. Distribution is the release tarball plus `install.sh`.

## [0.2.0] - 2026-04-19

- Core tests, AppKit host detection, CLI, CI, and release workflow.
- Universal macOS binary (`lipo` of aarch64 + x86_64) published on `v*` tags.
