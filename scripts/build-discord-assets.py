#!/usr/bin/env python3
"""Render the Discord Rich Presence art assets in `assets/discord/`.

Discord accepts either an uploaded art-asset **key** or a plain `https://` image URL in
`assets.large_image` / `assets.small_image`, so the PNGs this writes serve both paths: point
`[images] base_url` at the raw GitHub URL for the folder, or upload the same files under
Developer Portal → Rich Presence → Art Assets and keep `mode = "key"`.

Brand glyphs come from `assets/discord/sources.json`, extracted from the simple-icons npm package
(CC0 1.0). Apps simple-icons does not carry — VS Code, Apple Terminal, Kitty, Tabby, RustRover,
Fleet, Nova, Codex — get a monogram tile instead of a look-alike logo, so nothing here is a traced
copy of a trademark that its owner asked to have removed.

Requirements: `pip install pillow cairosvg`. Run from anywhere:

    python3 scripts/build-discord-assets.py [--check]

`--check` re-renders into a temp dir and fails if the committed PNGs differ, which is what CI
would run to catch a hand-edited asset.
"""

from __future__ import annotations

import argparse
import io
import json
import pathlib
import shutil
import sys
import tempfile

try:
    import cairosvg
    from PIL import Image, ImageDraw, ImageFont
except ImportError as exc:  # pragma: no cover - developer convenience
    sys.exit(f"missing dependency ({exc}); run: pip install pillow cairosvg")

REPO = pathlib.Path(__file__).resolve().parent.parent
SOURCES = REPO / "assets" / "discord" / "sources.json"
OUT_DIR = REPO / "assets" / "discord"

SIZE = 512
CORNER_RADIUS = int(SIZE * 0.22)
# Discord crops the small image to a circle, so every glyph stays inside the inscribed circle.
GLYPH_FRACTION = 0.54
BACKGROUND = (23, 24, 29, 255)
# A glyph this dark would vanish against BACKGROUND; those brands render white instead.
MIN_LUMINANCE = 0.22

FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/System/Library/Fonts/HelveticaNeue.ttc",
    "/Library/Fonts/Arial Bold.ttf",
]

# devsignal's own mark: a broadcast dot with two pairs of arcs, drawn rather than borrowed.
DEVSIGNAL_GLYPH = """
<circle cx="12" cy="12" r="2.5" fill="{color}"/>
<path d="M7.6 7.0a7.0 7.0 0 0 0 0 10.0" fill="none" stroke="{color}"
      stroke-width="1.8" stroke-linecap="round"/>
<path d="M16.4 7.0a7.0 7.0 0 0 1 0 10.0" fill="none" stroke="{color}"
      stroke-width="1.8" stroke-linecap="round"/>
<path d="M3.9 3.6a11.4 11.4 0 0 0 0 16.8" fill="none" stroke="{color}"
      stroke-width="1.8" stroke-linecap="round" opacity="0.55"/>
<path d="M20.1 3.6a11.4 11.4 0 0 1 0 16.8" fill="none" stroke="{color}"
      stroke-width="1.8" stroke-linecap="round" opacity="0.55"/>
"""


def icon(slug: str) -> dict:
    return {"kind": "icon", "slug": slug}


def mono(text: str, color: str) -> dict:
    return {"kind": "monogram", "text": text, "color": color}


def word(text: str, color: str) -> dict:
    return {"kind": "wordmark", "text": text, "color": color}


# Agent large images. Keys are `[[agents]]` ids — `<id>.png` is what url mode requests, so a new
# preset needs an entry here or its large image 404s.
AGENTS: dict[str, dict] = {
    "devsignal": {"kind": "custom", "svg": DEVSIGNAL_GLYPH, "color": "#5865F2"},
    "claude_code": icon("claude"),
    # OpenAI's mark was pulled from simple-icons at the owner's request; use a wordmark.
    "codex": word("codex", "#FFFFFF"),
    "opencode": icon("opencode"),
    "cursor_agent": icon("cursor"),
    # docs/community-presets.md agents, so a promoted preset already has art.
    "gemini_cli": icon("googlegemini"),
    "copilot_cli": icon("githubcopilot"),
    "qwen_code": icon("qwen"),
    "cline": icon("cline"),
    "droid": icon("android"),
    "amp": mono("A", "#F97316"),
    "aider": mono("AI", "#22C55E"),
    "crush": mono("CR", "#F472B6"),
    "goose": mono("GO", "#38BDF8"),
}

# Host small images, keyed by the image slug in `HOST_BUNDLE_LABELS` (devsignal-core).
HOSTS: dict[str, dict] = {
    "claude_desktop": icon("claude"),
    "cursor": icon("cursor"),
    # Microsoft asked for the VS Code logo to be removed from simple-icons; monogram in its blue.
    "vs_code": mono("VS", "#0078D4"),
    "vscodium": icon("vscodium"),
    "zed": icon("zedindustries"),
    "xcode": icon("xcode"),
    "sublime_text": icon("sublimetext"),
    "nova": mono("N", "#6E5AE6"),
    "fleet": mono("FL", "#87F7FF"),
    "intellij_idea": icon("intellijidea"),
    "pycharm": icon("pycharm"),
    "webstorm": icon("webstorm"),
    "goland": icon("goland"),
    "rubymine": icon("rubymine"),
    "clion": icon("clion"),
    "phpstorm": icon("phpstorm"),
    "rustrover": mono("RR", "#FE8C51"),
    "datagrip": icon("datagrip"),
    "aqua": icon("aqua"),
    "jetbrains": icon("jetbrains"),
    "android_studio": icon("androidstudio"),
    "terminal": mono(">_", "#FFFFFF"),
    "iterm2": icon("iterm2"),
    "warp": icon("warp"),
    "ghostty": icon("ghostty"),
    "kitty": mono("KT", "#7AA2F7"),
    "alacritty": icon("alacritty"),
    "hyper": icon("hyper"),
    "tabby": mono("TB", "#E4572E"),
    "wezterm": icon("wezterm"),
    # Shown when the frontmost app is unknown, which is also the `macOS` host label.
    "macos": icon("apple"),
}


