#!/usr/bin/env python3
"""Render the Discord Rich Presence art assets in `assets/discord/`.

Discord accepts either an uploaded art-asset **key** or a plain `https://` image URL in
`assets.large_image` / `assets.small_image`, so the PNGs this writes serve both paths: point
`[images] base_url` at the raw GitHub URL for the folder, or upload the same files under
Developer Portal → Rich Presence → Art Assets and keep `mode = "key"`.

Each app is drawn as **its own logo** — the real multi-colour app marks, not one-colour
silhouettes — on a tile tinted toward that logo's own dominant colour. That is what holds the set
together as a set while every icon stays recognisable at Discord's 64px corner size:

  * tile      512x512, 22% corner radius, tinted 16% toward the logo's dominant colour, thin ring
  * glyph     56% of the tile, centred, so it survives the circular crop of the small slot
  * polarity  a dark mark (Cursor, Apple, OpenAI, JetBrains) gets a **light** tile; everything
              else gets a dark one. Chosen by measured luminance, not by hand.

Glyph data is vendored in `assets/discord/sources.json` so a rebuild needs no network. See
`assets/discord/README.md` for provenance and the trademark note.

To use a different icon pack for some or all apps, drop `<name>.png` or `<name>.svg` into
`assets/discord/overrides/{agents,hosts}/` — an override is used verbatim instead of the generated
tile. Only add art you have the right to redistribute.

Requirements: `pip install pillow cairosvg`. Run from anywhere:

    python3 scripts/build-discord-assets.py [--check]

`--check` re-renders into a temp dir and fails if the committed PNGs differ, which is what CI
would run to catch a hand-edited asset.
"""

from __future__ import annotations

import argparse
import colorsys
import io
import json
import pathlib
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
# Hand-supplied art that wins over anything generated — see load_override.
OVERRIDES = OUT_DIR / "overrides"

SIZE = 512
CORNER_RADIUS = int(SIZE * 0.22)
# Discord crops the small image to a circle, so the glyph stays inside the inscribed circle.
GLYPH_FRACTION = 0.56
DARK_BASE = (23, 24, 29)
LIGHT_BASE = (238, 240, 245)
TINT_STRENGTH = 0.16
RING_STRENGTH = 0.40
# Below this mean glyph luminance, a dark tile would swallow the mark: flip to the light tile.
# Kept low on purpose — at 0.30 the JetBrains SKUs split across both polarities, which read as two
# unrelated sets rather than one. Only marks that are essentially black flip now.
DARK_MARK_LUMINANCE = 0.20
# A one-colour glyph is tinted to its brand colour, brightened until it clears this on dark.
MIN_GLYPH_LUMINANCE = 0.45

FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
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


def icon(key: str) -> dict:
    """A vendored logo, by its key in sources.json."""
    return {"kind": "icon", "key": key}


def mono(text: str, color: str) -> dict:
    """Fallback for an app with no logo in any CC0/MIT set — a monogram, not a traced look-alike."""
    return {"kind": "monogram", "text": text, "color": color}


# Agent large images, keyed by `[[agents]] id` — `<id>.png` is what url mode requests, so a new
# preset needs an entry here or its large image 404s.
AGENTS: dict[str, dict] = {
    "devsignal": {"kind": "custom", "svg": DEVSIGNAL_GLYPH, "color": "#7C8CFF"},
    "claude_code": icon("claude"),
    "codex": icon("openai"),
    "opencode": icon("opencode"),
    "cursor_agent": icon("cursor"),
    # docs/community-presets.md agents, so a promoted preset already has art.
    "gemini_cli": icon("gemini"),
    "copilot_cli": icon("copilot"),
    "qwen_code": icon("qwen"),
    "cline": icon("cline"),
    "droid": icon("android"),
    "amp": mono("A", "#F97316"),
    "aider": mono("AI", "#22C55E"),
    "crush": mono("CR", "#F472B6"),
    "goose": mono("GO", "#38BDF8"),
}

