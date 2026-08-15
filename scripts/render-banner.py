#!/usr/bin/env python3
"""Render the animated banner at docs/assets/banner.gif.

Bubbles nucleate in pairs out of nothing, drift apart, come back together and
annihilate — the way virtual particles do in a vacuum. It is a joke about
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
FRAMES = 40
FRAME_MS = 70
SUPERSAMPLE = 2  # rendered large and downscaled, for edges that are not jagged
SEED = 0xB0BB1E

PAIR_COUNT = 32
FOAM_COUNT = 60

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
    """A particle–antiparticle pair: born together, gone together.

    A pair with `separation` of zero is a single bubble — the quantum foam,
    too small and too brief for the parting to be visible.
    """

    birth: float       # frame it nucleates on
    lifetime: float    # how many frames it lasts
    x: float           # where, in supersampled pixels
    y: float
    radius: float      # peak radius
    separation: float  # how far the two halves drift apart before rejoining
    angle: float       # the axis they separate along
    drift: float       # how far the pair rises over its life
    hue: float         # where it sits in the iridescent cycle


def iridescent(t: np.ndarray) -> np.ndarray:
    """Map t in [0, 1) onto the soap-film palette."""
    stops = np.array([s[0] for s in IRIDESCENCE])
    channels = [np.interp(t, stops, [s[1][c] for s in IRIDESCENCE]) for c in range(3)]
    return np.stack(channels, axis=-1) / 255.0


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


def add_bubble(
    canvas: np.ndarray,
    cx: float,
    cy: float,
    radius: float,
    intensity: float,
    hue: float,
) -> None:
    """Composite one bubble additively, so overlaps glow like real films do."""
    if radius < 0.6 or intensity <= 0.002:
        return

    height, width, _ = canvas.shape
    pad = radius * 1.5 + 4
    x0, x1 = max(0, int(cx - pad)), min(width, int(cx + pad) + 1)
    y0, y1 = max(0, int(cy - pad)), min(height, int(cy + pad) + 1)
    if x0 >= x1 or y0 >= y1:
        return

    ys = np.arange(y0, y1)[:, None] - cy
    xs = np.arange(x0, x1)[None, :] - cx
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

    # Iridescence varies around the circumference and across the film.
    angle = np.arctan2(ys, xs) / (2 * math.pi)
    shift = (angle + hue + 0.22 * (distance / max(radius, 1e-6))) % 1.0
    colour = iridescent(shift)

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


def add_annihilation(
    canvas: np.ndarray, cx: float, cy: float, radius: float, progress: float
) -> None:
    """The flash left behind when a pair cancels out."""
    if not 0.0 <= progress <= 1.0:
        return

    height, width, _ = canvas.shape
    ring = radius * (0.5 + 1.7 * progress)
    fade = (1.0 - progress) ** 2 * 0.5

    pad = ring * 1.4 + 4
    x0, x1 = max(0, int(cx - pad)), min(width, int(cx + pad) + 1)
    y0, y1 = max(0, int(cy - pad)), min(height, int(cy + pad) + 1)
    if x0 >= x1 or y0 >= y1:
        return

    ys = np.arange(y0, y1)[:, None] - cy
    xs = np.arange(x0, x1)[None, :] - cx
    distance = np.hypot(xs, ys)

    thickness = max(1.6, ring * 0.16)
    shell = np.exp(-(((distance - ring) / thickness) ** 2)) * fade
    colour = np.array([0.95, 0.85, 1.0])

    canvas[y0:y1, x0:x1] += colour[None, None, :] * shell[..., None]


def stratified(rng: np.random.Generator, count: int) -> np.ndarray:
    """`count` jittered samples spread evenly over [0, 1).

    Uniform random sampling clumps: it leaves visible holes in the field and
    moments where the whole banner is empty. Stratifying keeps the density even
    in both space and time while still looking unplanned.
    """
    edges = np.arange(count) / count
    return (edges + rng.uniform(0, 1 / count, count)) % 1.0


def make_population(rng: np.random.Generator, width: int, height: int) -> list[Bubble]:
    """Pairs scattered across the banner and across the loop, plus the foam."""
    population: list[Bubble] = []

    pair_x = stratified(rng, PAIR_COUNT)
    pair_birth = stratified(rng, PAIR_COUNT) * FRAMES
    rng.shuffle(pair_birth)

    for index in range(PAIR_COUNT):
        radius = float(rng.uniform(8, 30)) * SUPERSAMPLE
        population.append(
            Bubble(
                birth=float(pair_birth[index]),
                lifetime=float(rng.uniform(11, 24)),
                x=float(0.02 + 0.96 * pair_x[index]) * width,
                y=float(rng.uniform(0.08, 0.94)) * height,
                radius=radius,
                separation=radius * float(rng.uniform(2.0, 3.4)),
                angle=float(rng.uniform(0, 2 * math.pi)),
                drift=float(rng.uniform(3, 14)) * SUPERSAMPLE,
                hue=float(rng.uniform(0, 1)),
            )
        )

    # Quantum foam: the small stuff, blinking in and out below the scale at
    # which anything interesting happens.
    foam_x = stratified(rng, FOAM_COUNT)
    foam_birth = stratified(rng, FOAM_COUNT) * FRAMES
    rng.shuffle(foam_birth)

    for index in range(FOAM_COUNT):
        population.append(
            Bubble(
                birth=float(foam_birth[index]),
                lifetime=float(rng.uniform(4, 11)),
                x=float(foam_x[index]) * width,
                y=float(rng.uniform(0, 1)) * height,
                radius=float(rng.uniform(1.2, 3.4)) * SUPERSAMPLE,
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
                # Just after annihilation, leave the flash behind.
                since = age - bubble.lifetime
                if since < 4:
                    add_annihilation(
                        canvas, bubble.x, bubble.y - bubble.drift, bubble.radius, since / 4
                    )
                continue

            progress = age / bubble.lifetime
            envelope = math.sin(math.pi * progress)
            radius = bubble.radius * envelope
            intensity = envelope**0.65

            cy = bubble.y - bubble.drift * progress
            if bubble.separation == 0.0:
                add_bubble(canvas, bubble.x, cy, radius, intensity * 0.8, bubble.hue)
                continue

            # The two halves part, then come back together to cancel.
            offset = bubble.separation * 0.5 * math.sin(math.pi * progress)
            dx = math.cos(bubble.angle) * offset
            dy = math.sin(bubble.angle) * offset * 0.55
            add_bubble(canvas, bubble.x + dx, cy + dy, radius, intensity, bubble.hue)
            add_bubble(
                canvas,
                bubble.x - dx,
                cy - dy,
                radius * 0.82,
                intensity,
                (bubble.hue + 0.5) % 1.0,
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
