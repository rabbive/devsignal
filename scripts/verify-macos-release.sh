#!/usr/bin/env bash
# Run the macOS-only checks that CI cannot do, and write a report to paste back.
#
# Three things about a devsignal release can only be confirmed on a real Mac with Discord
# running: that agent CLIs are detected under their actual process names, that the LaunchAgent
# publishes presence, and that `launchctl bootout` clears it again. This script does all three
# and prints one report.
#
# Usage: from repo root
#   ./scripts/verify-macos-release.sh                 # read-only: build, detect sweep
#   ./scripts/verify-macos-release.sh --launchd       # also do the LaunchAgent round trip
#   ./scripts/verify-macos-release.sh --config PATH   # use a config other than the default
#
# The read-only pass mutates nothing. --launchd loads and unloads a LaunchAgent; any existing
# plist is backed up and restored, and an already-running daemon is put back as it was found.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL="com.devsignal.daemon"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
CONFIG="${HOME}/.config/devsignal/config.toml"
REPORT="${ROOT}/verify-report.txt"
DO_LAUNCHD=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --launchd) DO_LAUNCHD=1; shift ;;
    --config)  CONFIG="${2:?--config needs a path}"; shift 2 ;;
    # Print the header comment as the help text, so the two cannot drift apart.
    -h|--help)
      awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"
      exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script only does anything useful on macOS (got $(uname -s))." >&2
  exit 1
fi

: > "$REPORT"
say() { printf '%s\n' "$*" | tee -a "$REPORT"; }
rule() { say ""; say "=== $* ==="; }

say "devsignal macOS verification — $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
say "macOS $(sw_vers -productVersion)  arch $(uname -m)  commit $(git -C "$ROOT" rev-parse --short HEAD)"

# ---------------------------------------------------------------- build

rule "build"
cargo build --release -p devsignal-daemon --manifest-path "${ROOT}/Cargo.toml" 2>&1 | tail -3 | tee -a "$REPORT"
BIN="${ROOT}/target/release/devsignal"
say "binary: ${BIN}"
say "version: $("$BIN" --version)"

# ---------------------------------------------------------------- config

rule "config"
if [[ -f "$CONFIG" ]]; then
  say "using ${CONFIG}"
  if "$BIN" validate --config "$CONFIG" >>"$REPORT" 2>&1; then
    say "validate: OK"
  else
    say "validate: FAILED — see the report; fix the config before continuing"
    exit 1
  fi
else
  say "no config at ${CONFIG}"
  say "run ./scripts/setup-local-config.sh (or 'devsignal init'), set discord.client_id, re-run"
  if [[ "$DO_LAUNCHD" -eq 1 ]]; then
    say "cannot do the launchd round trip without a config"
    exit 1
  fi
fi

# ---------------------------------------------------------------- detection
#
# The point of the sweep: docs/community-presets.md lists ten agents whose process names were
# inferred, never observed. Anything that shows up under --unmatched while its CLI is running is
# a preset whose process_names are wrong.

rule "detection"
say "Start every AI coding CLI you want confirmed BEFORE this point."
say ""
say "--- matched ---"
# macOS ships bash 3.2, where "${ARR[@]}" on an empty array is an unbound-variable error under
# `set -u`; the ${ARR[@]+...} guard is what makes an optional-args array portable there.
DETECT_ARGS=()
[[ -f "$CONFIG" ]] && DETECT_ARGS=(--config "$CONFIG")
"$BIN" detect ${DETECT_ARGS[@]+"${DETECT_ARGS[@]}"} 2>&1 | tee -a "$REPORT" \
  || say "(detect exited non-zero)"
say ""
say "--- unmatched (candidate process names) ---"
"$BIN" detect --unmatched 2>&1 | tee -a "$REPORT" || say "(detect --unmatched exited non-zero)"

# ---------------------------------------------------------------- launchd

if [[ "$DO_LAUNCHD" -eq 0 ]]; then
  rule "launchd"
  say "skipped — re-run with --launchd to do the round trip"
else
  rule "launchd round trip"

  WAS_LOADED=0
  if launchctl print "gui/$(id -u)/${LABEL}" >/dev/null 2>&1; then
    WAS_LOADED=1
    say "a ${LABEL} is already loaded; unloading it first and restoring it at the end"
    launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
  fi

  BACKUP=""
  if [[ -f "$PLIST" ]]; then
    BACKUP="${PLIST}.verify-backup.$$"
    cp "$PLIST" "$BACKUP"
    say "backed up existing plist to ${BACKUP}"
  fi

  restore() {
    launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
    if [[ -n "$BACKUP" ]]; then
      mv "$BACKUP" "$PLIST"
      if [[ "$WAS_LOADED" -eq 1 ]]; then
        launchctl bootstrap "gui/$(id -u)" "$PLIST" 2>/dev/null || true
      fi
      say "restored the original plist"
    else
      rm -f "$PLIST"
      say "removed the plist this script wrote"
    fi
  }
  trap restore EXIT

  mkdir -p "${HOME}/Library/LaunchAgents" "${HOME}/Library/Logs/devsignal"
  # Built from the packaged template so this exercises the shipped plist, not a private copy.
  sed -e "s|/REPLACE/WITH/ABSOLUTE/PATH/TO/devsignal|${BIN}|" \
      -e "s|REPLACE_HOME|${HOME}|g" \
      "${ROOT}/packaging/macos/com.devsignal.daemon.example.plist" > "$PLIST"
  # The wizard passes --config; match it, or the daemon reads a different file than we validated.
  /usr/libexec/PlistBuddy -c "Add :ProgramArguments: string --config" "$PLIST" >/dev/null
  /usr/libexec/PlistBuddy -c "Add :ProgramArguments: string ${CONFIG}" "$PLIST" >/dev/null

  if plutil -lint "$PLIST" >>"$REPORT" 2>&1; then
    say "plutil -lint: OK"
  else
    say "plutil -lint: FAILED — the plist template is malformed"
    exit 1
  fi

  say "bootstrapping..."
  launchctl bootstrap "gui/$(id -u)" "$PLIST"
  launchctl kickstart -k "gui/$(id -u)/${LABEL}"
  sleep 5

  if launchctl print "gui/$(id -u)/${LABEL}" | grep -q "state = running"; then
    say "daemon state: running"
  else
    say "daemon state: NOT running — check ~/Library/Logs/devsignal/devsignal.err.log"
  fi

  say ""
  say "Look at your Discord profile now. Presence should be showing."
  read -r -p "Did presence APPEAR in Discord? [y/N] " appeared
  say "presence appeared: ${appeared:-n}"

  say ""
  say "Unloading (this is the SIGTERM path that used to leave presence stuck)..."
  launchctl bootout "gui/$(id -u)/${LABEL}"
  sleep 3

  read -r -p "Did presence CLEAR in Discord? [y/N] " cleared
  say "presence cleared: ${cleared:-n}"

  rule "daemon log tail"
  tail -20 "${HOME}/Library/Logs/devsignal/devsignal.err.log" 2>/dev/null | tee -a "$REPORT" \
    || say "(no error log)"
fi

# ---------------------------------------------------------------- done

rule "summary"
say "Report written to ${REPORT} — paste it back."
say ""
say "Still not covered by this script:"
say "  - signing/notarization: needs the five Apple secrets, then re-run the release workflow"
say "    via workflow_dispatch and check that codesign/notarize are not skipped"
say "  - install.sh against a real v0.3.0 release: only possible after the tag exists"
