#!/usr/bin/env bash
#
# Puts the iOS application through a compiler.
#
# Until this existed, the only Swift in this repository was a snippet in a
# document — code nobody had ever compiled, which is a different thing from
# code that works. There is no Xcode project here yet and no signing identity,
# so this does not build an .ipa; what it does is type-check every source file
# against the real iOS SDK, with the real C header, through the module map the
# XCFramework carries. That catches everything a compiler catches, which on a
# SwiftUI screen is most of what there is to catch.
#
# Needs a Mac. It says so and exits cleanly anywhere else, because the rest of
# this repository must stay workable on a machine that is not one.
#
#   apps/ios/typecheck.sh

set -euo pipefail

APP="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$APP/../.." && pwd)"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "(skipping the iOS type-check: Swift for iOS needs a Mac, and this is $(uname -s))"
  exit 0
fi

if ! command -v xcrun >/dev/null 2>&1; then
  echo "(skipping the iOS type-check: no xcrun, so there is no iOS SDK here)"
  exit 0
fi

SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"
# The simulator, on whichever architecture this Mac is. A device triple would
# type-check identically and is what `device-targets.sh` already covers for the
# engine; the simulator is what a person building this would run.
case "$(uname -m)" in
  arm64) TARGET="arm64-apple-ios17.0-simulator" ;;
  *) TARGET="x86_64-apple-ios17.0-simulator" ;;
esac

echo "==> Type-checking against $TARGET"
echo "    SDK: $SDK"

# `-typecheck` rather than `-emit-object`: nothing is being linked, so the
# engine's static library does not need to be here — only its header, which is
# what the module map names.
xcrun swiftc \
  -typecheck \
  -sdk "$SDK" \
  -target "$TARGET" \
  -swift-version 5 \
  -Xcc -fmodule-map-file="$ROOT/crates/ephemeral-ffi/include/module.modulemap" \
  -Xcc -I"$ROOT/crates/ephemeral-ffi/include" \
  "$APP"/Sources/Ephemeral/*.swift

echo "The iOS application type-checks."
