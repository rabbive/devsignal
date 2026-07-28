#!/usr/bin/env bash
# One-shot installer: download a GitHub Release binary, verify it, install to ~/bin, scaffold
# config, optionally set up the LaunchAgent.
#
# Usage:
#   ./install.sh [version]                       # from a clone
#   curl -fsSL <raw-url>/install.sh | bash       # standalone
#   curl -fsSL <raw-url>/install.sh | bash -s 0.3.0
#
# version defaults to the latest release tag (leading "v" optional).
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

TAG="v${VERSION}"
TARBALL="devsignal-${VERSION}-macos-universal.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
RAW_URL="https://raw.githubusercontent.com/${REPO}/${TAG}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Installing devsignal ${VERSION} from ${REPO}"
if ! curl -fL --retry 3 --retry-delay 2 -o "${TMP}/${TARBALL}" "${BASE_URL}/${TARBALL}"; then
  echo "" >&2
  echo "Could not download ${TARBALL} from ${BASE_URL}." >&2
  echo "Check that ${TAG} exists and has a macOS universal asset:" >&2
  echo "  https://github.com/${REPO}/releases" >&2
  exit 1
fi

# Verify against the published SHA256SUMS. Releases before v0.3.0 have no such asset, so a missing
# file is a warning rather than a failure; a checksum that is present and wrong is always fatal.
if curl -fsSL -o "${TMP}/SHA256SUMS" "${BASE_URL}/SHA256SUMS" 2>/dev/null; then
  EXPECTED="$(awk -v f="$TARBALL" '$2 == f || $2 == "*"f {print $1}' "${TMP}/SHA256SUMS" | head -1)"
  if [[ -z "$EXPECTED" ]]; then
    echo "warning: SHA256SUMS has no entry for ${TARBALL}; skipping verification" >&2
  else
    ACTUAL="$(shasum -a 256 "${TMP}/${TARBALL}" | awk '{print $1}')"
    if [[ "$EXPECTED" != "$ACTUAL" ]]; then
      echo "Checksum mismatch for ${TARBALL}!" >&2
      echo "  expected: ${EXPECTED}" >&2
      echo "  actual:   ${ACTUAL}" >&2
      exit 1
    fi
    echo "Checksum verified."
  fi
else
  echo "warning: no SHA256SUMS published for ${TAG}; cannot verify the download" >&2
fi

tar xzf "${TMP}/${TARBALL}" -C "$TMP"

mkdir -p "${HOME}/bin"
install -m 0755 "${TMP}/devsignal" "${HOME}/bin/devsignal"

# Downloads carry a quarantine attribute. A notarized binary passes Gatekeeper's online check, but
# an unsigned one (or any build predating notarization) is blocked outright, so clear it here.
if xattr -p com.apple.quarantine "${HOME}/bin/devsignal" >/dev/null 2>&1; then
  xattr -d com.apple.quarantine "${HOME}/bin/devsignal" 2>/dev/null || true
  echo "Cleared the Gatekeeper quarantine attribute."
fi

# Prefer a local copy when running from a clone; otherwise fetch the file for this exact tag.
fetch_support_file() {
  local rel_path="$1" dest="$2"
  local script_dir repo_root
  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd || echo "")"
  if [[ -n "$script_dir" ]]; then
    repo_root="$(cd "${script_dir}/../.." 2>/dev/null && pwd || echo "")"
    if [[ -n "$repo_root" && -f "${repo_root}/${rel_path}" ]]; then
      cp "${repo_root}/${rel_path}" "$dest"
      return 0
    fi
  fi
  curl -fsSL -o "$dest" "${RAW_URL}/${rel_path}"
}

CFG_DIR="${HOME}/.config/devsignal"
CFG_FILE="${CFG_DIR}/config.toml"
mkdir -p "$CFG_DIR"
if [[ ! -f "$CFG_FILE" ]]; then
  if fetch_support_file "config.example.toml" "$CFG_FILE"; then
    echo "Created ${CFG_FILE} — set discord.client_id before running."
  else
    echo "warning: could not fetch config.example.toml; run 'devsignal init' to generate a config." >&2
  fi
