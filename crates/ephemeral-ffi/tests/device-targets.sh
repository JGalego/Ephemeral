#!/usr/bin/env bash
#
# Builds the library a phone actually links, for the architectures phones
# actually are, and checks that every function the header promises is really in
# the archive.
#
# The C ABI test compiles a host against the x86-64 build on this machine. That
# proves the contract holds; it does not prove the crate survives the trip to a
# device. A dependency that only compiles for the host, a `cfg` that quietly
# excludes a platform, or an export optimised away for one target would all pass
# every other check here and fail on someone's phone.
#
# No Xcode and no Android NDK are needed: a static library is an archive of
# object files, so it is assembled rather than linked, and the platform linker
# never runs. That is the whole reason this can be a CI check rather than a
# thing somebody does by hand on a Mac.
#
#   crates/ephemeral-ffi/tests/device-targets.sh

set -euo pipefail

CRATE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$CRATE/../.." && pwd)"
cd "$ROOT"

# The device architectures, plus the simulator an iOS developer builds against
# every day. Android's emulator is x86-64, which is already covered by the C ABI
# test, so it is not repeated here.
TARGETS=(
  aarch64-apple-ios      # iPhone and iPad
  aarch64-apple-ios-sim  # the simulator on an Apple-silicon Mac
  aarch64-linux-android  # every Android phone worth shipping to
)

if ! command -v rustup >/dev/null 2>&1; then
  echo "(skipping the device-target check: no rustup to add targets with)"
  exit 0
fi

# Whatever reads symbols. llvm-nm handles both ELF and Mach-O; GNU nm handles
# only ELF, so without llvm-nm the Apple archives can be built but not
# inspected, and the check says so rather than quietly passing.
#
# Saying so is enough on a laptop. In CI a check that quietly stops checking is
# worse than no check, so REQUIRE_SYMBOLS turns every such degradation into a
# failure.
REQUIRE_SYMBOLS="${EPHEMERAL_REQUIRE_SYMBOLS:-0}"
NM=""
for candidate in llvm-nm llvm-nm-19 llvm-nm-18 nm; do
  if command -v "$candidate" >/dev/null 2>&1; then
    NM="$candidate"
    break
  fi
done

# Every function the header promises. Read out of the header itself, so adding
# an export without declaring it — or declaring one without exporting it — is
# caught here without anybody remembering to edit this script. Prose mentions of
# a function name are not followed by a bracket, which is what separates a
# declaration from a sentence about one.
EXPECTED="$(grep -oE 'ephemeral_[a-z_]+\(' "$CRATE/include/ephemeral.h" \
  | tr -d '(' | sort -u)"
if [ -z "$EXPECTED" ]; then
  echo "no function declarations found in ephemeral.h — the check would be vacuous" >&2
  exit 1
fi

failures=0

# A degradation: something could not be inspected. Reported either way, fatal
# only where the whole point is that nothing goes unexamined.
unchecked() {
  if [ "$REQUIRE_SYMBOLS" = "1" ]; then
    echo "  FAIL  symbols must be checked here, but $1"
    failures=$((failures + 1))
  else
    echo "  ..    symbols unchecked: $1"
  fi
}

for target in "${TARGETS[@]}"; do
  printf '\n== %s\n' "$target"

  if ! rustup target add "$target" >/dev/null 2>&1; then
    echo "  .. skipped: this toolchain has no standard library for it"
    continue
  fi

  # Only the static library. A cdylib would need the platform's linker, which
  # is the one part of a device build this machine genuinely cannot do, and it
  # is not what an app embeds anyway.
  cargo rustc -p ephemeral-ffi --target "$target" --crate-type staticlib \
    >/dev/null 2>"$ROOT/target/device-$target.log" || {
    echo "  FAIL  it does not build for this target:"
    sed 's/^/        /' "$ROOT/target/device-$target.log" | tail -20
    failures=$((failures + 1))
    continue
  }

  archive="target/$target/debug/libephemeral_ffi.a"
  if [ ! -f "$archive" ]; then
    echo "  FAIL  no archive at $archive"
    failures=$((failures + 1))
    continue
  fi
  echo "  ok    it builds: $(du -h "$archive" | cut -f1)"

  if [ -z "$NM" ]; then
    unchecked "nothing here reads object files"
    continue
  fi

  # Mach-O decorates C symbols with a leading underscore and ELF does not, so
  # the comparison drops it rather than special-casing the platform.
  found="$("$NM" --defined-only "$archive" 2>/dev/null \
    | grep -oE '\b_?ephemeral_[a-z_]+' | sed 's/^_//' | sort -u || true)"
  if [ -z "$found" ]; then
    unchecked "$NM cannot read this archive's format"
    continue
  fi

  missing=""
  for symbol in $EXPECTED; do
    if ! printf '%s\n' "$found" | grep -qx "$symbol"; then
      missing="$missing $symbol"
    fi
  done

  if [ -n "$missing" ]; then
    echo "  FAIL  the header promises functions this archive does not have:$missing"
    failures=$((failures + 1))
  else
    count="$(printf '%s\n' "$EXPECTED" | wc -l | tr -d ' ')"
    echo "  ok    all $count exported functions are present"
  fi
done

if [ "$failures" -ne 0 ]; then
  printf '\nEphemeral does not reach the device.\n'
  exit 1
fi

printf '\nEphemeral builds for the device.\n'
