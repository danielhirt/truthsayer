# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50", "uharfbuzz>=0.39"]
# ///
"""Draw the README header and the GitHub social preview.

A spoken claim, drawn as a waveform with one burst for each word, passes
through a ring and leaves as one straight line that ends in a verdict.
The drawing holds no data, so it does not change as the project does.

    uv run assets/make_header.py

Writes assets/header-light.svg, assets/header-dark.svg, and
assets/social-preview.png (needs rsvg-convert).
"""

import math
import subprocess
from pathlib import Path

from glyphs import FONTS, Face

W, H = 1280, 320
CLAIM = "all tests pass now"
TAGLINE = "The model scores. Code decides."

THEMES = {
    "light": {"surface": "#ffffff", "ink": "#1f2328", "muted": "#59636e", "accent": "#6e56cf"},
    "dark": {"surface": "#0d1117", "ink": "#f0f6fc", "muted": "#9198a1", "accent": "#9e8cfc"},
}

LINE_Y = 250
WAVE_X0, RING_X, RING_R = 88, 640, 17
END_X = 1180


def waveform() -> str:
    """Polyline points for the spoken claim: one burst for each word,
    quiet between words, and flat for the last stretch into the ring."""
    words = CLAIM.split()
    span = RING_X - RING_R - 44 - WAVE_X0
    gap = 26
    unit = (span - gap * (len(words) - 1)) / sum(len(w) for w in words)
    pts = [(WAVE_X0, LINE_Y)]
    x = WAVE_X0
    for wi, word in enumerate(words):
        width = unit * len(word)
        steps = int(width * 2.2)
        for i in range(1, steps + 1):
            u = i / steps
            ch = word[min(int(u * len(word)), len(word) - 1)]
            env = math.sin(math.pi * u) ** 1.6
            loud = 0.62 + 0.38 * ((ord(ch) * 7) % 11) / 10
            freq = 0.36 + ((ord(ch) * 13) % 9) * 0.035
            px = x + width * u
            pts.append((px, LINE_Y + 30 * env * loud * math.sin(px * freq)))
        x += width
        if wi < len(words) - 1:
            steps = int(gap * 1.5)
            for i in range(1, steps + 1):
                px = x + gap * i / steps
                pts.append((px, LINE_Y + 1.6 * math.sin(px * 0.9)))
            x += gap
    pts.append((RING_X - RING_R, LINE_Y))
    return " ".join(f"{px:.1f},{py:.1f}" for px, py in pts)


def header(t: dict, faces: dict, background: bool = False, dy: float = 0) -> list[str]:
    reg, semi = faces["Regular"], faces["SemiBold"]
    parts = []
    if background:
        parts.append(f'<rect x="0" y="{-dy}" width="1280" height="640" fill="{t["surface"]}"/>')

    def centered(face, s, y, size, fill, tracking):
        x = (W - face.width(s, size, tracking)) / 2
        parts.append(f'<path fill="{fill}" d="{face.path(s, x, y, size, tracking)}"/>')

    centered(semi, "truthsayer", 112, 92, t["ink"], -2.6)
    centered(reg, TAGLINE, 158, 26, t["muted"], -0.1)
    parts += [
        '<defs><linearGradient id="fade" gradientUnits="userSpaceOnUse" '
        f'x1="{WAVE_X0}" y1="0" x2="{RING_X}" y2="0">'
        f'<stop offset="0" stop-color="{t["muted"]}" stop-opacity="0.25"/>'
        f'<stop offset="1" stop-color="{t["muted"]}" stop-opacity="1"/></linearGradient></defs>',
        f'<polyline points="{waveform()}" fill="none" stroke="url(#fade)" stroke-width="2" '
        'stroke-linejoin="round" stroke-linecap="round"/>',
        f'<circle cx="{RING_X}" cy="{LINE_Y}" r="{RING_R}" fill="none" stroke="{t["ink"]}" stroke-width="3"/>',
        f'<line x1="{RING_X + RING_R}" y1="{LINE_Y}" x2="{END_X - 9}" y2="{LINE_Y}" stroke="{t["ink"]}" stroke-width="3"/>',
        f'<circle cx="{END_X}" cy="{LINE_Y}" r="9" fill="{t["accent"]}"/>',
    ]
    return parts


def document(parts: list[str], width: int, height: int, view: str) -> str:
    return "\n".join(
        [
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="{view}" '
            'role="img" aria-labelledby="t d">',
            '<title id="t">truthsayer</title>',
            '<desc id="d">The model scores. Code decides. A spoken claim, drawn as a waveform, passes '
            "through a ring and leaves as one straight line that ends in a single point.</desc>",
            *parts,
            "</svg>",
        ]
    ) + "\n"


def main():
    faces = {w: Face(p) for w, p in FONTS.items()}
    out = Path(__file__).parent
    for name, theme in THEMES.items():
        (out / f"header-{name}.svg").write_text(document(header(theme, faces), W, H, f"0 0 {W} {H}"))
    # GitHub's social preview: 1280x640 on a solid surface, drawing centered.
    dy = (640 - H) / 2
    social = out / ".social.svg"
    social.write_text(document(header(THEMES["dark"], faces, background=True, dy=dy), 1280, 640, f"0 {-dy} 1280 640"))
    subprocess.run(["rsvg-convert", "-o", str(out / "social-preview.png"), str(social)], check=True)
    social.unlink()


if __name__ == "__main__":
    main()
