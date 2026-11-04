#!/bin/sh
# Pack a built Lumen into the Linux release tarball that dist/install.sh
# installs from:
#
#   scripts/package-linux.sh <arch> [out-dir]   e.g. scripts/package-linux.sh x86_64 dist-out
#
# It packs target/release/lumen (build it first: cargo build --release --locked)
# into <out-dir>/lumen-linux-<arch>.tar.gz, laid out like a prefix:
#
#   lumen-linux-<arch>/bin/lumen
#   lumen-linux-<arch>/share/applications/lumen.desktop
#   lumen-linux-<arch>/share/icons/hicolor/*/apps/lumen.*
#   lumen-linux-<arch>/LICENSE-MIT, LICENSE-APACHE
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
arch=${1:?usage: $0 <arch> [out-dir]}
out=${2:-$repo/target/dist}
bin=$repo/target/release/lumen
[ -x "$bin" ] || { echo "No $bin: build it first (cargo build --release --locked)." >&2; exit 1; }

name=lumen-linux-$arch
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root=$stage/$name

mkdir -p "$root/bin" "$root/share/applications" "$root/share/icons"
cp "$bin" "$root/bin/lumen"
cp "$repo/assets/linux/lumen.desktop" "$root/share/applications/"
cp -R "$repo/assets/icons/linux/hicolor" "$root/share/icons/"
cp "$repo/LICENSE-MIT" "$repo/LICENSE-APACHE" "$root/"

mkdir -p "$out"
out=$(cd "$out" && pwd)
# Sorted, with fixed owners, so the same build packs to the same bytes.
(cd "$stage" && tar --sort=name --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name")
echo "Made $out/$name.tar.gz"
