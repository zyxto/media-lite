#!/bin/bash
# Construye media-lite-x86_64.AppImage dentro de Debian 11 (bullseye, glibc 2.31), para que
# ande en antiX 21/23, Debian 11/12/13, Ubuntu 20.04+ y MX. Ver roadmap, sección 2.8 y Fase 9.
#
# Uso (desde la raíz del repo):   packaging/build-appimage.sh
# Salida:                         dist/media-lite-x86_64.AppImage (+ .sha256)
#
# El build corre en un contenedor debian:bullseye. El binario no lleva mpv (se usa el del
# sistema), ni libX11, ni libGL: se cargan con dlopen en el equipo.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${MEDIA_LITE_BUILD_IMAGE:-debian:bullseye}"

if [ "${MEDIA_LITE_IN_CONTAINER:-0}" != "1" ]; then
    # Afuera: se relanza el mismo script dentro del contenedor.
    exec docker run --rm \
        -e MEDIA_LITE_IN_CONTAINER=1 \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        -v "$ROOT":/src -w /src \
        "$IMAGE" bash packaging/build-appimage.sh
fi

# Dentro del contenedor.
export DEBIAN_FRONTEND=noninteractive
# Debian 11 terminó su soporte en agosto de 2026: sus paquetes ya están en el archivo.
# Solo dentro de este contenedor descartable se apunta apt ahí.
if grep -rq "deb.debian.org" /etc/apt/sources.list /etc/apt/sources.list.d/ 2>/dev/null; then
    sed -i -e 's|deb.debian.org/debian-security|archive.debian.org/debian-security|g' \
        -e 's|deb.debian.org/debian|archive.debian.org/debian|g' \
        -e 's|security.debian.org|archive.debian.org/debian-security|g' \
        -e '/bullseye-updates/d' /etc/apt/sources.list
    echo 'Acquire::Check-Valid-Until "false";' > /etc/apt/apt.conf.d/99archive
fi
# Los espejos a veces están a medio sincronizar: se reintenta.
for intento in 1 2 3; do
    if apt-get update -qq && apt-get install -y -qq --no-install-recommends \
        ca-certificates curl build-essential pkg-config binutils file >/dev/null; then
        break
    fi
    echo "apt falló (intento $intento), reintentando…" >&2
    sleep 15
    [ "$intento" = 3 ] && exit 1
done

if ! command -v cargo >/dev/null 2>&1; then
    curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable
fi
# shellcheck disable=SC1091
. "$HOME/.cargo/env"

cd /src
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
echo "==> media-lite $VERSION"

cargo build --release --locked -p media-lite

BIN=target/release/media-lite
echo "==> versiones de glibc que pide el binario (deben ser <= 2.31)"
MAX_GLIBC="$(objdump -T "$BIN" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
echo "    máxima: $MAX_GLIBC"
if [ "$(printf '%s\n%s\n' "GLIBC_2.31" "$MAX_GLIBC" | sort -V | tail -1)" != "GLIBC_2.31" ]; then
    echo "error: el binario necesita $MAX_GLIBC, más que glibc 2.31" >&2
    exit 1
fi

APPDIR=AppDir
rm -rf "$APPDIR" dist
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps" "$APPDIR/usr/share/doc/media-lite"
install -m 755 "$BIN" "$APPDIR/usr/bin/media-lite"
install -m 755 packaging/AppRun "$APPDIR/AppRun"
install -m 644 packaging/media-lite.desktop "$APPDIR/media-lite.desktop"
install -m 644 packaging/media-lite.desktop "$APPDIR/usr/share/applications/media-lite.desktop"
install -m 644 packaging/media-lite.png "$APPDIR/media-lite.png"
install -m 644 packaging/media-lite.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/media-lite.png"
install -m 644 LICENSE "$APPDIR/usr/share/doc/media-lite/LICENSE"
install -m 644 app/assets/fonts/OFL.txt "$APPDIR/usr/share/doc/media-lite/Inter-OFL.txt"

# Herramientas de AppImage. Sin FUSE dentro del contenedor, se usa --appimage-extract-and-run.
TOOLS=/tmp/appimage-tools
mkdir -p "$TOOLS"
if [ ! -x "$TOOLS/appimagetool" ]; then
    curl -fsSL -o "$TOOLS/appimagetool" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOLS/appimagetool"
fi
RUNTIME="$TOOLS/runtime-x86_64"
if [ ! -f "$RUNTIME" ]; then
    curl -fsSL -o "$RUNTIME" https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64
fi

mkdir -p dist
ARCH=x86_64 "$TOOLS/appimagetool" --appimage-extract-and-run -n --runtime-file "$RUNTIME" \
    "$APPDIR" "dist/media-lite-x86_64.AppImage"
(cd dist && sha256sum media-lite-x86_64.AppImage > media-lite-x86_64.AppImage.sha256)

chown -R "${HOST_UID:-0}:${HOST_GID:-0}" dist "$APPDIR" target 2>/dev/null || true
echo "==> listo: dist/media-lite-x86_64.AppImage ($(du -h dist/media-lite-x86_64.AppImage | cut -f1))"
