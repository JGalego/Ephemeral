#!/usr/bin/env python3
"""Render the animated banner at docs/assets/banner.gif.

Bubbles nucleate in pairs out of nothing, drift apart, hold for a moment and
then burst — the way virtual particles do in a vacuum. It is a joke about
quantum field theory that happens to be the product thesis: software appears
when it is needed and is gone when it is not.

No text: the README supplies the words, the banner supplies the idea.

Run it with:

    python3 scripts/render-banner.py

Needs Pillow and numpy, which are not project dependencies — the output is
committed, so this only runs when the banner changes. Rendering is
deterministic (fixed seed), so re-running with the same parameters reproduces
the same file rather than creating a spurious diff.
"""

from __future__ import annotations

import argparse
import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image

# --- what the banner looks like ---------------------------------------------

WIDTH, HEIGHT = 1000, 200
FRAMES = 56
FRAME_MS = 90  # ~5s a loop: slow enough to watch one bubble live and die
SUPERSAMPLE = 2  # rendered large and downscaled, for edges that are not jagged
SEED = 0xB0BB1E

PAIR_COUNT = 15
FOAM_COUNT = 34

# How a bubble spends its life: it inflates, holds at full size for most of it,
# then bursts. The hold is what makes the burst read as an event rather than as
# a shrink.
INFLATE = 0.16
BURST = 0.13

BACKGROUND_TOP = (8, 9, 18)
BACKGROUND_BOTTOM = (17, 19, 38)

# A soap film's colour shifts with the angle you catch it at. Cyclic, so the
# interpolation wraps without a seam.
IRIDESCENCE = [
    (0.00, (167, 139, 250)),  # violet
    (0.25, (103, 232, 249)),  # cyan
    (0.50, (94, 234, 212)),   # teal
    (0.75, (240, 171, 252)),  # pink
    (1.00, (167, 139, 250)),
]


@dataclass
class Bubble:
    """One half of a pair, or a single speck of foam."""

    birth: float       # frame it nucleates on
    lifetime: float    # how many frames it lasts
    x: float           # where the pair is born, in supersampled pixels
    y: float
    radius: float      # size once inflated
    separation: float  # how far this half travels from the birth point
    angle: float       # the direction it travels in
    drift: float       # how far it rises over its life
    hue: float         # where it sits in the iridescent cycle


def iridescent(t: np.ndarray) -> np.ndarray:
    """Map t in [0, 1) onto the soap-film palette."""
    stops = np.array([s[0] for s in IRIDESCENCE])
    channels = [np.interp(t, stops, [s[1][c] for s in IRIDESCENCE]) for c in range(3)]
    return np.stack(channels, axis=-1) / 255.0


def ease_out(u: float) -> float:
    """Fast at first, settling gently — how a bubble actually inflates."""
    return 1.0 - (1.0 - u) ** 3


def background(width: int, height: int) -> np.ndarray:
    """A vertical gradient with a gentle vignette."""
    top = np.array(BACKGROUND_TOP, dtype=np.float64) / 255.0
    bottom = np.array(BACKGROUND_BOTTOM, dtype=np.float64) / 255.0

    ramp = np.linspace(0.0, 1.0, height)[:, None, None]
    canvas = top[None, None, :] * (1 - ramp) + bottom[None, None, :] * ramp
    canvas = np.repeat(canvas, width, axis=1)

    ys = (np.arange(height) / height - 0.5)[:, None]
    xs = (np.arange(width) / width - 0.5)[None, :]
    vignette = 1.0 - 0.38 * np.clip((xs * 1.4) ** 2 + (ys * 1.0) ** 2, 0, 1)
    return canvas * vignette[..., None]


def _window(canvas: np.ndarray, cx: float, cy: float, reach: float):
    """The clipped region a shape can touch, and its local coordinates."""
    height, width, _ = canvas.shape
    x0, x1 = max(0, int(cx - reach)), min(width, int(cx + reach) + 1)
    y0, y1 = max(0, int(cy - reach)), min(height, int(cy + reach) + 1)
    if x0 >= x1 or y0 >= y1:
        return None

    ys = np.arange(y0, y1)[:, None] - cy
    xs = np.arange(x0, x1)[None, :] - cx
    return (x0, x1, y0, y1), xs, ys


