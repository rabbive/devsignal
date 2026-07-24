# Changelog

All notable changes to devsignal. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semver.

## [0.3.0] - unreleased

First release since 0.2.0 (2026-04-19). Everything under "Added" below was already on `main` but had
never shipped: a user installing the documented way was getting the April build.

### Fixed

- **Presence no longer gets stuck in Discord after a launchd shutdown.** `ctrlc` was declared without
  the `termination` feature, so only SIGINT was trapped. `launchctl bootout` and `kickstart -k` — what
  `devsignal init` installs — send SIGTERM, so the daemon exited without clearing presence. The
  README had claimed SIGTERM was handled in three places.
- **The example LaunchAgent plist is now well-formed XML.** Its comment contained `--` (from
  `cargo build --release`), which XML forbids. `launchd` tolerated it; strict parsers and
  `plutil -lint` do not.
- `devsignal once` now resolves the project folder name the same way `run` does. It previously always
  passed `None`, so a rule using `when.project_basenames` could never match under `once` and the
  ` · project` suffix never appeared — the command could not reproduce what the daemon published.
- `devsignal hosts --help` and `devsignal rules add --help` print usage and exit 0. They used to exit
  2 with a usage error, or die with `unknown rules add flag: --help`.
- Explicitly requested `--help` goes to stdout, so it can be piped.
- Paths containing `&`, `<`, or `>` no longer produce a malformed LaunchAgent plist.
- Threads are excluded from agent matching. On Linux sysinfo lists them as processes sharing the
  CLI's `argv[0]`, so a single running agent matched a dozen times.
- `config.example.toml` and the README described `argv_substrings` as requiring *all* substrings to
  match; the implementation requires *any*, and always has.

### Added

- **`devsignal detect`** — shows every process matching an agent rule with its pid, name, and
  `argv0`, then which one wins and why. When nothing matches it lists the rules it searched. This is
  the answer to "why isn't my agent detected?"; `agents list` only ever showed *configured* agents.
- **`--version` / `-V` / `version`.** Nothing in the workspace referenced `CARGO_PKG_VERSION` before,
  so there was no way to smoke-test an install.
- **Agent presets go from 3 to 13**: adds `gemini_cli`, `amp`, `cursor_agent`, `copilot_cli`,
  `aider`, `crush`, `qwen_code`, `droid`, `cline`, `goose`. Priorities are spaced by 10 so custom
  rules can slot between presets. Process names are best-effort — confirm with `devsignal detect`,
  and note that short names (`amp`, `crush`, `goose`, which collides with the Go migration tool) can
  false-positive. `devsignal agents disable <id>` turns any preset off.
- Rich presence **small assets and up to 2 buttons** per agent (built in April, unreleased until now).
- The interactive **`devsignal init` wizard** (built in April, unreleased until now).
- The **`[[rules]]` presence engine** and `[platforms]` enable/disable toggles, with the
  `hosts`/`agents`/`rules` subcommands (built in April, unreleased until now).
- Releases publish a **`SHA256SUMS`** asset, and are **signed and notarized** when the maintainer's
  Apple credentials are configured in CI. Without them the workflow still publishes an unsigned
  binary and warns in the run log.
- The release workflow fails if the tag disagrees with the `Cargo.toml` version.
- `devsignal --help` documents all nine `rules add` flags, which previously appeared only in the README.

### Changed

- **Config is validated at load instead of failing silently later.** `Config::validate` now rejects a
  non-numeric `client_id`, an agent with no `process_names`, duplicate agent ids or rule names, more
  than 2 buttons, a button label over 32 characters, a url over 512 characters or without an
  `http(s)` scheme, an unparseable `TimeWindow`, a rule setting both `active_only` and `idle_only`,
  and a rule whose `then` does nothing. Each of these previously failed silently: Discord rejects an
  invalid payload wholesale, which surfaced as one `warn!` in a log file while presence quietly
  stopped updating. A `time` window like `"9-5"` produced a rule that could never match, ever.
  **This is a breaking change** for configs that were relying on a third button being truncated, or
  that contain values Discord was already rejecting.
- **Config writes are atomic.** Both writers used a plain `fs::write` and validated *afterwards*, so
  an interrupted write truncated the config and an invalid one was reported once the good file was
  gone. Now: serialize, round-trip, validate, write a sibling temp file, rename.
- `hosts`/`agents`/`rules` now say that comments are not preserved and that a running daemon needs a
  restart to pick the change up.
- **Privacy presets are now behaviourally distinct.** `Public/OSS` was advertised as "polished copy"
  but was byte-identical to `Minimal`. They are now Minimal (agent only, host hidden via a catch-all
  rule appended *after* any user-chosen rule so it cannot shadow one), Balanced (agent + host), and
  Detailed (adds the project folder name).
- **`install.sh` works standalone.** It read `config.example.toml` and the plist from a repo clone, so
  `curl | bash` silently skipped both; it now fetches them for the release tag. It also verifies the
  download against `SHA256SUMS`, clears the Gatekeeper quarantine attribute, passes `--config` in the
  generated plist (matching the wizard), skips the interactive LaunchAgent prompt when there is no
  TTY, and points at `devsignal init`.
- The process refresh asks only for `cmd` (plus `cwd` when `show_cwd_basename` is set) instead of
  `ProcessRefreshKind::everything()`, which refreshed `environ`, `exe`, `root`, memory, CPU, and disk
  usage for every process on the machine every 2 seconds.
- Agent presets are defined once in `devsignal-core::agent_presets()`. The wizard and
  `config.example.toml` were independent hardcoded copies; a test now asserts they agree.
- `devsignal validate` prints small assets, buttons, rules, and platform toggles.
- Missing-config errors consistently point at `devsignal init`.
- CLI parsing moved to `cli.rs` with no platform gating, so CI lints and tests it; `validate`, `once`,
  `detect`, and the config-edit subcommands now run on any platform, while `run` and `init` keep a
  runtime macOS check. The workspace is warning-free on Linux (was 58 warnings) and
  `devsignal-daemon` joined the Linux clippy and test jobs.
- Tests: 24 → 80.

### Removed

- **The Homebrew formula.** It was a template pinned to the v0.2.0 tarball with no tap repo behind
  it, so `brew install` never worked. Distribution is the release tarball and `install.sh`; the
  README no longer advertises a `brew` path.

## [0.2.0] - 2026-04-19

- Core tests, AppKit host detection, CLI, CI, and release workflow.
- Universal macOS binary (`lipo` of aarch64 + x86_64) published on `v*` tags.
