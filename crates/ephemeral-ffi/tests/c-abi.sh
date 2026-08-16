#!/usr/bin/env bash
#
# Compiles a C host against the real header and the real static library, then
# runs it.
#
# This is the only check that can catch a header which has drifted from the
# library it describes: the Rust tests link the crate as an rlib and can only
# string-match `ephemeral.h`, so a wrong signature there is invisible to them
# and a link error here. Swift and Kotlin both reach Ephemeral through C, so
# what this proves is the boundary they will use.
#
#   crates/ephemeral-ffi/tests/c-abi.sh

set -euo pipefail

CRATE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$CRATE/../.." && pwd)"
CC="${CC:-cc}"

if ! command -v "$CC" >/dev/null 2>&1; then
  echo "(skipping the C ABI check: no C compiler)"
  exit 0
fi

cargo build -p ephemeral-ffi --manifest-path "$ROOT/Cargo.toml"

LIBRARY="$ROOT/target/debug/libephemeral_ffi.a"
if [ ! -f "$LIBRARY" ]; then
  echo "no static library at $LIBRARY" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# -Wall -Wextra -Werror so a header that compiles only by luck fails here.
"$CC" -std=c11 -Wall -Wextra -Werror \
  -I "$CRATE/include" \
  -o "$WORK/host" \
  "$CRATE/tests/host.c" \
  "$LIBRARY" \
  -lpthread -ldl -lm

"$WORK/host" "$WORK/home"
