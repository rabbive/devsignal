# Presence art assets

512×512 PNGs for every agent CLI and host app devsignal knows about.

```
agents/<agent id>.png     # large image — claude_code.png, codex.png, …
hosts/<image slug>.png    # small corner icon — ghostty.png, vs_code.png, …
```

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

## Provenance and trademarks

Brand glyphs come from [simple-icons](https://www.npmjs.com/package/simple-icons) v16.27.1, whose
icon files are released under **CC0 1.0**. The extracted path data lives in `sources.json` so the
build is reproducible offline.

Apps simple-icons does not carry — VS Code, Apple Terminal, Kitty, Tabby, RustRover, Fleet, Nova,
Codex — get a **monogram tile** rather than a hand-traced look-alike. Two of those logos were removed
from simple-icons at their owners' request, and re-drawing them here would route around that.

The `devsignal` mark is our own.

Every other logo is a trademark of its owner. They appear here only to identify the app devsignal
detected, which is nominative use; no endorsement or affiliation is implied. If you own one of these
marks and want it removed, open an issue and it will be replaced with a monogram.
