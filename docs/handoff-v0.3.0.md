# devsignal v0.3.0 — handoff

State of the v0.3.0 work as of commit `08348b1`, written so it can be picked up cold.

`CLAUDE.md` is the orientation doc for the codebase itself; this file covers what is *done*, what is
*unverified*, and what is *left*. Delete it once v0.3.0 is tagged and the open items below are closed.

## Where things stand

| | |
|---|---|
| Branch | `claude/claude-md-docs-x2sc96` — **12 commits ahead** of `main` (`43e7511`), all pushed |
| PR | **none open** — nothing has been opened against `main` |
| Version | `Cargo.toml` = `0.3.0`; the only tag on the remote is `v0.2.0` (2026-04-19) |
| CHANGELOG | `## [0.3.0] - unreleased` — not yet dated |
| Tests | **114 passing** (core 55, daemon lib 47, macos 6; integration: shutdown 3, hot_reload 2, detection 1) |
| Lint | `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `shellcheck` all clean |
| MSRV | **1.87**, enforced by a CI job (was falsely declared 1.74) |

Commits, newest first:

```
08348b1 docs: bring CLAUDE.md in line with the loop, CLI, and CI as they now are
4dd8bdd ci: fix the signing gate, add release dry-run, shellcheck, verify install.sh
c9ecda2 feat: hot-reload config, surface the matched rule, reject unknown keys
c29cbd7 fix: remove exited processes from the snapshot, rate-limit every Discord send
7e62ad8 fix(macos): time-box osascript, stop the fallback respawn treadmill
22d88ef feat: ship only confirmed presets, add agent discovery and `agents add`
a2c626a test: prove the SIGTERM shutdown path, add `devsignal watch`
8868bc2 docs(readme): list the shipped agent presets and clarify art-asset keys
000f590 chore(release): make v0.3.0 installable — checksums, notarization, install.sh
22706f9 feat: broaden agent coverage, add `devsignal detect`, single-source presets
9e7cb27 fix: clear presence on SIGTERM, validate config at load, write atomically
92fbb2b docs: refresh CLAUDE.md for current CLI, rules, and platform config
```

## What this work was

The project was stalled at **distribution, not features**. Three features — buttons/small assets, the
`init` wizard, and the `[[rules]]` engine — had been sitting on `main` unreleased since April, so
anyone installing the documented way got the April build. Shipping them surfaced a chain of silent
failures underneath.

**Bugs fixed, roughly by severity:**

1. **Presence got stuck in Discord on launchd shutdown.** `ctrlc` lacked the `termination` feature, so
   only SIGINT was trapped — but `launchctl bootout` / `kickstart -k`, which `devsignal init`
   installs, send SIGTERM. The README claimed SIGTERM worked in three places.
2. **Exited agents were never released.** `System::refresh_specifics` passes
   `remove_dead_processes: false`, so a seen agent matched for the daemon's whole life: presence kept
   showing a CLI you had quit, `idle_mode = "clear"` never fired, the elapsed timer never reset. Found
   by measurement — a flapping agent produced *one* transition in 24s; after the fix, twelve.
3. **A hung `osascript` blocked shutdown itself.** No timeout, running on the daemon's only thread,
   and the loop reads its stop flag *between* ticks — so a wedged `System Events` prevented both exit
   and the final clear. The SIGTERM fix does not help there. It also re-forked every other poll
   forever, because the native-miss streak was reset *before* the fallback ran.
4. **Config constraints were documented but unenforced.** Button limits were written in doc comments
   and never checked; a third button was silently dropped; `time = { start = "9" }` produced a rule
   that could never match, forever, with no error. Each failed the same way — Discord rejects the
   payload, one `warn!` lands in a log file, presence quietly stops updating.
5. **Forced sends ignored the rate limit, and clears skipped the debouncer entirely.**
6. **The declared MSRV was wrong** (1.74 vs the real 1.87), and every CI job used `stable` so nothing
   could catch it.
7. **The signing gate checked one of the five secrets notarization needs**, so a half-configured repo
   would sign successfully and then fail *after* building the artifact.
8. The example LaunchAgent plist was not well-formed XML (`--` inside an XML comment).

**Added:** `devsignal watch`, `detect`, `detect --unmatched` / `--all`, `agents add` / `remove`,
`--version`, config hot-reload, a visible matched-rule, `SHA256SUMS`, notarization, and a
`workflow_dispatch` release dry-run.

**Removed:** the Homebrew formula — a template pinned to the v0.2.0 tarball with no tap behind it, so
`brew install` never worked for anyone.

## Design invariants — do not undo these

Each exists for a reason that is not obvious from the code alone.

- **`matched_rule_name` must stay off `PresenceView`.** That struct is the `Debouncer`'s equality key,
  so adding it would trigger a Discord write whenever the matched rule changed, even with identical
  visible text. `build_policy_view` returns it alongside the view instead.
- **Platform gating is a runtime check (`require_macos`), not `cfg`.** That is what lets CI lint and
  test everything except `devsignal-macos`. Reintroducing `#[cfg(target_os = "macos")]` around the
  loop brings back 58 Linux warnings and makes the integration tests unrunnable.
