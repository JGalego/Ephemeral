#!/usr/bin/env bash
#
# Films the real desktop window, on a machine with no display.
#
# `tests/film.mjs` films the frontend in Chromium, which catches everything
# about what the page says and nothing about the window around it. Tauri renders
# through WebKit on Linux and macOS, not Chromium, so the two are not the same
# picture — a layout that holds in one can break in the other, and the whole
# point of filming is to look at what somebody would actually see.
#
# This runs the real binary against a virtual X server and records that server's
# framebuffer. Nothing appears on anybody's screen; the output is a file.
#
#   apps/desktop/tests/film-window.sh [seconds]
#
# Needs Xvfb and ffmpeg. Both are absent on plenty of machines, so this is not
# part of `scripts/check` and is not part of CI — it is a thing you run when you
# want to look at the window.

set -euo pipefail

DESKTOP="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$DESKTOP/recordings"
SECONDS_TO_FILM="${1:-20}"
DISPLAY_NUMBER="${EPHEMERAL_FILM_DISPLAY:-:99}"
SIZE="1280x900"

for tool in Xvfb ffmpeg; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "This needs $tool, which is not installed."
    echo "Debian/Ubuntu: sudo apt-get install xvfb ffmpeg"
    exit 1
  fi
done

BINARY="$DESKTOP/src-tauri/target/debug/ephemeral-desktop"
if [ ! -x "$BINARY" ]; then
  echo "No window binary at $BINARY."
  echo "Build it first: (cd $DESKTOP/src-tauri && cargo build)"
  exit 1
fi

mkdir -p "$OUT"

# Everything started here is killed on the way out, including on failure. A
# stranded Xvfb holding a display number is the kind of thing that makes the
# next run fail for an unrelated reason.
PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT

echo "==> Virtual display on $DISPLAY_NUMBER ($SIZE)"
Xvfb "$DISPLAY_NUMBER" -screen 0 "${SIZE}x24" -nolisten tcp &
PIDS+=($!)

# Xvfb takes a moment to accept connections, and a window started before it is
# ready fails with an error about the display rather than doing anything useful.
for _ in $(seq 1 40); do
  if DISPLAY="$DISPLAY_NUMBER" xdpyinfo >/dev/null 2>&1; then break; fi
  sleep 0.25
done

# Filmed against a scratch home so this never draws somebody's real
# applications, and never writes to their state to produce a demo.
FILM_HOME="${EPHEMERAL_FILM_HOME:-$(mktemp -d)}"
echo "==> Ephemeral home: $FILM_HOME"

echo "==> Starting the window"
DISPLAY="$DISPLAY_NUMBER" EPHEMERAL_HOME="$FILM_HOME" \
  WEBKIT_DISABLE_COMPOSITING_MODE=1 "$BINARY" &
PIDS+=($!)
sleep 3

echo "==> Recording ${SECONDS_TO_FILM}s"
ffmpeg -loglevel error -y \
  -f x11grab -video_size "$SIZE" -framerate 15 -i "$DISPLAY_NUMBER" \
  -t "$SECONDS_TO_FILM" -pix_fmt yuv420p "$OUT/window.mp4"

# A still, because a person reviewing this reads frames rather than scrubbing a
# video, and a frame from the middle is past any startup flicker.
ffmpeg -loglevel error -y -i "$OUT/window.mp4" \
  -vf "select=eq(n\,$((SECONDS_TO_FILM * 15 / 2)))" -vframes 1 "$OUT/window.png"

echo
echo "Filmed into $OUT"
echo "  window.mp4"
echo "  window.png"
echo
echo "Now look at them. That is the whole point."
