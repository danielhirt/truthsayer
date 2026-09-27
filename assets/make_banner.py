# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50", "uharfbuzz>=0.39"]
# ///
"""Draw the README banner from an eval run.

Each dot is one labeled case, placed at the judge's mean P(yes) across
repeats. Filled dots are cases whose true answer is yes; rings are cases
whose true answer is no. Text is converted to paths, so the SVG needs no
font on the viewer's side.

    uv run assets/make_banner.py evals/runs/2026-09-26-jev

Writes assets/banner-light.svg, assets/banner-dark.svg, and
assets/social-preview.png (the last needs rsvg-convert).
"""

import json
import math
import subprocess
import sys
import tomllib
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

FONT_DIR = Path("/usr/share/fonts/OTF")
FONTS = {w: FONT_DIR / f"Geist-{w}.otf" for w in ("Regular", "Medium", "SemiBold")}

W, H = 1280, 400
LEFT, RIGHT = 72, 1208
BASE = 318  # the axis line
R = 4.6  # dot radius
GAP = 1.4  # space between dots

THEMES = {
    "light": {
        "surface": "#ffffff",
        "ink": "#1f2328",
        "muted": "#59636e",
        "axis": "#d1d9e0",
        "band": "#818b98",
        "band_opacity": 0.10,
        "yes": "#2a78d6",
        "no": "#eb6834",
    },
    "dark": {
        "surface": "#0d1117",
        "ink": "#f0f6fc",
        "muted": "#9198a1",
        "axis": "#3d444d",
        "band": "#9198a1",
        "band_opacity": 0.12,
        "yes": "#3987e5",
        "no": "#d95926",
    },
}


class Face:
    def __init__(self, path: Path):
        self.tt = TTFont(path)
        self.glyphs = self.tt.getGlyphSet()
        self.order = self.tt.getGlyphOrder()
        self.upem = self.tt["head"].unitsPerEm
        blob = hb.Blob.from_file_path(str(path))
        self.hb_font = hb.Font(hb.Face(blob))

    def shape(self, text: str):
        buf = hb.Buffer()
        buf.add_str(text)
        buf.guess_segment_properties()
        hb.shape(self.hb_font, buf, {"kern": True, "liga": True})
        return buf.glyph_infos, buf.glyph_positions

    def width(self, text: str, size: float, tracking: float = 0.0) -> float:
        _, pos = self.shape(text)
        scale = size / self.upem
        return sum(p.x_advance * scale + tracking for p in pos) - tracking

    def path(self, text: str, x: float, y: float, size: float, tracking: float = 0.0) -> str:
        """SVG path data for `text` with its baseline at y."""
        infos, pos = self.shape(text)
        scale = size / self.upem
        pen = SVGPathPen(self.glyphs)
        cx = x
        for info, p in zip(infos, pos):
            name = self.order[info.codepoint]
            t = TransformPen(pen, (scale, 0, 0, -scale, cx + p.x_offset * scale, y - p.y_offset * scale))
            self.glyphs[name].draw(t)
            cx += p.x_advance * scale + tracking
        return pen.getCommands()


def load_points(run: Path):
    """(mean P(yes), truth) for each case in the run."""
    cases = {}
    for f in sorted(Path("evals/synthetic").glob("*.toml")):
        for c in tomllib.loads(f.read_text())["case"]:
            cases[c["id"]] = (c["question"], c["truth"])
    ps: dict[str, list[float]] = {}
    for line in (run / "records.jsonl").read_text().splitlines():
        rec = json.loads(line)
        cid = rec["labels"].get("case")
        if cid not in cases:
            continue
        question, _ = cases[cid]
        for v in rec["report"]["verdicts"]:
            if f'{v["rubric"]}.{v["question"]}' == question and "noul" in v["answer"]:
                ps.setdefault(cid, []).append(v["answer"]["noul"])
    return [(sum(p) / len(p), cases[cid][1]) for cid, p in ps.items()]


def x_of(p: float) -> float:
    return LEFT + p * (RIGHT - LEFT)


def swarm(points, nudge: float = 10.0):
    """Stack dots upward from the axis without overlap. A dot may move
    sideways by up to `nudge` pixels (under 0.01 on the axis) when that
    lets it sit lower; the lowest spot with the smallest shift wins."""
    placed: list[tuple[float, float]] = []
    out = []
    d = 2 * R + GAP
    floor = BASE - R - 3

    def lowest(x: float) -> float:
        near = [(px, py) for px, py in placed if abs(px - x) < d]
        candidates = [floor] + [py - math.sqrt(max(d * d - (px - x) ** 2, 0)) for px, py in near]
        for y in sorted(c for c in candidates if c <= floor)[::-1]:
            if all((px - x) ** 2 + (py - y) ** 2 >= d * d - 1e-6 for px, py in near):
                return y
        return min(candidates) - d

    for p, truth in sorted(points, key=lambda t: t[0]):
        x0 = x_of(p)
        best = None
        for step in range(int(nudge) + 1):
            for x in {x0 - step, x0 + step}:
                if not LEFT <= x <= RIGHT:
                    continue
                y = lowest(x)
                if best is None or y > best[1] + 0.5:
                    best = (x, y)
        x, y = best
        placed.append((x, y))
        out.append((x, y, truth))
    return out