- **`refresh_processes_specifics(ProcessesToUpdate::All, true, …)`** — that `true` is
  `remove_dead_processes`. Reverting to `refresh_specifics` reintroduces bug 2 above.
- **The shutdown clear deliberately bypasses the debouncer.** Clearing on exit must never be
  rate-limited away.
- **`StdoutSink`'s `presence:set` / `presence:clear` markers are a contract** with
  `tests/shutdown.rs`, `tests/detection.rs`, and `tests/hot_reload.rs`.
- **Only agents with process names confirmed on a real machine belong in `agent_presets()`.** Adding
  one is a claim that `devsignal detect` was run against it. A bidirectional drift test ties
  `config.example.toml` to that table; unconfirmed agents live in `docs/community-presets.md` with
  priorities ≥ 100, and their TOML is validated by a core test.
- **An invalid config edit must never kill a running daemon** — `maybe_reload_config` warns and keeps
  the previous configuration.

## Verified vs not

**Verified on Linux, in-session:** all 114 tests. The three regression tests were each confirmed to
*fail* when their fix is reverted — removing `ctrlc`'s `termination` feature makes the two SIGTERM
cases fail while SIGINT still passes (the original bug's exact signature), and setting
`remove_dead_processes: false` makes the detection test fail on precisely the "released when it exits"
assertion. Rate limiting measured end to end on a flapping agent: 12 transitions → 7 sends + 5
suppressions. Hot-reload measured: a valid edit applies, an invalid edit is rejected with the daemon
surviving, and a later valid edit still applies. `install.sh` was **actually executed** against the
live v0.2.0 release in a throwaway `HOME` three ways — from a clone, standalone, and true
`curl | bash`; v0.2.0 publishes no `SHA256SUMS`, which exercised the warn-don't-fail branch for real.
Checksum verification was checked against five fixtures including a tampered download. The MSRV was
measured by bisecting toolchains, not guessed.

**Not verified — needs macOS or maintainer credentials:**

1. **Community preset process names.** The ten in `docs/community-presets.md` are inferred and have
   never been run. Confirm with `devsignal detect --unmatched` while each CLI is running, then promote
   into `agent_presets()` *and* `config.example.toml` — the drift test fails if only one is updated.
2. **The real launchd → Discord loop**: `init` → LaunchAgent loaded → presence appears →
   `launchctl bootout gui/$(id -u)/com.devsignal.daemon` → presence clears in the actual client.
3. **Signing and notarization have never executed.** They need five repo secrets:
   `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD`, `APPLE_NOTARY_API_KEY`, `APPLE_NOTARY_KEY_ID`,
   `APPLE_NOTARY_ISSUER_ID`. All five or none — a partial set is a hard error by design. The
   `workflow_dispatch` trigger dry-runs the whole pipeline without cutting a tag.
4. **`install.sh` against a real v0.3.0 release**, including the Gatekeeper path.

## Next steps, in order

1. Open a PR for the branch (none exists), or merge to `main`.
2. Add the five Apple secrets, then run the release workflow via `workflow_dispatch` as a dry run.
3. Do the macOS verification in items 1 and 2 above.
4. Date the CHANGELOG heading and tag `v0.3.0`. The workflow now **fails** if the tag disagrees with
   `Cargo.toml`, so bump both together in future.
5. Re-run `install.sh` against the real release.

**Three changes are breaking**, and are called out in `CHANGELOG.md`: config validation at load,
`deny_unknown_fields`, and the 5-per-20s rate limit. A config that previously "worked" by having
Discord silently reject it, or by relying on button truncation, will now fail to load.

## Still deferred (never started)

`status` / `doctor` / `logs` / `stop` / `uninstall` subcommands; log rotation for the launchd log files
(nothing rotates them, though ANSI escapes are no longer written); `hosts disable` accepting
unvalidated bundle ids that then do not appear in `hosts list`; session timers keyed on agent id
rather than process identity, so quitting and relaunching within one poll keeps a stale elapsed time;
the three untimed `Command::output()` calls in the `init` wizard (interactive, not in the poll loop);
and cross-platform host detection — `devsignal-core` is deliberately platform-free so that door stays
open.

## Orientation

Start with `CLAUDE.md`, which documents the numbered poll loop, the module layout, and the invariants
above. Then:

```bash
cargo test --workspace                                  # 114 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo +1.87 check -p devsignal-core -p devsignal-discord -p devsignal-daemon --all-targets
shellcheck packaging/macos/install.sh scripts/*.sh

devsignal watch  --config <path>     # the real loop, no Discord — the best debugging tool
devsignal detect --unmatched         # find an agent CLI's real process name
```

Layout: `crates/devsignal-core` (platform-free config, matching, rules, debouncer),
`devsignal-macos` (AppKit plus the time-boxed `osascript` fallback), `devsignal-discord` (IPC
wrapper), and `devsignal-daemon` (binary `devsignal`: `main.rs`, `cli.rs`, `sink.rs`, `config_io.rs`,
`config_edit.rs`, `init.rs`, plus `tests/`).
