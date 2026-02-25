#!/usr/bin/env bash
set -euo pipefail

on_error() {
  echo "Error occurred in $(basename "$0") at line $1" >&2
  exit 1
}
trap 'on_error $LINENO' ERR

DIR=$(cd "$(dirname "$0")" && pwd)
PROJ=$(cd "$DIR/.." && pwd)
cd "$PROJ"
VERSION="${VERSION:-0.1.0}"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
BIN_NAME="${BINARY_NAME:-server}"
cargo build --release
BIN="target/release/${BIN_NAME}"
if [ ! -x "$BIN" ]; then
  exit 1
fi
WORK="$(mktemp -d)"
PKG="${WORK}/pkg"
mkdir -p "${PKG}/DEBIAN" "${PKG}/usr/bin" "${PKG}/lib/systemd/system"
install -m 0755 "$BIN" "${PKG}/usr/bin/${BIN_NAME}"
cp "$DIR/systemd/phs-nasserver.service" "${PKG}/lib/systemd/system/phs-nasserver.service"
cp "$DIR/debian/control" "${PKG}/DEBIAN/control"
cp "$DIR/debian/postinst" "${PKG}/DEBIAN/postinst"
chmod 755 "${PKG}/DEBIAN/postinst"
OUT="$(cd "$PROJ/artifacts" 2>/dev/null || mkdir -p "$PROJ/artifacts"; echo "$PROJ/artifacts")"
dpkg-deb -b "$PKG" "${OUT}/nasserver_${VERSION}_${ARCH}.deb" >/dev/null
echo "${OUT}/nasserver_${VERSION}_${ARCH}.deb"