# Host small images, keyed by the image slug in `HOST_APPS` (devsignal-core).
HOSTS: dict[str, dict] = {
    "claude_desktop": icon("claude"),
    "cursor": icon("cursor"),
    "vs_code": icon("vscode"),
    "vscodium": icon("vscodium"),
    "zed": icon("zed"),
    "xcode": icon("xcode"),
    "sublime_text": icon("sublimetext"),
    "nova": mono("N", "#6E5AE6"),
    "fleet": icon("fleet"),
    "intellij_idea": icon("intellij"),
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
    "terminal": icon("terminal"),
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


def relative_luminance(rgb: tuple[int, int, int]) -> float:
    r, g, b = (c / 255 for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def parse_hex(value: str) -> tuple[int, int, int]:
    h = value.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))  # type: ignore[return-value]


def to_hex(rgb: tuple[int, int, int]) -> str:
    return "#{:02X}{:02X}{:02X}".format(*rgb)


def brighten(rgb: tuple[int, int, int], target: float) -> tuple[int, int, int]:
    """Raise lightness, keeping hue and saturation, until the colour clears `target` luminance."""
    if relative_luminance(rgb) >= target:
        return rgb
    h, l, s = colorsys.rgb_to_hls(*(c / 255 for c in rgb))
    if s < 0.05:  # a grey or black mark has no hue to preserve — go white
        return (255, 255, 255)
    for step in range(1, 101):
        cand = colorsys.hls_to_rgb(h, min(1.0, l + step / 100), s)
        out = tuple(round(c * 255) for c in cand)
        if relative_luminance(out) >= target:  # type: ignore[arg-type]
            return out  # type: ignore[return-value]
    return (255, 255, 255)


def mix(colour: tuple[int, int, int], base: tuple[int, int, int], t: float) -> tuple[int, int, int]:
    return tuple(round(base[i] + (colour[i] - base[i]) * t) for i in range(3))  # type: ignore


def render_svg(inner: str, view_w: float, view_h: float, px: int) -> Image.Image:
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view_w} {view_h}" '
        f'width="{px}" height="{px}">{inner}</svg>'
    )
    png = cairosvg.svg2png(bytestring=svg.encode(), output_width=px, output_height=px)
    return Image.open(io.BytesIO(png)).convert("RGBA")


