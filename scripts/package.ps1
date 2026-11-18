# Arma el zip portable de Orion: target\dist\orion-<versión>-windows-x64.zip (+ su SHA256).
# Uso: .\scripts\package.ps1
# Adentro van el .exe, el README y la licencia. Se descomprime y se usa: no instala nada.

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

# La versión sale de Cargo.toml.
$version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
cargo build --release
if ($LASTEXITCODE) { exit $LASTEXITCODE }

$name = "orion-$version-windows-x64"
$stage = "target\dist\$name"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null
Copy-Item target\release\orion.exe, README.md $stage
if (Test-Path LICENSE) { Copy-Item LICENSE $stage }

$zip = "target\dist\$name.zip"
Compress-Archive -Path $stage -DestinationPath $zip -Force
# El hash permite verificar que el zip no cambió (winget lo exige).
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash
"$hash  $name.zip" | Set-Content "$zip.sha256"

$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Listo: $zip ($mb MB)"
Write-Host "SHA256: $hash"
