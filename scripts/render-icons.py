#!/usr/bin/env python3
"""Render the application icon set at apps/desktop/src-tauri/icons/.

The same joke as the banner: a bubble, which is a thing that exists for a while
and then does not. Here it sits on a ground rather than floating on nothing,
because an icon is not a picture on a page — it is a small object on somebody
else's desktop, and it has to be legible against a wallpaper nobody chose for
it. The first version of this icon was a pale blue ring on transparency, which
looked correct in a file browser and disappeared entirely on a light desktop.

Emits every format the bundlers need, which is the reason this script exists at
all: `.ico` for the Windows installers, `.icns` for the macOS disk image, and
PNGs for Linux and for the window itself. Missing either of the first two does
not degrade a release — it fails the build.

Run it with:

    python3 scripts/render-icons.py

Needs Pillow and numpy, which are not project dependencies — the output is
committed, so this only runs when the icon changes. Rendering is deterministic,
so re-running without changing anything reproduces the same files rather than
creating a spurious diff.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np
from PIL import Image

# Rendered large and reduced, because every edge here is a curve and a curve
# drawn at final size is a curve with stairs on it.
SUPERSAMPLE = 4
MASTER = 1024

# The sizes each platform actually asks for. Windows wants them inside one
# `.ico`; macOS wants them inside one `.icns`; Linux wants files.
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
ICNS_SIZES = (16, 32, 64, 128, 256, 512, 1024)
PNG_FILES = {
    "32x32.png": 32,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 512,
}

# Slate blue, dark enough that a white-ish bubble reads on it and not so dark it
# looks like a hole punched in the dock.
GROUND_TOP = np.array([0.212, 0.278, 0.404])
GROUND_BOTTOM = np.array([0.114, 0.153, 0.239])

# The bubble itself: a rim that catches light, a wall you can see through, and
# the faint warm/cool split real soap films have.
RIM_COOL = np.array([0.694, 0.831, 1.000])
RIM_WARM = np.array([0.988, 0.914, 0.976])
INTERIOR = np.array([0.541, 0.702, 0.933])


def smoothstep(edge0: float, edge1: float, values: np.ndarray) -> np.ndarray:
    """A soft threshold, which is what every edge in this image is."""
    t = np.clip((values - edge0) / (edge1 - edge0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def rounded_square(size: int, radius: float, inset: float) -> np.ndarray:
    """Coverage for a rounded square, as a float mask.

    macOS draws icon art exactly as given, so the rounding has to be in the
    picture rather than applied by the platform. Windows and Linux draw the same
    art on a square, where a rounded one simply looks deliberate.
    """
    axis = np.arange(size, dtype=np.float64) + 0.5
    x, y = np.meshgrid(axis, axis, indexing="xy")

    half = size / 2.0
    # Distance to a rounded square, by way of the usual signed-distance trick:
    # measure to the inner rectangle, then subtract the corner radius.
    dx = np.abs(x - half) - (half - inset - radius)
    dy = np.abs(y - half) - (half - inset - radius)
    outside = np.hypot(np.maximum(dx, 0.0), np.maximum(dy, 0.0))
    inside = np.minimum(np.maximum(dx, dy), 0.0)
    distance = outside + inside - radius

    return 1.0 - smoothstep(-1.0, 1.0, distance)


def bubble(size: int, centre: tuple[float, float], outer: float, thickness: float):
    """One bubble: its colour and its opacity, both as arrays.

    A bubble is mostly not there. The wall is what you see, and it is brightest
    where you look through the most of it — at the rim, edge on. That is why
    this is a ring rather than a disc, and why the interior is a wash rather
    than a fill.
    """
    axis = np.arange(size, dtype=np.float64) + 0.5
    x, y = np.meshgrid(axis, axis, indexing="xy")

    cx, cy = centre
    radius = np.hypot(x - cx, y - cy)
    inner = outer - thickness

    # The wall, thickest at the rim and falling off inwards.
    wall = smoothstep(inner - thickness * 0.9, inner + thickness * 0.15, radius)
    wall *= 1.0 - smoothstep(outer - thickness * 0.15, outer + 1.5, radius)

    # What you see through the middle: the far wall, much fainter.
    through = (1.0 - smoothstep(inner - thickness, inner, radius)) * 0.16

    # Soap-film iridescence, faked as a cool-to-warm sweep around the ring so
    # the highlight is not the same colour all the way round.
    angle = np.arctan2(y - cy, x - cx)
    sweep = (np.cos(angle - 0.9) + 1.0) / 2.0
    rim = RIM_COOL[None, None, :] * (1.0 - sweep[..., None]) + RIM_WARM[None, None, :] * sweep[..., None]

    alpha = np.clip(wall + through, 0.0, 1.0)
    # Where the wall is faint the colour tends to the interior wash; where it is
    # strong it tends to the rim.
    strength = np.clip(wall / (wall + through + 1e-6), 0.0, 1.0)[..., None]
    colour = INTERIOR[None, None, :] * (1.0 - strength) + rim * strength

    # The specular: one small highlight up and to the left, the thing that makes
    # a ring read as a sphere rather than a washer.
    spec_x, spec_y = cx - outer * 0.42, cy - outer * 0.46
    spec = 1.0 - smoothstep(0.0, outer * 0.30, np.hypot(x - spec_x, y - spec_y))
    spec = spec**2 * 0.85

    alpha = np.clip(alpha + spec, 0.0, 1.0)
    colour = np.clip(colour + spec[..., None] * 0.9, 0.0, 1.0)

    return colour, alpha


def render(size: int) -> Image.Image:
    """The whole icon, at one size."""
    work = size * SUPERSAMPLE

    axis = np.arange(work, dtype=np.float64) + 0.5
    _, y = np.meshgrid(axis, axis, indexing="xy")

    # The ground, with a gentle top-to-bottom gradient so it is not a flat slab.
    fall = (y / work)[..., None]
    image = GROUND_TOP[None, None, :] * (1.0 - fall) + GROUND_BOTTOM[None, None, :] * fall

    # The pair from the banner: one bubble that is the subject, and a small one
    # that has just nucleated beside it. Two is the whole idea — they come out
    # of nothing together.
    big_colour, big_alpha = bubble(
        work,
        centre=(work * 0.455, work * 0.480),
        outer=work * 0.300,
        thickness=work * 0.072,
    )
    image = image * (1.0 - big_alpha[..., None]) + big_colour * big_alpha[..., None]

    small_colour, small_alpha = bubble(
        work,
        centre=(work * 0.735, work * 0.725),
        outer=work * 0.108,
        thickness=work * 0.030,
    )
    image = image * (1.0 - small_alpha[..., None]) + small_colour * small_alpha[..., None]

    shape = rounded_square(work, radius=work * 0.200, inset=work * 0.035)

    rgba = np.concatenate([image, shape[..., None]], axis=-1)
    rendered = Image.fromarray((np.clip(rgba, 0.0, 1.0) * 255).round().astype(np.uint8), "RGBA")

    return rendered.resize((size, size), Image.LANCZOS)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "apps/desktop/src-tauri/icons",
        help="where to write the icon set",
    )
    arguments = parser.parse_args()

    out: Path = arguments.out
    out.mkdir(parents=True, exist_ok=True)

    master = render(MASTER)

    for name, size in PNG_FILES.items():
        master.resize((size, size), Image.LANCZOS).save(out / name)
        print(f"  {name}")

    # One file carrying every size Windows might ask for. Letting Windows scale
    # a single large bitmap down to 16px produces mush.
    master.save(
        out / "icon.ico",
        format="ICO",
        sizes=[(size, size) for size in ICO_SIZES],
    )
    print("  icon.ico")

    master.save(
        out / "icon.icns",
        format="ICNS",
        sizes=[(size, size) for size in ICNS_SIZES],
    )
    print("  icon.icns")

    print(f"\nWrote the icon set to {out}")


if __name__ == "__main__":
    main()
