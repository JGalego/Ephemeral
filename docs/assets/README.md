# Assets

| File | What it is | Where it is used |
|------|------------|------------------|
| `logo.svg` | The project mark: one bubble's life in three stages | Application icon, favicon, documentation, anywhere a square mark is needed |
| `banner.gif` | The animated header | Top of the README |

## The mark

`logo.svg` is hand-written SVG rather than an exported bitmap, so it scales to
any size, stays legible on light and dark backgrounds, and adds no binary to the
repository.

It reads as one bubble's life rather than as three bubbles:

1. **Whole** — the film holds and you see straight through it.
2. **Mid-pop** — the film is gone and only the rim is left, already coming
   apart. The dashes are uneven on purpose; an evenly dotted circle reads as a
   UI border rather than as something failing.
3. **Popped** — only the traces of where it went, all radiating from one centre
   with a gap in the middle. The shared centre and the void are what let the eye
   reconstruct the bubble that was there.

That last stage is the product thesis, not decoration.

## The banner

`banner.gif` is generated, not drawn. Bubbles nucleate in pairs out of nothing,
drift apart, hold for a moment and then burst — the way virtual particles do in
a vacuum, which is a physics joke that happens to describe what Ephemeral does
to software.

It carries no text. The README supplies the words.

Regenerate it with:

```bash
python3 scripts/render-banner.py
```

The script needs Pillow and numpy, which are deliberately **not** project
dependencies — the output is committed, so nobody needs them to build or run
Ephemeral. Rendering is deterministic (fixed seed), so re-running it without
changing any parameters reproduces the same file rather than creating a
spurious diff.