def luminance(hex_color: str) -> float:
    h = hex_color.lstrip("#")
    r, g, b = (int(h[i : i + 2], 16) / 255 for i in (0, 2, 4))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def readable(hex_color: str) -> str:
    """Brand colour, unless it would disappear against the dark tile."""
    return "#FFFFFF" if luminance(hex_color) < MIN_LUMINANCE else f"#{hex_color.lstrip('#')}"


def tile() -> Image.Image:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle(
        (0, 0, SIZE - 1, SIZE - 1), radius=CORNER_RADIUS, fill=BACKGROUND
    )
    return img


def render_svg(inner: str, px: int) -> Image.Image:
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" '
        f'width="{px}" height="{px}">{inner}</svg>'
    )
    png = cairosvg.svg2png(bytestring=svg.encode(), output_width=px, output_height=px)
    return Image.open(io.BytesIO(png)).convert("RGBA")


def load_font(px: int) -> ImageFont.FreeTypeFont:
    for candidate in FONT_CANDIDATES:
        if pathlib.Path(candidate).exists():
            return ImageFont.truetype(candidate, px)
    raise SystemExit(
        "no bold sans font found; add one to FONT_CANDIDATES in scripts/build-discord-assets.py"
    )


def draw_text(img: Image.Image, text: str, color: str, box: int) -> None:
    """Fit `text` inside a `box`-wide square centred on the tile."""
    draw = ImageDraw.Draw(img)
    px = box
    while px > 8:
        font = load_font(px)
        left, top, right, bottom = draw.textbbox((0, 0), text, font=font)
        if right - left <= box and bottom - top <= box:
            break
        px -= 4
    left, top, right, bottom = draw.textbbox((0, 0), text, font=font)
    x = (SIZE - (right - left)) / 2 - left
    y = (SIZE - (bottom - top)) / 2 - top
    draw.text((x, y), text, font=font, fill=color)


def render(spec: dict, icons: dict) -> Image.Image:
    img = tile()
    glyph_px = int(SIZE * GLYPH_FRACTION)

    if spec["kind"] == "icon":
        src = icons.get(spec["slug"])
        if src is None:
            raise SystemExit(f"sources.json has no icon {spec['slug']!r}")
        color = readable(src["hex"])
        glyph = render_svg(f'<path d="{src["path"]}" fill="{color}"/>', glyph_px)
        img.alpha_composite(glyph, ((SIZE - glyph_px) // 2, (SIZE - glyph_px) // 2))
    elif spec["kind"] == "custom":
        glyph = render_svg(spec["svg"].format(color=spec["color"]), glyph_px)
        img.alpha_composite(glyph, ((SIZE - glyph_px) // 2, (SIZE - glyph_px) // 2))
    elif spec["kind"] == "monogram":
        draw_text(img, spec["text"], spec["color"], int(SIZE * 0.42))
    elif spec["kind"] == "wordmark":
        draw_text(img, spec["text"], spec["color"], int(SIZE * 0.56))
    else:  # pragma: no cover - guarded by the tables above
        raise SystemExit(f"unknown spec kind {spec['kind']!r}")

    return img


def build(out_dir: pathlib.Path) -> list[pathlib.Path]:
    icons = json.loads(SOURCES.read_text())["icons"]
    written = []
    for folder, table in (("agents", AGENTS), ("hosts", HOSTS)):
        target = out_dir / folder
        target.mkdir(parents=True, exist_ok=True)
        for name, spec in sorted(table.items()):
            path = target / f"{name}.png"
            render(spec, icons).save(path, "PNG", optimize=True)
            written.append(path)
    return written


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="render to a temp dir and fail if the committed PNGs differ",
    )
    args = ap.parse_args()

    if not args.check:
        written = build(OUT_DIR)
        print(f"wrote {len(written)} PNGs under {OUT_DIR.relative_to(REPO)}")
        return 0

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = pathlib.Path(tmp)
        drift = []
        for path in build(tmp_dir):
            committed = OUT_DIR / path.relative_to(tmp_dir)
            if not committed.exists() or committed.read_bytes() != path.read_bytes():
                drift.append(committed.relative_to(REPO))
        if drift:
            print("assets differ from a fresh render:", file=sys.stderr)
            for p in drift:
                print(f"  {p}", file=sys.stderr)
            print("run: python3 scripts/build-discord-assets.py", file=sys.stderr)
            return 1
    print("assets match a fresh render")
    return 0


if __name__ == "__main__":
    sys.exit(main())