def add_bubble(
    canvas: np.ndarray,
    cx: float,
    cy: float,
    radius: float,
    intensity: float,
    hue: float,
) -> None:
    """An intact bubble, composited additively so overlaps glow like real films."""
    if radius < 0.6 or intensity <= 0.002:
        return

    window = _window(canvas, cx, cy, radius * 1.5 + 4)
    if window is None:
        return
    (x0, x1, y0, y1), xs, ys = window

    distance = np.hypot(xs, ys)

    # The rim: brightest where the film is edge-on to the viewer.
    thickness = max(1.1, radius * 0.085)
    rim = np.exp(-(((distance - radius) / thickness) ** 2))

    # The interior: a wash of colour across the whole disc, brightening toward
    # the edge. Without it the bubbles read as solid balls rather than film.
    inside = distance < radius
    interior = np.where(
        inside, 0.24 + 0.76 * np.clip(distance / max(radius, 1e-6), 0, 1) ** 2.4, 0.0
    )

    angle = np.arctan2(ys, xs) / (2 * math.pi)
    colour = iridescent((angle + hue + 0.22 * (distance / max(radius, 1e-6))) % 1.0)
    alpha = (rim * 0.95 + interior * 0.20) * intensity

    # The specular highlight, up and to the left, as on every drawn bubble since
    # the invention of the drawn bubble.
    hx, hy = cx - radius * 0.42, cy - radius * 0.46
    hr = max(radius * 0.20, 1.0)
    highlight = np.exp(
        -(
            (np.arange(x0, x1)[None, :] - hx) ** 2
            + (np.arange(y0, y1)[:, None] - hy) ** 2
        )
        / (2 * hr**2)
    )
    highlight *= intensity * 0.55

    patch = canvas[y0:y1, x0:x1]
    patch += colour * alpha[..., None]
    patch += highlight[..., None]


def add_burst(
    canvas: np.ndarray,
    cx: float,
    cy: float,
    radius: float,
    progress: float,
    hue: float,
) -> None:
    """A bubble mid-rupture.

    The film does not shrink — it gives way. The rim springs outward, tears into
    arcs that widen into gaps, and the fragments fly off and fade. That reading
    depends on the ring *growing* while it breaks up: a ring that contracts looks
    like a bubble being deflated by somebody.
    """
    if radius < 0.6 or not 0.0 <= progress <= 1.0:
        return

    ring = radius * (1.0 + 0.55 * ease_out(progress))
    fade = (1.0 - progress) ** 1.7

    window = _window(canvas, cx, cy, ring * 2.1 + 6)
    if window is None:
        return
    (x0, x1, y0, y1), xs, ys = window

    distance = np.hypot(xs, ys)
    angle = np.arctan2(ys, xs)

    # The film thins as it stretches.
    thickness = max(1.2, radius * 0.085) * (1.0 + 2.4 * progress)
    shell = np.exp(-(((distance - ring) / thickness) ** 2))

    # Tear the ring into arcs. At the moment of rupture the ring is still whole;
    # the gaps open from there, so the eye sees it come apart rather than blink
    # out.
    lobes = 7
    seam = 0.5 + 0.5 * np.cos(lobes * angle + hue * 2 * math.pi)
    torn = np.clip(1.0 - progress * 1.9 * (1.0 - seam), 0.0, 1.0)

    colour = iridescent((angle / (2 * math.pi) + hue) % 1.0)
    patch = canvas[y0:y1, x0:x1]
    patch += colour * (shell * torn * fade * 1.15)[..., None]

    # Droplets thrown clear of the rupture.
    droplets = 6
    grid_x = np.arange(x0, x1)[None, :]
    grid_y = np.arange(y0, y1)[:, None]
    for index in range(droplets):
        theta = 2 * math.pi * (index / droplets + hue)
        travel = ring * (1.0 + 0.8 * progress)
        size = max(radius * 0.06, 0.9)
        spark = np.exp(
            -(
                (grid_x - (cx + math.cos(theta) * travel)) ** 2
                + (grid_y - (cy + math.sin(theta) * travel)) ** 2
            )
            / (2 * size**2)
        )
        patch += colour * (spark * fade * 0.45)[..., None]


