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

# Swift reads `module.modulemap` to turn the header into something it can
# `import`, and a broken one fails inside Xcode on somebody else's machine
# rather than here. Clang can build the module on any platform, so it is
# checked wherever clang exists rather than only where Swift does.
if command -v clang >/dev/null 2>&1; then
  printf '#include "ephemeral.h"\nint main(void){ ephemeral_close(0); return 0; }\n' \
    > "$WORK/module.c"
  clang -fmodules -fimplicit-module-maps \
    -fmodules-cache-path="$WORK/modules" -fsyntax-only \
    -I "$CRATE/include" "$WORK/module.c"
  echo "The module map Swift imports builds."
else
  echo "(skipping the module map check: no clang)"
fi

# The mobile guide is the first thing somebody embedding this reads, and its
# Swift is the code they will paste. Sample code that calls a function which no
# longer exists is worse than no sample: it is confidently wrong. Every call and
# every constant it uses has to be in the header.
GUIDE="$ROOT/docs/mobile.md"
if [ -f "$GUIDE" ]; then
  unknown=""
  for name in $(grep -oE '\bephemeral_[a-z_]+\(|\bEPHEMERAL_[A-Z_]+\b' "$GUIDE" \
                  | tr -d '(' | sort -u); do
    grep -q "\b$name\b" "$CRATE/include/ephemeral.h" || unknown="$unknown $name"
  done
  if [ -n "$unknown" ]; then
    echo "docs/mobile.md uses names the header does not have:$unknown" >&2
    exit 1
  fi
  echo "Everything the mobile guide calls is in the header."
fi
