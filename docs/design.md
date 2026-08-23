# How Ephemeral looks, and why

Ephemeral asks people to make decisions about what software on their machine is
allowed to do. Everything below follows from that one fact. This is not a style
guide bolted onto a finished product; it is the part of the product that decides
whether somebody reads the question they are answering.

## One palette, generated

The colours live in [`crates/ephemeral-design`](../crates/ephemeral-design), in
Rust, once. Every platform's file is generated from them:

```bash
cargo run -p ephemeral-design
```

| Generated | Used by |
|---|---|
| `apps/desktop/ui/tokens.css` | the desktop window |
| `apps/android/app/src/main/res/values/colors.xml` | the Android application |

The generated files are **checked in** — the window has no build step and
should not acquire one to draw a list — and a test asserts they still match, so
editing one by hand fails rather than drifts. Neither client may name a colour
of its own: a hex in a stylesheet is a hex the contrast checks never see, and
the desktop's own test suite fails on one.

This is the same argument as [`ephemeral-api`](../crates/ephemeral-api), one
layer out. The terminal and the window must not describe a critical permission
in two different reds any more than they may describe it in two different
sentences.

## Contrast is a test, not an intention

Risk is carried by colour in all three clients. A risk level somebody cannot
read against the ground it sits on is a permission prompt that quietly does not
work, and the person it fails is the one least able to say so.

So every pairing either client actually draws is listed in `PAIRINGS` and
checked against [WCAG 2.1][wcag] in both schemes on every commit: 4.5:1 for
text, 3:1 for the outline of a control. The first version of this palette failed
on the outline of a button in *both* schemes, which is exactly the kind of thing
that gets waved through when contrast is a matter of opinion.

Run `cargo test -p ephemeral-design -- --nocapture` to see the whole table with
its margins, which is what somebody adjusting a colour needs.

Colour is never the only carrier. Every risk-coloured thing also says its level
in words, in every client.

[wcag]: https://www.w3.org/TR/WCAG21/#contrast-minimum

## Dark, and not by media query

Ephemeral is dark. A browser reports `prefers-color-scheme: light` for a machine
that has expressed no preference at all, so following the preference means being
white for everybody who never set one — which is not "respecting the system", it
is defaulting to white with extra steps.

The light palette is a full palette rather than an inversion: the four risk
levels have to keep their order of alarm against a pale ground, which needs
different colours, not lighter ones. It is applied by
`:root[data-theme="light"]`, set by a control in the window's header and
remembered in `localStorage`. The Android application is dark only, because
`minSdk` there is 26 — which predates the system dark-mode setting the `-night`
qualifier follows, so a light scheme on a phone would be chosen by a switch a
good share of supported devices do not have.

## What the colours mean

| Token | For |
|---|---|
| `ground`, `ground-soft` | the page |
| `surface`, `surface-high` | a card; something floating over one |
| `edge`, `edge-strong` | a hairline; the outline of something you can operate |
| `ink`, `ink-quiet`, `ink-faint` | what you read, in three weights of importance |
| `accent` | Ephemeral itself: the primary action, and every focus ring |
| `low`, `medium`, `high`, `critical` | the four risk levels, and nothing else |
| `*-soft` | a ground tinted towards one of the above |

The risk colours are reserved. Green is not "success" in general — it is `low`,
and it also happens to mark a version that worked. Nothing that is not a risk
may borrow one of those four.

## Rules a film paid for

Each of these exists because somebody looked at a recording of the window and
saw it. They are enforced in the stylesheet and in
`apps/desktop/tests/render.test.mjs`, and they hold on the phone too.

- **A granted permission and an unanswered one must not look the same.** Held is
  marked by the tick, the tinted ground, and the control offering to take it
  back.
- **Granting something never recolours it towards safety.** An early version
  painted a green stripe over an application that had just been allowed to reach
  the entire internet, and faded it for good measure: the most dangerous grant
  on the page became the calmest thing on screen. The held tint is the ordinary
  surface, precisely so it cannot be read as reassurance.
- **Risk colours what a permission would let an application do** — never the
  sentence saying it can be undone. "You can take this back at any time" in
  crimson reads as a warning, which is the opposite of what it says.
- **Allow and Refuse are drawn identically.** A window that made the permissive
  answer the pretty one would be collecting consent rather than asking for it.
  The only filled buttons in either client are the ones that grant nothing:
  asking for an application, and generating one.
- **A refusal is pinned to the viewport.** One rendered below the fold is the
  same as no refusal — which is what a film showed, several hundred pixels past
  where the person was looking.
- **An application waiting on a decision is the only thing that shouts.**
- **Anything an application cannot do right now is absent, not disabled.** A row
  of greyed-out buttons is a puzzle; the state is already on the page in words.
  Which actions exist comes from the lifecycle, not from a client's own reading
  of a few booleans.

## Motion

Almost none. Hover and focus transitions at 130ms, and one thing that breathes:
the dot on a state the engine is working through, because generation takes
minutes and a perfectly still window reads as nothing happening. Everything
stops entirely under `prefers-reduced-motion`.

Somebody reading a permission prompt is making a decision. Nothing may move
while they are doing it.

## Looking at it

Neither client's appearance is asserted by a test — you have to look. Both
produce files rather than needing a screen:

```bash
cd apps/desktop
node tests/film.mjs        # the frontend in Chromium, one still per step
tests/film-window.sh 20    # the real Tauri window, under Xvfb
```

See [development.md](development.md#looking-at-it-without-a-display) for what
those found, and add to that list when you find the next one.
