#!/usr/bin/env bash
#
# Fetch Bootstrap Icons and write fonts/BootstrapIcons.ttf and its license.
#
# Run this only to move to another Bootstrap Icons release, then commit the
# output along with your change.
#
# The TTF is committed rather than built on purpose:
#
#   - Upstream ships the font only as WOFF and WOFF2. `fonts.rs` embeds plain
#     TrueType, because the FreeType we link is not guaranteed to have zlib and
#     brotli, and a fallback face that fails to parse only logs a line.
#   - Converting from build.rs would put a C++ tool and a network download on the
#     critical path of every build, on four CI runners, in the Nix derivation
#     and on every contributor's machine, for a file that changes about once a
#     year.
#
# The whole set is kept, not just the glyphs the list prefixes use: a face draws
# only what is asked of it, and a stylesheet or a site may ask for any icon.
#
# woff2 is not in the dev shell, since this runs so rarely:
#
#   nix shell nixpkgs#woff2 -c scripts/gen-icon-font.sh

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=1.13.2
SHA256=7b5960145e1ab14a49a117f96a9e5a54a94fdf07f21e302d9e4f88d73dd19f9c

if ! command -v woff2_decompress >/dev/null 2>&1; then
    echo "woff2_decompress not found, see this script's header" >&2
    exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -sSfL -o "$work/package.tgz" \
    "https://registry.npmjs.org/bootstrap-icons/-/bootstrap-icons-$VERSION.tgz"
echo "$SHA256  $work/package.tgz" | sha256sum -c --quiet

tar xzf "$work/package.tgz" -C "$work" \
    package/font/fonts/bootstrap-icons.woff2 package/LICENSE

woff2_decompress "$work/package/font/fonts/bootstrap-icons.woff2"
cp "$work/package/font/fonts/bootstrap-icons.ttf" fonts/BootstrapIcons.ttf
cp "$work/package/LICENSE" fonts/LICENSE-BootstrapIcons.txt

echo "fonts/BootstrapIcons.ttf from bootstrap-icons $VERSION"
