# Build the Linux meninx-server binaries the app installs on SSH servers (docs/remote-install.md):
# static (musl) x86_64 and aarch64, profile `remote` (stripped), copied to <AppDir>\remote\linux-<arch>\.
# Needs zig (on PATH, or <toolchain>\zig\zig.exe beside CARGO_HOME) and cargo-zigbuild.
#
# Windows PowerShell 5.1. Exit code 0 = both built; 1 = a build failed; 2 = zig / cargo-zigbuild missing.
param(
    [Parameter(Mandatory)] [string] $AppDir,       # where meninx.exe is (<target>\release)
    [string] $TargetDir = $env:CARGO_TARGET_DIR
)
$ErrorActionPreference = 'Stop'

$zig = (Get-Command zig -ErrorAction SilentlyContinue).Source
if (-not $zig -and $env:CARGO_HOME) {
    $guess = Join-Path (Split-Path -Parent $env:CARGO_HOME) 'zig\zig.exe'
    if (Test-Path -LiteralPath $guess) { $zig = $guess }
}
if (-not $zig) { Write-Host 'zig not found: Linux servers not built'; exit 2 }
$env:Path = (Split-Path -Parent $zig) + ';' + $env:Path
if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) {
    Write-Host 'cargo-zigbuild not found (cargo install cargo-zigbuild): Linux servers not built'
    exit 2
}
if (-not $TargetDir) { $TargetDir = 'target' }

$failed = $false
foreach ($t in @(@('x86_64-unknown-linux-musl', 'linux-x86_64'), @('aarch64-unknown-linux-musl', 'linux-aarch64'))) {
    $triple, $name = $t
    Write-Host "> cargo zigbuild --profile remote -p meninx-server --target $triple"
    & cargo zigbuild --profile remote -p meninx-server --target $triple
    if ($LASTEXITCODE -ne 0) { Write-Host "build for $triple failed"; $failed = $true; continue }
    $out = Join-Path $AppDir "remote\$name"
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    Copy-Item -LiteralPath (Join-Path $TargetDir "$triple\remote\meninx-server") -Destination (Join-Path $out 'meninx-server') -Force
    Write-Host "copied to $out"
}
if ($failed) { exit 1 }
exit 0