def svg(theme: dict, dots, faces) -> str:
    reg, med, semi = faces["Regular"], faces["Medium"], faces["SemiBold"]
    t = theme
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" '
        f'aria-labelledby="t d">',
        "<title id=\"t\">truthsayer</title>",
        '<desc id="d">The model scores. Code decides. Each dot is one labeled eval case, placed at the '
        "judge's probability that the answer is yes. Filled dots are true yes cases and rings are true no "
        "cases; they gather at opposite ends, and few fall in the uncertain band between 0.3 and 0.7.</desc>",
    ]

    def text(face, s, x, y, size, fill, tracking=0.0, anchor="start"):
        if anchor == "middle":
            x -= face.width(s, size, tracking) / 2
        elif anchor == "end":
            x -= face.width(s, size, tracking)
        parts.append(f'<path fill="{fill}" d="{face.path(s, x, y, size, tracking)}"/>')

    # Wordmark and line.
    mid = (x_of(0.3) + x_of(0.7)) / 2
    text(semi, "truthsayer", mid, 112, 76, t["ink"], tracking=-2.2, anchor="middle")
    text(reg, "The model scores. Code decides.", mid, 156, 25, t["muted"], tracking=-0.1, anchor="middle")

    # Uncertain band and the rule threshold.
    x3, x7 = x_of(0.3), x_of(0.7)
    parts.append(
        f'<rect x="{x3:.1f}" y="212" width="{x7 - x3:.1f}" height="{BASE - 212}" '
        f'fill="{t["band"]}" fill-opacity="{t["band_opacity"]}"/>'
    )
    text(med, "uncertain", (x3 + x7) / 2, 238, 15, t["muted"], anchor="middle")
    parts.append(
        f'<line x1="{x7:.1f}" y1="212" x2="{x7:.1f}" y2="{BASE}" stroke="{t["muted"]}" '
        f'stroke-width="1.5" stroke-dasharray="3 4"/>'
    )
    text(med, "rules act at 0.7", x7 + 10, 238, 15, t["muted"])

    # Axis.
    parts.append(
        f'<line x1="{LEFT}" y1="{BASE + 0.75}" x2="{RIGHT}" y2="{BASE + 0.75}" stroke="{t["axis"]}" stroke-width="1.5"/>'
    )
    for p, label in ((0, "0"), (0.3, "0.3"), (0.7, "0.7"), (1, "1")):
        x = x_of(p)
        parts.append(f'<line x1="{x:.1f}" y1="{BASE}" x2="{x:.1f}" y2="{BASE + 7}" stroke="{t["axis"]}" stroke-width="1.5"/>')
        text(reg, label, x, BASE + 27, 14, t["muted"], anchor="middle")

    # Dots: no cases first, so yes cases sit on top where they meet.
    for x, y, truth in sorted(dots, key=lambda d: d[2]):
        if truth:
            parts.append(
                f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{R}" fill="{t["yes"]}" stroke="{t["surface"]}" stroke-width="1"/>'
            )
        else:
            parts.append(
                f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{R - 0.9:.1f}" fill="{t["surface"]}" stroke="{t["no"]}" stroke-width="1.8"/>'
            )

    # Legend row under the axis, centered.
    ly = BASE + 64
    items = [("no", "true answer no"), ("yes", "true answer yes"), (None, "180 labeled eval cases, placed at the judge's P(yes), jev-1.13.0")]
    widths = [reg.width(s, 15) + (12 + R if kind else 0) for kind, s in items]
    x = mid - (sum(widths) + 28 * (len(items) - 1)) / 2
    for (kind, s), w in zip(items, widths):
        if kind == "no":
            parts.append(f'<circle cx="{x + R:.1f}" cy="{ly - 5}" r="{R - 0.9:.1f}" fill="{t["surface"]}" stroke="{t["no"]}" stroke-width="1.8"/>')
        elif kind == "yes":
            parts.append(f'<circle cx="{x + R:.1f}" cy="{ly - 5}" r="{R}" fill="{t["yes"]}"/>')
        text(reg, s, x + (12 + R if kind else 0), ly, 15, t["muted"])
        x += w + 28

    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def main():
    run = Path(sys.argv[1] if len(sys.argv) > 1 else "evals/runs/2026-09-26-jev")
    faces = {w: Face(p) for w, p in FONTS.items()}
    dots = swarm(load_points(run))
    top = min(y for _, y, _ in dots)
    print(f"{len(dots)} dots, tallest stack reaches y={top:.0f}")
    out = Path("assets")
    for name, theme in THEMES.items():
        (out / f"banner-{name}.svg").write_text(svg(theme, dots, faces))
    # GitHub's social preview wants 1280x640 on a solid surface.
    light = (out / "banner-light.svg").read_text()
    framed = light.replace(
        f'width="{W}" height="{H}" viewBox="0 0 {W} {H}"',
        f'width="1280" height="640" viewBox="0 -120 {W} 640"',
    ).replace('aria-labelledby="t d">', 'aria-labelledby="t d"><rect x="0" y="-120" width="1280" height="640" fill="#ffffff"/>', 1)
    tmp = out / ".social.svg"
    tmp.write_text(framed)
    subprocess.run(["rsvg-convert", "-o", str(out / "social-preview.png"), str(tmp)], check=True)
    tmp.unlink()


if __name__ == "__main__":
    main()
