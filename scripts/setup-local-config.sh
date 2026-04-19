#!/usr/bin/env bash
# Scaffold ~/.config/devsignal/config.toml from repo config.example.toml (skip if already present).
# Usage: from repo root — ./scripts/setup-local-config.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${ROOT}/config.example.toml"
CFG_DIR="${HOME}/.config/devsignal"
CFG_FILE="${CFG_DIR}/config.toml"

if [[ ! -f "$SRC" ]]; then
  echo "Missing ${SRC}" >&2
  exit 1
fi

mkdir -p "$CFG_DIR"
if [[ -f "$CFG_FILE" ]]; then
  echo "Already exists: ${CFG_FILE} (not overwriting)"
  exit 0
fi

cp "$SRC" "$CFG_FILE"
echo "Created ${CFG_FILE}"
echo "Edit discord.client_id and Rich Presence assets in Discord Developer Portal, then run Discord desktop + devsignal."
