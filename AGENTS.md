## Learned User Preferences

- Prefers the agent to run builds, checks, and git or GitHub CLI steps locally when possible instead of only describing commands for the user to run.
- When the caveman skill is attached to a message, follow its terse style until the user says "stop caveman" or "normal mode".
- When a plan is attached and todos already exist, do not edit the plan file or recreate todos; execute the plan and update existing todo statuses as you work.

## Learned Workspace Facts

- devsignal is a Rust workspace (crates include devsignal-core, devsignal-discord, devsignal-macos, devsignal-daemon) for a macOS Discord Rich Presence daemon; primary upstream GitHub repo is rabbive/devsignal.
- Workspace MSRV is Rust 1.74; CI runs `cargo fmt --check`, clippy, tests, and full macOS release-style builds.
- `Cargo.lock` is committed for reproducible CI and local dependency resolution.
- `packaging/macos/install.sh` defaults `DEVSIGNAL_GITHUB_REPO` to rabbive/devsignal; set the variable to use a fork.
- `scripts/setup-local-config.sh` copies `config.example.toml` to `~/.config/devsignal/config.toml` when missing; Discord desktop (not browser-only) is required for Rich Presence IPC.
- Releases publish a universal macOS tarball plus `SHA256SUMS`; distribution is the tarball and
  `packaging/macos/install.sh` only (the stale v0.2.0-pinned Homebrew formula was removed in 0.3.0).
- `ci.yml` and `release.yml` set `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24` to reduce GitHub Actions Node 20 deprecation noise.
