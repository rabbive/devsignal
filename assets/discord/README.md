# Presence art assets

512×512 PNGs for every agent CLI and host app devsignal knows about.

```
agents/<agent id>.png     # large image — claude_code.png, codex.png, …
hosts/<image slug>.png    # small corner icon — ghostty.png, vs_code.png, …
overrides/                # optional: your own art, wins over anything generated
```

Each app wears **its own logo** — the real multi-colour marks (VS Code's ribbon, the JetBrains SKU
tiles, Xcode's hammer, Ghostty's ghost), not one-colour silhouettes. What makes them a *set* is the
treatment around them, applied identically to all 45:

| | |
| --- | --- |
| tile | 512×512, 22% corner radius, tinted 16% toward that logo's own dominant colour, 6px ring at 40% |
| glyph | 56% of the tile, centred — it has to survive Discord cropping the small slot to a circle |
| polarity | a mark that is essentially black (Cursor, Apple, OpenAI, Copilot, Terminal, Hyper, iTerm2) gets a **light** tile; everything else gets a dark one, decided by measured luminance rather than by hand |
| monogram | the eight apps with no logo in any CC0/MIT set (Kitty, Tabby, Nova, RustRover, Amp, Aider, Crush, Goose) get initials in the same tile, so they read as deliberate rather than missing |

The names are not decorative: `agents/<id>.png` is keyed by the `[[agents]] id`, and
`hosts/<slug>.png` by the `image` field of `HOST_APPS` in `devsignal-core`. Two tests
(`every_host_icon_name_has_a_png`, `every_preset_agent_image_has_a_png`) fail if a name in the code
has no file here, because a missing image is invisible at runtime — Discord just shows a blank tile
and logs nothing.

## Using them

Discord accepts a plain `https://` image URL anywhere it accepts an uploaded art-asset key, so there
are two ways in and both use these exact files:

```toml
# No uploads. Every image name expands to {base_url}/{agents|hosts}/{name}.png.
[images]
mode      = "url"
base_url  = "https://raw.githubusercontent.com/rabbive/devsignal/main/assets/discord"
host_icon = true
```

```toml
# Or upload these PNGs under Developer Portal → Rich Presence → Art Assets, keeping the
# filename (without .png) as the key.
[images]
mode = "key"
```

`base_url` points at `main`, so a branch that adds an icon serves it only after merge. Point
`base_url` at your own fork or CDN if you would rather not depend on this repo.

## Regenerating

```bash
pip install pillow cairosvg
python3 scripts/build-discord-assets.py           # rewrite the PNGs
python3 scripts/build-discord-assets.py --check   # fail if the committed PNGs drifted
```

Edit the `AGENTS` / `HOSTS` tables in that script to add an icon. Adding a host also means adding
its `HostApp` entry in `devsignal-core`; adding an agent means `agent_presets()` and
`config.example.toml`. The drift tests enforce both directions.

## Using a different icon pack

Drop your own file into `overrides/` and it is used verbatim — no code change, no table edit:

```
assets/discord/overrides/hosts/ghostty.png     # or .svg; any size, resized to 512
assets/discord/overrides/agents/claude_code.svg
```

The filename must match the generated one (`devsignal hosts list` prints the host slugs). Re-run the
build script and it reports which files came from `overrides/`.

**Check the licence before you add art here.** This folder is published to GitHub and served to
Discord like everything else, so an override is *redistribution*. Icon packs on Behance, DeviantArt,
Dribbble, and the like are normally © the artist and frequently licensed for personal use only —
"free download" is not the same as "free to redistribute". Sets that are safe to vendor state it:
CC0, MIT, CC-BY (with attribution), or an explicit redistribution grant you have bought. If you are
not sure, keep it local: point `[images] base_url` at your own private host instead of committing the
files.

## Provenance and trademarks

Glyph data is vendored in `sources.json`, extracted from four icon sets distributed on npm:

| Set | Package | Licence | Used for |
| --- | --- | --- | --- |
| [SVG Logos](https://github.com/gilbarbara/logos) | `@iconify-json/logos` | CC0-1.0 | 20 marks — VS Code, Xcode, the JetBrains SKUs, Claude, OpenAI, Qwen, Android, Terminal, Hyper |
| [Devicon](https://github.com/devicons/devicon) | `@iconify-json/devicon` | MIT | Zed, Cursor, VSCodium, Android Studio |
| [vscode-icons](https://github.com/vscode-icons/vscode-icons) | `@iconify-json/vscode-icons` | MIT | Fleet, Gemini |
| [Simple Icons](https://github.com/simple-icons/simple-icons) | `@iconify-json/simple-icons` | CC0-1.0 | one-colour marks — Ghostty, Warp, Alacritty, WezTerm, iTerm2, Aqua, Apple, OpenCode, Cline |

Eight apps have no logo in any of them and get a monogram instead of a hand-traced look-alike:
Kitty, Tabby, Nova, RustRover, Amp, Aider, Crush, Goose. VS Code's and OpenAI's logos were removed
from Simple Icons at their owners' request, which is why those two come from sets that carry them
with permission rather than being re-drawn here.

The `devsignal` mark is our own.

Every other logo is a trademark of its owner. They appear here only to identify the app devsignal
detected, which is nominative use; no endorsement or affiliation is implied. If you own one of these
marks and want it removed, open an issue and it will be replaced with a monogram.
