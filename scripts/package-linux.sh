#!/usr/bin/env bash
# Arma Orion para Linux: target/dist/orion-<versión>-linux-x64.tar.gz (+ su SHA256).
# Uso: ./scripts/package-linux.sh   (en Linux x86_64)
# Adentro van el binario, un orion.desktop y el ícono. Para instalarlo, ver "Linux" en el README.
# tar.gz y no zip: guarda el permiso de ejecución del binario.
set -euo pipefail
cd "$(dirname "$0")/.."

# La versión sale de Cargo.toml (ver package-macos.sh).
pkgid=$(cargo pkgid)
version=${pkgid##*[#@]}

cargo build --release

name=orion-$version-linux-x64
dist=target/dist
stage=$dist/$name
rm -rf "$stage"
mkdir -p "$stage"

cp target/release/orion "$stage/orion"
strip "$stage/orion"

# El ícono sale del mismo dibujo que el de Windows y macOS (src/icon.rs).
rm -rf target/orion.iconset
cargo test --quiet write_iconset -- --ignored
cp target/orion.iconset/icon_256x256.png "$stage/orion.png"

cat >"$stage/orion.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Orion
Comment=Tus terminales y agentes, en un solo lugar
Exec=orion
Icon=orion
Terminal=false
Categories=System;TerminalEmulator;Development;
StartupWMClass=orion
EOF

tarball=$dist/$name.tar.gz
rm -f "$tarball" "$tarball.sha256"
tar -czf "$tarball" -C "$dist" "$name"
# El hash permite verificar que el archivo no cambió.
hash=$(sha256sum "$tarball" | cut -d' ' -f1)
echo "$hash  $name.tar.gz" >"$tarball.sha256"

echo "Listo: $tarball ($(du -h "$tarball" | cut -f1))"
