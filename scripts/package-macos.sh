#!/usr/bin/env bash
# Arma la app de Orion para macOS: target/dist/orion-<versión>-macos.zip (+ su SHA256).
# Uso: ./scripts/package-macos.sh   (en un Mac, con Xcode Command Line Tools)
# Adentro va Orion.app, universal: un solo binario para Apple Silicon (aarch64) e Intel (x86_64).
set -euo pipefail
cd "$(dirname "$0")/.."

# La versión sale de Cargo.toml: `cargo pkgid` imprime "path+file:///…#orion@0.4.0".
pkgid=$(cargo pkgid)
version=${pkgid##*@}

targets=(aarch64-apple-darwin x86_64-apple-darwin)
rustup target add "${targets[@]}"
for target in "${targets[@]}"; do
    cargo build --release --target "$target"
done

dist=target/dist
app=$dist/Orion.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# Los dos binarios en uno: macOS elige el de su procesador al abrirlo.
lipo -create -output "$app/Contents/MacOS/orion" \
    "target/aarch64-apple-darwin/release/orion" \
    "target/x86_64-apple-darwin/release/orion"

# El ícono sale del mismo dibujo que el de Windows (src/icon.rs).
rm -rf target/orion.iconset
cargo test --quiet write_iconset -- --ignored
for png in target/orion.iconset/*.png; do
    sips -s format png "$png" --out "$png" >/dev/null # recomprime (to_png los deja sin comprimir)
done
iconutil -c icns target/orion.iconset -o "$app/Contents/Resources/orion.icns"

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Orion</string>
    <key>CFBundleDisplayName</key>
    <string>Orion</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.1bnja.orion</string>
    <key>CFBundleExecutable</key>
    <string>orion</string>
    <key>CFBundleIconFile</key>
    <string>orion</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>© 2026 Benjamin Canales · MIT</string>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Firma "ad hoc" (sin certificado de Apple): Apple Silicon no abre binarios sin firmar,
# y firmar la .app entera sella el Info.plist y el ícono junto con el binario.
codesign --force --sign - "$app"
codesign --verify --strict "$app"

name=orion-$version-macos
zip=$dist/$name.zip
rm -f "$zip" "$zip.sha256"
# ditto (no zip): guarda los permisos y la firma tal como los espera macOS.
ditto -c -k --keepParent "$app" "$zip"
# El hash permite verificar que el zip no cambió (lo usa el actualizador).
hash=$(shasum -a 256 "$zip" | cut -d' ' -f1)
echo "$hash  $name.zip" >"$zip.sha256"

echo "Listo: $zip ($(du -h "$zip" | cut -f1))"
echo "Arquitecturas: $(lipo -archs "$app/Contents/MacOS/orion")"
echo "SHA256: $hash"
