# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50", "uharfbuzz>=0.39"]
# ///
"""Draw the plot of an eval run.

Each dot is one labeled case, placed at the judge's mean P(yes) across
repeats. Filled dots are cases whose true answer is yes; rings are cases
whose true answer is no. Text is converted to paths, so the SVG needs no
font on the viewer's side.

    uv run assets/make_eval_plot.py evals/runs/2026-09-26-jev

Writes plot-light.svg and plot-dark.svg into the run directory.
"""

import json
import math
import sys
import tomllib
from pathlib import Path

from glyphs import FONTS, Face

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


def svg(theme: dict, dots, faces, meta: dict) -> str:
    reg, med, semi = faces["Regular"], faces["Medium"], faces["SemiBold"]
    t = theme
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" '
        f'aria-labelledby="t d">',
        "<title id=\"t\">Judge answers on the synthetic eval cases</title>",
        '<desc id="d">Each dot is one labeled eval case, placed at the '
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
    text(semi, "Where the judge puts each case", mid, 104, 40, t["ink"], tracking=-0.8, anchor="middle")
    text(reg, f'{meta["cases"]} labeled synthetic cases, the mean of {meta["repeat"]} answers each, {meta["model"]}', mid, 146, 22, t["muted"], anchor="middle")

    # Uncertain band and the rule threshold.
    x3, x7 = x_of(0.3), x_of(0.7)
    parts.append(
        f'<rect x="{x3:.1f}" y="212" width="{x7 - x3:.1f}" height="{BASE - 212}" '
        f'fill="{t["band"]}" fill-opacity="{t["band_opacity"]}"/>'
    )
    text(med, "uncertain", (x3 + x7) / 2, 240, 18, t["muted"], anchor="middle")
    parts.append(
        f'<line x1="{x7:.1f}" y1="212" x2="{x7:.1f}" y2="{BASE}" stroke="{t["muted"]}" '
        f'stroke-width="1.5" stroke-dasharray="3 4"/>'
    )
    text(med, "rules act at 0.7", x7 + 10, 240, 18, t["muted"])

    # Axis.
    parts.append(
        f'<line x1="{LEFT}" y1="{BASE + 0.75}" x2="{RIGHT}" y2="{BASE + 0.75}" stroke="{t["axis"]}" stroke-width="1.5"/>'
    )
    for p, label in ((0, "0"), (0.3, "0.3"), (0.7, "0.7"), (1, "1")):
        x = x_of(p)
        parts.append(f'<line x1="{x:.1f}" y1="{BASE}" x2="{x:.1f}" y2="{BASE + 7}" stroke="{t["axis"]}" stroke-width="1.5"/>')
        text(reg, label, x, BASE + 30, 18, t["muted"], anchor="middle")

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
    ly = BASE + 70
    items = [("no", "true answer no"), ("yes", "true answer yes"), (None, "x: the judge's P(yes)")]
    widths = [reg.width(s, 18) + (12 + R if kind else 0) for kind, s in items]
    x = mid - (sum(widths) + 28 * (len(items) - 1)) / 2
    for (kind, s), w in zip(items, widths):
        if kind == "no":
            parts.append(f'<circle cx="{x + R:.1f}" cy="{ly - 6}" r="{R - 0.9:.1f}" fill="{t["surface"]}" stroke="{t["no"]}" stroke-width="1.8"/>')
        elif kind == "yes":
            parts.append(f'<circle cx="{x + R:.1f}" cy="{ly - 6}" r="{R}" fill="{t["yes"]}"/>')
        text(reg, s, x + (12 + R if kind else 0), ly, 18, t["muted"])
        x += w + 28

    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def main():
    run = Path(sys.argv[1] if len(sys.argv) > 1 else "evals/runs/2026-09-26-jev")
    faces = {w: Face(p) for w, p in FONTS.items()}
    meta = json.loads((run / "run.json").read_text())
    dots = swarm(load_points(run))
    top = min(y for _, y, _ in dots)
    print(f"{len(dots)} dots, tallest stack reaches y={top:.0f}")
    for name, theme in THEMES.items():
        (run / f"plot-{name}.svg").write_text(svg(theme, dots, faces, meta))

if __name__ == "__main__":
    main()
