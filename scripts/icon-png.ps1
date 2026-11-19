# Arma assets/orion.png (el ícono que muestra el README) a partir del render de src/icon.rs.
# Uso:
#   cargo test write_icon -- --ignored   # escribe assets/orion.ico y target/icon-512.rgba
#   .\scripts\icon-png.ps1

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
Add-Type -AssemblyName System.Drawing

$size = 512
$bytes = [System.IO.File]::ReadAllBytes("$PWD\target\icon-512.rgba")
# RGBA → BGRA, el orden que usa Windows (Format32bppArgb).
for ($i = 0; $i -lt $bytes.Length; $i += 4) {
    $r = $bytes[$i]; $bytes[$i] = $bytes[$i + 2]; $bytes[$i + 2] = $r
}
$bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$rect = [System.Drawing.Rectangle]::new(0, 0, $size, $size)
$data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::WriteOnly, $bmp.PixelFormat)
[System.Runtime.InteropServices.Marshal]::Copy($bytes, 0, $data.Scan0, $bytes.Length)
$bmp.UnlockBits($data)
$bmp.Save("$PWD\assets\orion.png", [System.Drawing.Imaging.ImageFormat]::Png)
Write-Host "Listo: assets\orion.png"