def glyph_stats(img: Image.Image) -> tuple[tuple[int, int, int], float]:
    """Dominant colour and mean luminance over the opaque pixels."""
    small = img.resize((64, 64))
    total = [0, 0, 0]
    lum = 0.0
    n = 0
    # `getdata` is deprecated in Pillow 13 in favour of `get_flattened_data`; support both so the
    # script keeps working on whatever Pillow the contributor happens to have.
    pixels = small.get_flattened_data() if hasattr(small, "get_flattened_data") else small.getdata()
    for r, g, b, a in pixels:
        if a < 110:
            continue
        total[0] += r
        total[1] += g
        total[2] += b
        lum += relative_luminance((r, g, b))
        n += 1
    if not n:
        return (140, 150, 170), 0.5
    dominant = tuple(c // n for c in total)
    return dominant, lum / n  # type: ignore[return-value]


def tile(bg: tuple[int, int, int], ring: tuple[int, int, int]) -> Image.Image:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.rounded_rectangle((0, 0, SIZE - 1, SIZE - 1), radius=CORNER_RADIUS, fill=bg + (255,))
    draw.rounded_rectangle(
        (3, 3, SIZE - 4, SIZE - 4),
        radius=CORNER_RADIUS - 3,
        outline=ring + (255,),
        width=6,
    )
    return img


def compose(glyph: Image.Image, dominant: tuple[int, int, int], on_light: bool) -> Image.Image:
    base = LIGHT_BASE if on_light else DARK_BASE
    img = tile(mix(dominant, base, TINT_STRENGTH), mix(dominant, base, RING_STRENGTH))
    img.alpha_composite(glyph, ((SIZE - glyph.width) // 2, (SIZE - glyph.height) // 2))
    return img


def load_font(px: int) -> ImageFont.FreeTypeFont:
    for candidate in FONT_CANDIDATES:
        if pathlib.Path(candidate).exists():
            return ImageFont.truetype(candidate, px)
    raise SystemExit(
        "no bold sans font found; add one to FONT_CANDIDATES in scripts/build-discord-assets.py"
    )


def render_monogram(text: str, colour: str) -> Image.Image:
    accent = parse_hex(colour)
    img = tile(mix(accent, DARK_BASE, TINT_STRENGTH), mix(accent, DARK_BASE, RING_STRENGTH))
    draw = ImageDraw.Draw(img)
    box = int(SIZE * 0.42)
    px = box
    while px > 8:
        font = load_font(px)
        left, top, right, bottom = draw.textbbox((0, 0), text, font=font)
        if right - left <= box and bottom - top <= box:
            break
        px -= 4
    left, top, right, bottom = draw.textbbox((0, 0), text, font=font)
    draw.text(
        ((SIZE - (right - left)) / 2 - left, (SIZE - (bottom - top)) / 2 - top),
        text,
        font=font,
        fill=to_hex(brighten(accent, MIN_GLYPH_LUMINANCE)),
    )
    return img


def render(spec: dict, sources: dict) -> Image.Image:
    glyph_px = int(SIZE * GLYPH_FRACTION)

    if spec["kind"] == "monogram":
        return render_monogram(spec["text"], spec["color"])

    if spec["kind"] == "custom":
        accent = parse_hex(spec["color"])
        glyph = render_svg(spec["svg"].format(color=spec["color"]), 24, 24, glyph_px)
        return compose(glyph, accent, on_light=False)

    src = sources.get(spec["key"])
    if src is None:
        raise SystemExit(f"sources.json has no icon {spec['key']!r}")

    body = src["body"]
    # One-colour glyphs arrive as `currentColor`; give them their brand colour, brightened enough
    # to read on the dark tile. Multi-colour marks are left exactly as their owner drew them.
    if "currentColor" in body:
        brand = parse_hex(src.get("hex") or "888888")
        body = body.replace("currentColor", to_hex(brighten(brand, MIN_GLYPH_LUMINANCE)))

    glyph = render_svg(body, src.get("width", 24), src.get("height", 24), glyph_px)
    dominant, luminance = glyph_stats(glyph)
    return compose(glyph, dominant, on_light=luminance < DARK_MARK_LUMINANCE)


def load_override(folder: str, name: str) -> Image.Image | None:
    """Use a hand-supplied image in place of the generated one, if there is one.

    This is the seam for a third-party icon pack: drop `<name>.png` (or `.svg`) into
    `assets/discord/overrides/{agents,hosts}/` and it wins, no code change. Only put art here that
    you have the right to redistribute — the folder is published to GitHub like everything else, and
    most icon packs sold or posted on design sites are licensed for personal use only.
    """
    folder_dir = OVERRIDES / folder
    png = folder_dir / f"{name}.png"
    if png.exists():
        img = Image.open(png).convert("RGBA")
        return img if img.size == (SIZE, SIZE) else img.resize((SIZE, SIZE), Image.LANCZOS)
    svg = folder_dir / f"{name}.svg"
    if svg.exists():
        raw = cairosvg.svg2png(url=str(svg), output_width=SIZE, output_height=SIZE)
        return Image.open(io.BytesIO(raw)).convert("RGBA")
    return None


def build(out_dir: pathlib.Path) -> tuple[list[pathlib.Path], list[str]]:
    sources = json.loads(SOURCES.read_text())["icons"]
    written = []
    overridden = []
    for folder, table in (("agents", AGENTS), ("hosts", HOSTS)):
        target = out_dir / folder
        target.mkdir(parents=True, exist_ok=True)
        for name, spec in sorted(table.items()):
            path = target / f"{name}.png"
            override = load_override(folder, name)
            if override is not None:
                overridden.append(f"{folder}/{name}")
                override.save(path, "PNG", optimize=True)
            else:
                render(spec, sources).save(path, "PNG", optimize=True)
            written.append(path)
    return written, overridden


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="render to a temp dir and fail if the committed PNGs differ",
    )
    args = ap.parse_args()

    if not args.check:
        written, overridden = build(OUT_DIR)
        print(f"wrote {len(written)} PNGs under {OUT_DIR.relative_to(REPO)}")
        if overridden:
            print(f"  {len(overridden)} from overrides/: {', '.join(overridden)}")
        return 0

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = pathlib.Path(tmp)
        drift = []
        rendered, _ = build(tmp_dir)
        for path in rendered:
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
