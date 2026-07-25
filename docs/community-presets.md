# Community agent presets

devsignal ships presets only for agent CLIs whose process names have been **confirmed on a real
machine**: Claude Code, Codex, and OpenCode. Everything below is a plausible-but-unconfirmed guess.

That distinction matters. A preset with a wrong `process_names` does not fail loudly — it simply never
matches, and devsignal looks broken with no way to tell the difference. So these live here, as
snippets you confirm and opt into, rather than in the default config.

## The workflow

```bash
# 1. Start the agent CLI you want to track, in another terminal.

# 2. Ask devsignal what your machine actually reports.
devsignal detect --unmatched

# 3. Add it, using the name from step 2.
devsignal agents add --id gemini_cli --label "Gemini CLI" --process-name gemini --priority 100
```

`detect --unmatched` lists running processes that no rule matched, filtered to things that look like
user-installed binaries. If your agent does not appear, `devsignal detect --all` skips the filter.

Once added, confirm it wins as expected:

```bash
devsignal detect          # should list your agent and name it the winner
devsignal watch           # shows the presence payload tick by tick, without touching Discord
```

Matching is case-insensitive and checks the process name **or** the basename of `argv[0]`, so
Node- and Python-wrapped CLIs (`node .../gemini`, a `pipx`-installed `aider`) are covered without
extra configuration.

## Snippets

Paste any of these into the `[[agents]]` section of `~/.config/devsignal/config.toml`. Priorities
start at 100 so they never collide with the shipped presets (10, 20, 30) — lower wins when more than
one agent is running.

`large_image` is a Discord art-asset **key**, not a URL. A key you have not uploaded under
Developer Portal → Rich Presence → Art Assets renders blank, so either upload one named to match or
delete the line to fall back to the `devsignal` image.

### Gemini CLI

```toml
[[agents]]
id            = "gemini_cli"
label         = "Gemini CLI"
process_names = ["gemini"]
priority      = 100
large_image   = "gemini_cli"
```

### Amp

`amp` is a short, generic name and may collide with unrelated binaries. If it matches something
unexpected, narrow it with `argv_substrings` or turn it off with `devsignal agents disable amp`.

```toml
[[agents]]
id            = "amp"
label         = "Amp"
process_names = ["amp"]
priority      = 110
large_image   = "amp"
```

### Cursor Agent

```toml
[[agents]]
id            = "cursor_agent"
label         = "Cursor Agent"
process_names = ["cursor-agent"]
priority      = 120
large_image   = "cursor_agent"
```

### Copilot CLI

Older installs run as a `gh` extension (`gh copilot`), in which case the process is `gh` and this
preset will not match — matching `gh` itself would light up for every unrelated `gh` command, so
there is no good preset for that shape.

```toml
[[agents]]
id            = "copilot_cli"
label         = "Copilot CLI"
process_names = ["copilot"]
priority      = 130
large_image   = "copilot_cli"
```

### Aider

```toml
[[agents]]
id            = "aider"
label         = "Aider"
process_names = ["aider"]
priority      = 140
large_image   = "aider"
```

### Crush

```toml
[[agents]]
id            = "crush"
label         = "Crush"
process_names = ["crush"]
priority      = 150
large_image   = "crush"
```

### Qwen Code

```toml
[[agents]]
id            = "qwen_code"
label         = "Qwen Code"
process_names = ["qwen"]
priority      = 160
large_image   = "qwen_code"
```

### Droid

```toml
[[agents]]
id            = "droid"
label         = "Droid"
process_names = ["droid"]
priority      = 170
large_image   = "droid"
```

### Cline

```toml
[[agents]]
id            = "cline"
label         = "Cline"
process_names = ["cline"]
priority      = 180
large_image   = "cline"
```

### Goose

**Name collision:** `goose` is also a widely used Go database-migration tool. If you have both, this
preset will show "Goose" while you are running migrations. Disable it with
`devsignal agents disable goose`, or narrow it with `argv_substrings`.

```toml
[[agents]]
id            = "goose"
label         = "Goose"
process_names = ["goose"]
priority      = 190
large_image   = "goose"
```

## Adding buttons

Any preset can carry up to two clickable buttons. More than two is rejected when the config loads,
rather than silently truncated.

```toml
[[agents]]
id            = "gemini_cli"
label         = "Gemini CLI"
process_names = ["gemini"]
priority      = 100

  [[agents.buttons]]
  label = "Docs"
  url   = "https://example.com/docs"
```

Or from the CLI: `devsignal agents add --id gemini_cli --process-name gemini --button "Docs=https://example.com/docs"`.

## Getting one promoted

If you have confirmed a preset against a real CLI — `devsignal detect` naming it as the winner — open
an issue or PR with the `detect` output. Confirmed presets move into `agent_presets()` in
`devsignal-core` and ship as defaults.
