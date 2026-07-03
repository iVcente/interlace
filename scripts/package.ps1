# Build a release binary and bundle it into a distributable .zip under dist\.
#
#   scripts\package.ps1
#
# Produces dist\interlace-<version>-win-x64.zip containing the exe, the README,
# and the icon. Run from anywhere; paths are resolved relative to the repo root.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Read the crate version out of Cargo.toml
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Packaging Interlace v$version" -ForegroundColor Cyan

# A running instance would lock the exe; stop it first
Get-Process interlace -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Host "Building release..." -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) 
{ 
    throw "cargo build --release failed"
}

$exe = "$root\target\release\interlace.exe"
if (-not (Test-Path $exe)) 
{ 
    throw "expected binary not found: $exe" 
}

# Stage the payload
$stage = "$root\dist\interlace-$version-win-x64"
if (Test-Path $stage) 
{ 
    Remove-Item -Recurse -Force $stage 
}

New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage
Copy-Item "$root\README.md" $stage
Copy-Item "$root\assets\icon.png" "$stage\icon.png"

# Zip it
$zip = "$stage.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip

$size = "{0:N1} MB" -f ((Get-Item $zip).Length / 1MB)
Write-Host "Wrote $zip ($size)" -ForegroundColor Green
