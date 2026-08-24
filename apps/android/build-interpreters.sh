#!/usr/bin/env bash
#
# Builds the interpreters this app ships and puts them in its assets.
#
# A phone cannot compile anything. So an application a model wrote a moment ago
# runs on the device it was written on only if it is a script and something
# already there runs scripts — which is a WebAssembly module, shipped inside the
# APK and signed with the rest of it (ADR-0022).
#
# Separate from build-native.sh because this needs no NDK and no cross-linker:
# a `.wasm` file is the same file on every phone. All it wants is the
# `wasm32-wasip1` target.
#
#     ./build-interpreters.sh
#
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-wasip1; then
    echo "The wasm32-wasip1 target is not installed." >&2
    echo "Install it with: rustup target add wasm32-wasip1" >&2
    exit 1
fi

assets="$here/app/src/main/assets/interpreters"
mkdir -p "$assets"

# name-on-the-device:where-it-is-built. The name is what
# `ephemeral-runtime`'s interpreter table looks for, and the two must agree or
# an application that is a script fails to start with nothing to point at.
interpreters=(
    "javascript.wasm:interpreters/javascript:ephemeral-javascript.wasm"
)

for entry in "${interpreters[@]}"; do
    name="${entry%%:*}"
    rest="${entry#*:}"
    crate="${rest%%:*}"
    built="${rest#*:}"

    echo "  $name"
    (cd "$root/$crate" && cargo build --release --locked --target wasm32-wasip1)
    cp "$root/$crate/target/wasm32-wasip1/release/$built" "$assets/$name"
done

echo
echo "Interpreters in $assets:"
ls -la "$assets"
