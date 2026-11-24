#!/usr/bin/env bash
#
# Build the Terax desktop app on a Mac and drop the bundle in dist-mac/.
#
# This has to run on the Mac itself. A .app links against WebKit and is stamped
# by codesign, both of which live in the macOS SDK — there is no cross-compile
# from the Linux box that hosts phantom-server.
#
# The release workflow (.github/workflows/release.yml) builds the same bundle on
# a macos runner and signs and notarizes it. This script does neither: the
# result is ad-hoc signed, which is enough to run on the machine that built it.

set -euo pipefail

cd "$(dirname "$0")/.."
out="dist-mac"

die() { echo "error: $*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "this builds a .app — run it on the Mac, not on $(uname -s)"

# Xcode command line tools: clang, the SDK, codesign, hdiutil.
xcode-select -p >/dev/null 2>&1 || die "Xcode command line tools missing — run: xcode-select --install"

command -v cargo >/dev/null || die "cargo missing — install rustup: https://rustup.rs"
command -v node  >/dev/null || die "node missing — install Node 22 or newer"

node_major=$(node -p 'process.versions.node.split(".")[0]')
[ "$node_major" -ge 22 ] || die "node $node_major is too old — this build needs 22 or newer"

# pnpm ships with node via corepack; the version is pinned in package.json.
command -v pnpm >/dev/null || corepack enable >/dev/null 2>&1 || die "pnpm missing — run: corepack enable"

echo "==> installing frontend dependencies"
pnpm install --frozen-lockfile

# The checked-in config points the updater at a signing key that only CI holds,
# and tauri refuses to build updater artifacts without it. A local build has
# nothing to update from, so turn them off rather than ask for the key.
echo "==> building (this compiles the Rust side — the first run takes a while)"
pnpm tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}'

bundle="src-tauri/target/release/bundle"
[ -d "$bundle" ] || die "no bundle directory at $bundle — the build did not get that far"

rm -rf "$out/Phantom.app" "$out/Terax.app" "$out"/*.dmg
mkdir -p "$out"
# The .app is what you drag to /Applications; the .dmg is what you send to
# another Mac. Both, when the build produced both.
[ -d "$bundle/macos/Phantom.app" ] && cp -R "$bundle/macos/Phantom.app" "$out/"
find "$bundle/dmg" -name '*.dmg' -maxdepth 1 -exec cp {} "$out/" \; 2>/dev/null || true

echo
echo "==> done"
ls -1d "$out"/Phantom.app "$out"/*.dmg 2>/dev/null || die "the build finished but produced no bundle"
echo
echo "Install:  cp -R $out/Phantom.app /Applications/"
echo "Then follow $out/README.md to point it at the room."
