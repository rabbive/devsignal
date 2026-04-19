# devsignal

Unified **Discord Rich Presence** for AI coding CLIs on **macOS**. One daemon, one Discord connection: it detects which agent-style tool is running (for example Claude Code, Codex, OpenCode — configurable) and shows the **frontmost host app** (Cursor, VS Code, JetBrains, terminals, etc.).

## Discord application setup

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and **New Application**.
2. In **OAuth2** (optional for local IPC): not required for Rich Presence; you only need the app record.
3. Copy **Application ID** (this is the Rich Presence `client_id`).
4. Under **Rich Presence → Art Assets**, upload PNGs. Each **image key** must match what you put in `config.toml` (`large_image` per agent and the global default).
5. Install and run the **Discord desktop** client (not only the web app). The daemon connects over local IPC.

## Quick start

1. Copy `config.example.toml` to `~/.config/devsignal/config.toml`.
2. Set `discord.client_id` to your Application ID.
3. Run:

```bash
cargo run --release -p devsignal-daemon
```

Leave **Discord desktop** open; the daemon talks to it over local IPC.

### macOS permissions

Host detection uses AppleScript (`osascript`) to ask **System Events** for the frontmost app’s bundle id. The first run may prompt for **Automation** access for the app that launches `devsignal` (for example Terminal, iTerm2, or Cursor).

## Configuration

- `poll_interval_secs`: how often processes and the frontmost app are sampled.
- `min_push_interval_secs`: minimum time between Discord presence updates unless the active agent changes (reduces flicker and rate limits).
- `idle_mode`: `status` (default) shows an idle line when no agent is detected; `clear` calls Discord **CLEAR_ACTIVITY** so nothing is shown for this application.
- `show_cwd_basename`: when `true`, appends the **basename only** of the winning agent process working directory (never full paths). Off by default for privacy.
- `[[agents]]`: `process_names` are matched case-insensitively against `sysinfo` process names. Optional `argv_substrings` narrow matches when non-empty.
- `priority`: **lower number wins** when multiple agents match.

## LaunchAgent (login item)

Use an absolute path to the `devsignal` binary in the plist **ProgramArguments**.

Example plist: [`packaging/macos/com.devsignal.daemon.example.plist`](packaging/macos/com.devsignal.daemon.example.plist)

Suggested log directory (referenced in the plist):

```text
~/Library/Logs/devsignal/
```

Create it before loading the agent:

```bash
mkdir -p ~/Library/Logs/devsignal
```

Copy the plist to `~/Library/LaunchAgents/`, edit paths, then:

```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.devsignal.daemon.plist
```

Privacy defaults: keep `show_cwd_basename = false` unless you are comfortable exposing folder names in Discord.

## macOS only (for now)

Other platforms fail fast until host detection and packaging are added.

## License

MIT
