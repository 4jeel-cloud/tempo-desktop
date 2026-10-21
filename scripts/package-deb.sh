#!/usr/bin/env bash
#
# Build a Debian package for Tempo Desktop.
#
# Produces dist/tempo-desktop_<version>_<arch>.deb containing the release binary, the desktop
# entry and every hicolor icon, so a friend on Debian or Ubuntu can install it with a single
# `apt install ./…deb` and launch it from the menu with no toolchain and no setup.
#
# Run from anywhere; the repo root is derived from the script path. Requires cargo, dpkg-deb
# and fakeroot.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
ARCH="$(dpkg --print-architecture)"
NAME="tempo-desktop"
MAINTAINER="4jeel-cloud <https://github.com/4jeel-cloud>"
OUT="$ROOT/dist"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "tempo-deb: building release binary (version $VERSION, arch $ARCH)"
LOCK=""
[ -f Cargo.lock ] && LOCK="--locked"
cargo build --release $LOCK --package sonora

BIN="target/release/sonora"
if [ ! -x "$BIN" ]; then
  echo "tempo-deb: $BIN is missing after the build" >&2
  exit 1
fi

# Layout: /usr/bin, /usr/share/applications, /usr/share/icons/hicolor, /usr/share/doc.
install -Dm755 "$BIN" "$STAGE/usr/bin/sonora"
install -Dm644 assets/linux/sonora.desktop "$STAGE/usr/share/applications/sonora.desktop"
install -Dm644 assets/linux/sonora.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/sonora.svg"
for icon in assets/linux/icons/hicolor/*/apps/sonora.png; do
  size="$(basename "$(dirname "$(dirname "$icon")")")"
  install -Dm644 "$icon" "$STAGE/usr/share/icons/hicolor/$size/apps/sonora.png"
done

DOC="$STAGE/usr/share/doc/$NAME"
install -Dm644 COPYING "$DOC/copyright"
install -Dm644 THIRD-PARTY.md "$DOC/THIRD-PARTY.md"

# Dependencies: the shared libraries the binary links against, plus the runtime stacks GPUI
# loads at start (a GL/Vulkan driver, an ALSA bridge, and a font stack). YouTube playback is
# inside the binary, so nothing else is needed to play music.
DEPS="libc6, libstdc++6, libgcc-s1, libasound2, libsqlite3-0, libxkbcommon0, \
libfontconfig1, libfreetype6, libvulkan1, mesa-vulkan-drivers, \
libwayland-client0, libxkbcommon-x11-0, libpipewire-0.3-0"

SIZE="$(du -sk "$STAGE/usr" | cut -f1)"
mkdir -p "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<EOF
Package: $NAME
Version: $VERSION
Section: sound
Priority: optional
Architecture: $ARCH
Depends: $DEPS
Installed-Size: $SIZE
Maintainer: $MAINTAINER
Description: Native YouTube Music client with local playback
 Tempo Desktop is a native music streaming client built with Rust and GPUI.
 It plays YouTube Music, signed in or as a guest, alongside your local files,
 with gapless playback, synced lyrics and scrobbling.
EOF

mkdir -p "$OUT"
DEB="$OUT/${NAME}_${VERSION}_${ARCH}.deb"
fakeroot dpkg-deb --build --root-owner-group "$STAGE" "$DEB"

echo "tempo-deb: wrote $DEB"
dpkg-deb --info "$DEB" | sed -n '1,20p'