def spread(
    rng: np.random.Generator, count: int, width: int, height: int
) -> list[tuple[float, float, float]]:
    """Positions and birth times that do not clump.

    Uniform random sampling puts bubbles on top of each other and leaves holes
    elsewhere. This picks each new point from a handful of candidates and keeps
    whichever sits furthest from everything already placed — in space *and* in
    time, so two bubbles may share a spot as long as they are not there at the
    same moment.
    """
    points: list[tuple[float, float, float]] = []

    def separation(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
        dx = (a[0] - b[0]) / width
        dy = (a[1] - b[1]) / width  # by width in both axes, to respect the aspect
        dt = abs(a[2] - b[2])
        dt = min(dt, FRAMES - dt) / FRAMES  # the loop wraps, so time is cyclic
        return math.sqrt(dx * dx + dy * dy + (dt * 1.5) ** 2)

    for _ in range(count):
        best: tuple[float, float, float] | None = None
        best_distance = -1.0
        for _ in range(16):
            candidate = (
                float(rng.uniform(0.05, 0.95)) * width,
                float(rng.uniform(0.16, 0.84)) * height,
                float(rng.uniform(0, FRAMES)),
            )
            if not points:
                best = candidate
                break
            nearest = min(separation(candidate, other) for other in points)
            if nearest > best_distance:
                best_distance, best = nearest, candidate
        if best is not None:
            points.append(best)

    return points


def make_population(rng: np.random.Generator, width: int, height: int) -> list[Bubble]:
    """Pairs scattered across the banner and across the loop, plus the foam."""
    population: list[Bubble] = []

    for x, y, birth in spread(rng, PAIR_COUNT, width, height):
        radius = float(rng.uniform(10, 26)) * SUPERSAMPLE
        lifetime = float(rng.uniform(24, 40))
        angle = float(rng.uniform(0, 2 * math.pi))
        hue = float(rng.uniform(0, 1))
        drift = float(rng.uniform(4, 12)) * SUPERSAMPLE
        travel = float(rng.uniform(2.6, 4.2))

        # The two halves are born at the same point and burst at the same
        # instant, but they travel apart and stay apart. Letting them converge
        # again just puts two bubbles on top of each other.
        for direction, size, tint in ((1.0, 1.0, hue), (-1.0, 0.86, (hue + 0.5) % 1.0)):
            population.append(
                Bubble(
                    birth=birth,
                    lifetime=lifetime,
                    x=x,
                    y=y,
                    radius=radius * size,
                    separation=direction * radius * travel,
                    angle=angle,
                    drift=drift,
                    hue=tint,
                )
            )

    # Quantum foam: the small stuff, blinking in and out below the scale at
    # which anything interesting happens.
    for x, y, birth in spread(rng, FOAM_COUNT, width, height):
        population.append(
            Bubble(
                birth=birth,
                lifetime=float(rng.uniform(10, 20)),
                x=x,
                y=y,
                radius=float(rng.uniform(1.4, 3.6)) * SUPERSAMPLE,
                separation=0.0,
                angle=0.0,
                drift=float(rng.uniform(1, 5)) * SUPERSAMPLE,
                hue=float(rng.uniform(0, 1)),
            )
        )

    return population


def render() -> list[Image.Image]:
    width, height = WIDTH * SUPERSAMPLE, HEIGHT * SUPERSAMPLE
    rng = np.random.default_rng(SEED)

    population = make_population(rng, width, height)
    base = background(width, height)

    frames: list[Image.Image] = []
    for frame in range(FRAMES):
        canvas = base.copy()

        for bubble in population:
            # Ages wrap around the loop, so the animation has no seam.
            age = (frame - bubble.birth) % FRAMES
            if age >= bubble.lifetime:
                continue

            progress = age / bubble.lifetime

            # Travel outward from the birth point and settle there, rather than
            # returning to it. The pair starts already partly apart, so the two
            # halves inflate side by side instead of on top of each other.
            offset = bubble.separation * (
                0.34 + 0.66 * ease_out(min(progress / 0.7, 1.0))
            )
            cx = bubble.x + math.cos(bubble.angle) * offset
            cy = bubble.y + math.sin(bubble.angle) * offset * 0.55
            cy -= bubble.drift * progress

            if progress < INFLATE:
                u = progress / INFLATE
                add_bubble(
                    canvas, cx, cy, bubble.radius * ease_out(u), math.sqrt(u), bubble.hue
                )
            elif progress < 1.0 - BURST:
                # Held at full size, with the faint wobble of a real film.
                wobble = 1.0 + 0.02 * math.sin(
                    2 * math.pi * (progress * 1.5 + bubble.hue)
                )
                add_bubble(canvas, cx, cy, bubble.radius * wobble, 1.0, bubble.hue)
            else:
                add_burst(
                    canvas,
                    cx,
                    cy,
                    bubble.radius,
                    (progress - (1.0 - BURST)) / BURST,
                    bubble.hue,
                )

        pixels = np.clip(canvas, 0.0, 1.0)
        image = Image.fromarray((pixels * 255).astype(np.uint8), mode="RGB")
        frames.append(image.resize((WIDTH, HEIGHT), Image.LANCZOS))

    return frames


def quantise(frames: list[Image.Image], colours: int, dither: bool) -> list[Image.Image]:
    """Give every frame the same palette.

    A per-frame palette makes the background shimmer as colours are reassigned;
    one shared palette keeps it still and compresses far better.
    """
    montage = Image.new("RGB", (frames[0].width, frames[0].height * len(frames)))
    for index, frame in enumerate(frames):
        montage.paste(frame, (0, index * frames[0].height))

    palette = montage.quantize(colors=colours, method=Image.MEDIANCUT)
    method = Image.FLOYDSTEINBERG if dither else Image.NONE
    return [frame.quantize(palette=palette, dither=method) for frame in frames]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "docs/assets/banner.gif",
    )
    parser.add_argument("--colours", type=int, default=192)
    parser.add_argument(
        "--dither",
        action="store_true",
        help="dither the palette; smoother gradients, roughly twice the file size",
    )
    args = parser.parse_args()

    frames = quantise(render(), args.colours, args.dither)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(
        args.output,
        save_all=True,
        append_images=frames[1:],
        duration=FRAME_MS,
        loop=0,
        optimize=True,
        disposal=2,
    )

    size = args.output.stat().st_size
    print(f"wrote {args.output} — {len(frames)} frames, {size / 1024:.0f} KiB")


if __name__ == "__main__":
    main()
