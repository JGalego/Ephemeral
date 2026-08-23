#!/usr/bin/env bash
#
# Builds Ephemeral's engine for Android and puts it where Gradle expects it.
#
# Gradle is deliberately not driving this. Cross-compiling Rust from a Gradle
# task hides the failure inside a build system that cannot explain it, and it
# makes the command different here and in CI. This is the command, in both.
#
# Needs: the Android NDK, and the four Rust targets. `rustup target add` them,
# or run scripts/bootstrap.
#
#     ANDROID_NDK_HOME=~/Android/Sdk/ndk/26.3.11579264 ./build-native.sh
#
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

# The API level the .so is compiled against. Must not be higher than minSdk in
# app/build.gradle.kts, or the app installs and then fails to load the library
# on exactly the older devices minSdk promised to support.
readonly API=26

ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [ -z "$ndk" ]; then
    # A single installed NDK is unambiguous; several are not, and guessing which
    # one somebody meant is how a build becomes irreproducible.
    sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
    if [ -d "$sdk/ndk" ]; then
        count="$(find "$sdk/ndk" -maxdepth 1 -mindepth 1 -type d | wc -l)"
        if [ "$count" = "1" ]; then
            ndk="$(find "$sdk/ndk" -maxdepth 1 -mindepth 1 -type d)"
        elif [ "$count" -gt 1 ]; then
            echo "Several NDKs are installed under $sdk/ndk." >&2
            echo "Set ANDROID_NDK_HOME to the one you want." >&2
            exit 1
        fi
    fi
fi

if [ -z "$ndk" ] || [ ! -d "$ndk" ]; then
    echo "No Android NDK found. Set ANDROID_NDK_HOME." >&2
    echo "Install one with: sdkmanager 'ndk;26.3.11579264'" >&2
    exit 1
fi

host="linux-x86_64"
case "$(uname -s)" in
    Darwin) host="darwin-x86_64" ;;
esac

toolchain="$ndk/toolchains/llvm/prebuilt/$host/bin"
if [ ! -d "$toolchain" ]; then
    echo "The NDK at $ndk has no toolchain for $host." >&2
    exit 1
fi

# target:abi:linker-prefix — the ABI directory names are Android's, not Rust's,
# and the two disagree often enough to be worth writing down rather than derived.
targets=(
    "aarch64-linux-android:arm64-v8a:aarch64-linux-android"
    "armv7-linux-androideabi:armeabi-v7a:armv7a-linux-androideabi"
    "x86_64-linux-android:x86_64:x86_64-linux-android"
    "i686-linux-android:x86:i686-linux-android"
)

# Which ABIs to build. All four, unless somebody says otherwise — a release has
# to run on whatever phone downloads it. `EPHEMERAL_ANDROID_ABIS=x86_64` is for
# the one case where that is waste: an emulator is a single architecture, and
# building the three it will never load is four times the wait for the same
# screenshot.
wanted="${EPHEMERAL_ANDROID_ABIS:-}"

libs="$here/app/src/main/jniLibs"
mkdir -p "$libs"

for entry in "${targets[@]}"; do
    target="${entry%%:*}"
    rest="${entry#*:}"
    abi="${rest%%:*}"
    prefix="${rest#*:}"

    if [ -n "$wanted" ] && [[ " $wanted " != *" $abi "* ]]; then
        continue
    fi

    linker="$toolchain/${prefix}${API}-clang"
    if [ ! -x "$linker" ]; then
        echo "No linker at $linker" >&2
        exit 1
    fi

    # Cargo reads this per target, which is why it is spelled in capitals with
    # the target's dashes turned into underscores.
    variable="CARGO_TARGET_$(echo "$target" | tr '[:lower:]-' '[:upper:]_')_LINKER"
    export "$variable=$linker"

    echo "  $abi"
    (cd "$root" && cargo build --release --locked -p ephemeral-android --target "$target")

    mkdir -p "$libs/$abi"
    cp "$root/target/$target/release/libephemeral_android.so" "$libs/$abi/"
done

echo
echo "Engine in $libs:"
find "$libs" -name '*.so' -exec ls -la {} +
