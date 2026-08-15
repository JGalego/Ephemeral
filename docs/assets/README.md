# Assets

| File | What it is | Where it is used |
|------|------------|------------------|
| `logo.svg` | The project mark: three bubbles, one of them mid-pop | Application icon, favicon, documentation, anywhere a square mark is needed |
| `banner.gif` | The animated header | Top of the README |

## The mark

`logo.svg` is hand-written SVG rather than an exported bitmap, so it scales to
any size, stays legible on light and dark backgrounds, and adds no binary to the
repository. Three bubbles: one whole, one small, and one already breaking up.
The last one is the product thesis rather than decoration.

## The banner

`banner.gif` is generated, not drawn. Bubbles nucleate in pairs out of nothing,
drift apart, come back together and annihilate — the way virtual particles do in
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
