#!/usr/bin/env bash
# Remove everything install.sh (or `devsignal init`) put on this machine.
#
# Usage:
#   ./uninstall.sh              # interactive: asks before deleting the config
#   ./uninstall.sh --keep-config
#   ./uninstall.sh --purge      # delete the config too, no prompt
#   curl -fsSL <raw-url>/uninstall.sh | bash -s -- --keep-config
#
# Stopping the LaunchAgent clears Discord presence on the way out: the daemon traps SIGTERM and
# clears before exiting, and `launchctl bootout` sends exactly that.
#
set -euo pipefail

LABEL="com.devsignal.daemon"
BIN="${HOME}/bin/devsignal"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
CFG_DIR="${HOME}/.config/devsignal"
LOG_DIR="${HOME}/Library/Logs/devsignal"

PURGE_CONFIG="ask"
for arg in "$@"; do
  case "$arg" in
    --purge)       PURGE_CONFIG="yes" ;;
    --keep-config) PURGE_CONFIG="no" ;;
    -h|--help)     sed -n '2,12p' "$0"; exit 0 ;;
    *)             echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

removed_anything="no"

# Bootout first, so the daemon clears presence before its binary disappears from under it.
if launchctl print "gui/$(id -u)/${LABEL}" >/dev/null 2>&1; then
  launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
  echo "Stopped and unloaded the LaunchAgent (presence cleared)."
  removed_anything="yes"
else
  echo "No LaunchAgent loaded."
fi

if [[ -f "$PLIST" ]]; then
  rm -f "$PLIST"
  echo "Removed ${PLIST}"
  removed_anything="yes"
fi

if [[ -e "$BIN" ]]; then
  rm -f "$BIN"
  echo "Removed ${BIN}"
  removed_anything="yes"
else
  echo "No binary at ${BIN} (installed elsewhere? remove it by hand)."
fi

if [[ -d "$LOG_DIR" ]]; then
  rm -rf "$LOG_DIR"
  echo "Removed ${LOG_DIR}"
  removed_anything="yes"
fi

# The config is the one thing worth keeping: it holds the Discord application id and any hand-written
# rules, so deleting it silently would be the difference between reinstalling and starting over.
if [[ -d "$CFG_DIR" ]]; then
  if [[ "$PURGE_CONFIG" == "ask" ]]; then
    if [[ -t 0 ]]; then
      read -r -p "Also delete ${CFG_DIR} (Discord app id and rules)? [y/N] " ans || true
      [[ "${ans:-}" =~ ^[Yy]$ ]] && PURGE_CONFIG="yes" || PURGE_CONFIG="no"
    else
      # No TTY to ask, so keep it. Destroying config without being able to confirm is the wrong
      # default for a piped one-liner.
      PURGE_CONFIG="no"
    fi
  fi

  if [[ "$PURGE_CONFIG" == "yes" ]]; then
    rm -rf "$CFG_DIR"
    echo "Removed ${CFG_DIR}"
  else
    echo "Kept ${CFG_DIR} (delete it by hand, or re-run with --purge)."
  fi
  removed_anything="yes"
fi

echo ""
if [[ "$removed_anything" == "yes" ]]; then
  echo "devsignal uninstalled."
else
  echo "Nothing to uninstall — no devsignal files found in the usual places."
fi
