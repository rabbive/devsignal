#!/usr/bin/env bash
# One-shot installer: download the latest (or requested) GitHub Release binary, install to ~/bin,
# scaffold config, optional LaunchAgent bootstrap.
#
# Usage:
#   ./install.sh [version]     # version defaults to latest release tag (without leading v)
# Optional: DEVSIGNAL_GITHUB_REPO=owner/repo (default: rabbive/devsignal)
#
set -euo pipefail

REPO="${DEVSIGNAL_GITHUB_REPO:-rabbive/devsignal}"

VERSION_INPUT="${1:-}"
if [[ -n "$VERSION_INPUT" ]]; then
  VERSION="${VERSION_INPUT#v}"
else
  JSON="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")"
  TAG="$(printf '%s' "$JSON" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  VERSION="${TAG#v}"
fi

if [[ -z "$VERSION" ]]; then
  echo "Could not determine release version." >&2
  exit 1
fi

TARBALL="devsignal-${VERSION}-macos-universal.tar.gz"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/${TARBALL}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -fL -o "${TMP}/${TARBALL}" "$URL"
tar xzf "${TMP}/${TARBALL}" -C "$TMP"

mkdir -p "${HOME}/bin"
install -m 0755 "${TMP}/devsignal" "${HOME}/bin/devsignal"

CFG_DIR="${HOME}/.config/devsignal"
CFG_FILE="${CFG_DIR}/config.toml"
mkdir -p "$CFG_DIR"
if [[ ! -f "$CFG_FILE" ]]; then
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
  if [[ -f "${REPO_ROOT}/config.example.toml" ]]; then
    cp "${REPO_ROOT}/config.example.toml" "$CFG_FILE"
    echo "Created ${CFG_FILE} from config.example.toml — edit discord.client_id."
  else
    echo "No config at ${CFG_FILE}; copy config.example.toml from the repo and set discord.client_id."
  fi
else
  echo "Keeping existing ${CFG_FILE}"
fi

LOG_DIR="${HOME}/Library/Logs/devsignal"
mkdir -p "$LOG_DIR"

echo "Installed ${HOME}/bin/devsignal (add ~/bin to PATH if needed)."
echo "Run: devsignal validate"
echo "Run daemon: devsignal"
echo ""
read -r -p "Load LaunchAgent from packaging/macos plist? [y/N] " ans || true
if [[ "${ans:-}" =~ ^[Yy]$ ]]; then
  PLIST_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/com.devsignal.daemon.example.plist"
  PLIST_DST="${HOME}/Library/LaunchAgents/com.devsignal.daemon.plist"
  if [[ ! -f "$PLIST_SRC" ]]; then
    echo "Plist not found at $PLIST_SRC" >&2
    exit 1
  fi
  sed -e "s|/REPLACE/WITH/ABSOLUTE/PATH/TO/devsignal|${HOME}/bin/devsignal|g" \
      -e "s|REPLACE_HOME|${HOME}|g" \
      "$PLIST_SRC" > "$PLIST_DST"
  launchctl bootout "gui/$(id -u)/com.devsignal.daemon" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$PLIST_DST"
  echo "Loaded LaunchAgent com.devsignal.daemon"
fi
