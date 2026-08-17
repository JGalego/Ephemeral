#!/usr/bin/env bash
#
# Loads the JNI bridge into a real JVM and drives it.
#
# The Android application cannot be run here — that needs a device or an
# emulator — but the bridge can, because JNI is the same on a desktop JVM. This
# catches the failures that matter and that nothing else would see: a symbol
# name that no longer matches its class, a method signature the bridge looks up
# and cannot find, a callback that leaves an exception pending.
#
# Skips itself, loudly, when there is no JDK.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"

if ! command -v javac >/dev/null 2>&1 || ! command -v java >/dev/null 2>&1; then
    echo "(skipping the JNI bridge: no JDK on this machine)"
    exit 0
fi

cargo build --quiet -p ephemeral-android --manifest-path "$root/Cargo.toml"

# Cargo names it .so on Linux and .dylib on macOS, and `System.loadLibrary`
# expects exactly the platform's own spelling — so let the directory be the
# search path rather than naming the file.
libraries="$root/target/debug"
if [ ! -e "$libraries/libephemeral_android.so" ] && \
   [ ! -e "$libraries/libephemeral_android.dylib" ]; then
    echo "No ephemeral_android library was built in $libraries." >&2
    exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

javac -d "$work/classes" "$here"/java/io/github/jgalego/ephemeral/*.java

java -Djava.library.path="$libraries" \
     -cp "$work/classes" \
     io.github.jgalego.ephemeral.Check