else
  echo "Keeping existing ${CFG_FILE}"
fi

LOG_DIR="${HOME}/Library/Logs/devsignal"
mkdir -p "$LOG_DIR"

echo ""
echo "Installed ${HOME}/bin/devsignal (add ~/bin to PATH if needed)."
echo "Next steps:"
echo "  devsignal init       # guided setup: Discord app id, privacy level, autostart"
echo "  devsignal validate   # check the config"
echo "  devsignal detect     # see which agent CLIs are detected right now"
echo "  devsignal run        # run in the foreground"
echo ""

# Only offer the interactive LaunchAgent step when there is a TTY to answer it. Under `curl | bash`
# stdin is the script itself, so a `read` would consume script text instead of user input.
if [[ ! -t 0 ]]; then
  # An upgrade over `curl | bash` has no TTY to answer the autostart prompt, but a LaunchAgent that is
  # already loaded is still running the *old* binary. Restarting it here is the difference between an
  # upgrade taking effect now and taking effect at next login, with no indication which one happened.
  if launchctl print "gui/$(id -u)/com.devsignal.daemon" >/dev/null 2>&1; then
    launchctl kickstart -k "gui/$(id -u)/com.devsignal.daemon"
    echo "Restarted the running LaunchAgent on the new binary."
  else
    echo "Run 'devsignal init' to set up autostart (LaunchAgent)."
  fi
  exit 0
fi

read -r -p "Load the LaunchAgent now (autostart at login)? [y/N] " ans || true
if [[ ! "${ans:-}" =~ ^[Yy]$ ]]; then
  echo "Skipped. 'devsignal init' can do this later."
  exit 0
fi

PLIST_SRC="${TMP}/com.devsignal.daemon.example.plist"
PLIST_DST="${HOME}/Library/LaunchAgents/com.devsignal.daemon.plist"
if ! fetch_support_file "packaging/macos/com.devsignal.daemon.example.plist" "$PLIST_SRC"; then
  echo "Could not obtain the LaunchAgent template; run 'devsignal init' instead." >&2
  exit 1
fi

mkdir -p "$(dirname "$PLIST_DST")"
# The template carries `--config` with a placeholder path, so plain `sed` finishes the job. This used
# to need a python3 `plistlib` pass that ran *after* sed had already written the file, so on a machine
# without python3 the install left behind a plist with no --config at all.
sed -e "s|/REPLACE/WITH/ABSOLUTE/PATH/TO/devsignal|${HOME}/bin/devsignal|g" \
    -e "s|REPLACE_CONFIG_PATH|${CFG_FILE}|g" \
    -e "s|REPLACE_HOME|${HOME}|g" \
    "$PLIST_SRC" > "$PLIST_DST"

# Never bootstrap a job that cannot start. `KeepAlive` restarts the daemon on any nonzero exit, and
# an unloadable config is a permanent failure, so installing one means a respawn every minute until
# the user notices. Exit codes cannot tell launchd the difference, so check here instead.
if ! "${HOME}/bin/devsignal" validate --config "$CFG_FILE" >/dev/null 2>&1; then
  echo "Not loading the LaunchAgent: ${CFG_FILE} does not validate." >&2
  echo "Fix it (or run 'devsignal init'), then re-run this script. The error was:" >&2
  "${HOME}/bin/devsignal" validate --config "$CFG_FILE" >&2 || true
  exit 1
fi

launchctl bootout "gui/$(id -u)/com.devsignal.daemon" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST_DST"
launchctl kickstart -k "gui/$(id -u)/com.devsignal.daemon"
echo "Loaded LaunchAgent com.devsignal.daemon"
echo "Logs: ${LOG_DIR}"
