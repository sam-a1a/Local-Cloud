#!/bin/bash
#
# Cross-compiles the engine for the Mac and generates the Swift that calls it.
#
# This app is the second consumer of the Rust engine that lives beside it, and
# consuming it means two artefacts that no `git clone` provides: a static
# library per architecture, and the Swift that calls into it. Both are produced
# here, by an ordinary Cmd-B, rather than by a script somebody has to remember
# to run. Neither is checked in - they are build output of the Rust, and a stale
# copy of either is a bug that presents as something else entirely.
#
# Runs as a build phase, and also on its own:
#
#     macos/build-engine.sh          # whatever this Mac is
#     ARCHS="arm64 x86_64" macos/build-engine.sh
#
set -euo pipefail

# `$0` is this file wherever it was invoked from, so the paths below are the
# repository's rather than the caller's working directory.
macos_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace_root="$(dirname "$macos_dir")"
generated_dir="$macos_dir/Generated"

# Xcode started from Finder does not inherit a login shell's PATH, so `cargo` is
# frequently not on it. Rustup puts it in a known place; the bare name still
# works for anyone who installed it elsewhere and does have a PATH.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
    echo "error: cargo is not installed, or not in \$HOME/.cargo/bin. The engine cannot be built." >&2
    exit 1
fi

# Xcode names architectures; rustup names targets. Standalone, build for this
# machine - the same thing Xcode asks for in a Debug build.
: "${ARCHS:=$(uname -m)}"
targets=()
for arch in $ARCHS; do
    case "$arch" in
        arm64|aarch64) targets+=("aarch64-apple-darwin") ;;
        x86_64)        targets+=("x86_64-apple-darwin") ;;
        *) echo "error: no Rust target for architecture '$arch'." >&2; exit 1 ;;
    esac
done

installed="$(rustup target list --installed)"
for target in "${targets[@]}"; do
    if ! grep -qx "$target" <<<"$installed"; then
        echo "error: the Rust target $target is not installed. Run: rustup target add $target" >&2
        exit 1
    fi
done

# Two of the engine's dependencies compile C - sqlite and ring - so whatever
# Xcode exported into the environment reaches a C compiler. Pinned to the macOS
# SDK rather than inherited, because the app target's SDK is only the right one
# by coincidence, and taken from `xcrun` so this is also correct when run from a
# terminal where nothing set it at all. Unsetting it instead is what a first
# draft did, and it fails with "stdio.h not found".
export SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"

# These two are Xcode's search paths for the app, and have no business reaching
# a Rust dependency's C compiler.
unset CPATH LIBRARY_PATH

# Always `--release`, including for a Debug build of the app. A debug build of
# the engine is not a slower version of the same thing: chunking and hashing a
# file is the work, and unoptimised SHA-256 turns a transfer that should take
# seconds into one that takes minutes. Debugging the engine is what the Rust
# tests are for.
libraries=()
for target in "${targets[@]}"; do
    echo "note: building the engine for $target"
    ( cd "$workspace_root" && cargo build --release --target "$target" -p engine )
    libraries+=("$workspace_root/target/$target/release/liblocalcloud.a")
done

mkdir -p "$generated_dir"

# One library, however many architectures were asked for. `lipo` with a single
# input is a copy, so there is no branch for the common case.
lipo -create "${libraries[@]}" -output "$generated_dir/liblocalcloud.a"

# The Swift is generated from the library that was just built, and read out of
# the compiled artefact rather than from a description of it - so it cannot
# claim a method the engine does not export, and the bindings and the
# implementation cannot drift apart.
#
# The bindgen is a host binary, and a debug one deliberately: it runs for a
# fraction of a second, and compiling it optimised costs far more than it ever
# saves.
first_target="${targets[0]}"
( cd "$workspace_root" && cargo run --quiet --release --target "$first_target" \
    -p engine --bin uniffi-bindgen -- \
    generate \
    --library "$workspace_root/target/$first_target/release/liblocalcloud.dylib" \
    --language swift \
    --out-dir "$generated_dir" )

# uniffi writes `localcloudFFI.modulemap`; Clang only looks for a file called
# `module.modulemap` on an import search path. Renaming it is the whole of the
# integration - no bridging header, and the C stays in its own module where the
# generated Swift expects to import it from.
mv -f "$generated_dir/localcloudFFI.modulemap" "$generated_dir/module.modulemap"

echo "note: engine ready in $generated_dir"
