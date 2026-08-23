#!/usr/bin/env python3
"""Photographs the Android application, on whatever device adb is talking to.

The desktop window has `film.mjs` and `film-window.sh`. This is the same idea
for the phone, and it exists for the same reason: a screen nobody has looked at
has problems no test finds, and the Android application had never been looked
at by anybody.

Nothing here asserts. That is the point — you have to look at the frames. Each
one is named for what you are meant to check in it.

It drives the real interface: taps land on controls found by their resource id
in a UI dump, not on coordinates guessed from a screenshot. The first version
of this did guess, and pressed empty space next to the Create button on a
device whose resolution was not the one the numbers were written for.

    adb wait-for-device && apps/android/tests/photograph.py [directory]
"""

from __future__ import annotations

import re
import subprocess
import sys
import time
from pathlib import Path

PACKAGE = "io.github.jgalego.ephemeral"


def adb(*arguments: str, binary: bool = False) -> bytes | str:
    """Runs one adb command, and fails loudly rather than silently."""
    finished = subprocess.run(
        ["adb", *arguments],
        capture_output=True,
        check=True,
        timeout=120,
    )
    return finished.stdout if binary else finished.stdout.decode("utf-8", "replace")


def screen() -> str:
    """The current interface, as the accessibility tree describes it."""
    adb("shell", "uiautomator", "dump", "/sdcard/ui.xml")
    return str(adb("shell", "cat", "/sdcard/ui.xml"))


def centre_of(node: str) -> tuple[int, int] | None:
    """Where to tap for a node, from the bounds in its dump entry."""
    found = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', node)
    if not found:
        return None

    left, top, right, bottom = (int(value) for value in found.groups())
    return (left + right) // 2, (top + bottom) // 2


def find(what: str) -> tuple[int, int] | None:
    """Locates a control by resource id, or by the text it shows.

    Resource id first, because it is what the application named the thing;
    text is the fallback for what has no id, and is what a person would be
    looking for anyway.
    """
    dump = screen()
    for node in dump.split("<node"):
        if f'resource-id="{PACKAGE}:id/{what}"' in node or f'text="{what}"' in node:
            return centre_of(node)
    return None


def tap(what: str, *, required: bool = True) -> bool:
    """Presses a control. Says which one it could not find, and shows what was
    on screen instead — a run that fails here fails for a reason somebody can
    read."""
    where = find(what)
    if where is None:
        if not required:
            return False
        print(f"Could not find {what!r} on screen. What is there:", file=sys.stderr)
        for node in screen().split("<node"):
            identity = re.search(rf'resource-id="{PACKAGE}:id/([^"]+)"', node)
            if identity:
                print(f"  {identity.group(1)}", file=sys.stderr)
        raise SystemExit(1)

    adb("shell", "input", "tap", str(where[0]), str(where[1]))
    return True


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "screens")
    out.mkdir(parents=True, exist_ok=True)

    taken = 0

    def shot(what: str, settle: float = 1.5) -> None:
        nonlocal taken
        time.sleep(settle)
        taken += 1
        name = out / f"{taken:02d}-{what}.png"
        name.write_bytes(bytes(adb("exec-out", "screencap", "-p", binary=True)))
        print(f"  {name.name}")

    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "am", "start", "-n", f"{PACKAGE}/.MainActivity")
    time.sleep(4)

    # Nothing recorded yet. The sentence about what a phone will not do has to
    # be on this screen, not in an about box.
    shot("nothing-yet", 2.0)

    # Asking for one. No code is written and nothing runs — which is exactly
    # why a phone can do this part.
    tap("intent")
    adb("shell", "input", "text", "compare%stwo%sCSV%sfiles")
    shot("asking-for-one")

    adb("shell", "input", "keyevent", "111")  # dismiss the keyboard
    tap("create")

    # One application, recorded. The card has to show what it is, where it is
    # in its life, and what it holds — as three things, because they were one
    # grey line and that made a dangerous application look like a harmless one.
    shot("one-application-recorded", 3.0)

    # Its page. Nothing has been generated, so it should be asking for nothing
    # and saying so, and the Generate button should be the only filled control.
    if tap("purpose", required=False):
        shot("its-page", 3.0)

    print()
    print(f"Photographed into {out}")
    print("Now look at them. That is the whole point.")


if __name__ == "__main__":
    main()
