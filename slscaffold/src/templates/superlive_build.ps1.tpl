#Requires -Version 5.1
# __NAME__ -> superlive component build entry.
#
# superlive's managed build/package flow (inside the shell) invokes this script
# to build the component's planes; the shell owns assembly, fingerprinting and
# packaging - this script's job ends when the build outputs exist.
#
# This is your single build customization point: if your component needs a
# custom build (workspace layout, cross-compilation, private build steps),
# change this script.
#
# Form adaptation: the build sections below check the directory layout at
# runtime - a server/ directory triggers the backend build, a web/ directory
# triggers the frontend build.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir

function Log([string]$msg) { Write-Host "==> $msg" }
function Die([string]$msg) { Write-Error "error: $msg"; exit 1 }

function Test-Command([string]$name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

$HasServerDir = $false
$HasWebDir = $false
#@if HAS_SERVER
if (Test-Path (Join-Path $Root 'server')) { $HasServerDir = $true }
#@endif
#@if HAS_WEB
if (Test-Path (Join-Path $Root 'web')) { $HasWebDir = $true }
#@endif

if (-not $HasServerDir -and -not $HasWebDir) {
    Die "nothing to build: neither server/ nor web/ exists under $Root"
}

#@if HAS_SERVER
if ($HasServerDir) {
    if (-not (Test-Command 'cargo')) { Die 'missing required command: cargo' }
    Log 'building server: cargo build --release'
    Push-Location (Join-Path $Root 'server')
    try { cargo build --release }
    finally { Pop-Location }
    $ServerBin = Join-Path $Root 'server/target/release/__ID__-server'
    if (-not (Test-Path $ServerBin)) { Die "release binary does not exist: $ServerBin" }
}
#@endif

#@if HAS_WEB
if ($HasWebDir) {
    # pnpm when available, npm otherwise
    $PM = if (Test-Command 'pnpm') { 'pnpm' } elseif (Test-Command 'npm') { 'npm' } else { $null }
    if (-not $PM) { Die 'missing required command: pnpm (or npm)' }
    Log "building web: $PM run build (output web/dist)"
    Push-Location (Join-Path $Root 'web')
    try { & $PM run build }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { Die 'web build failed' }
    # Component build-shape contract (web/vite.config.js): the entry is the fixed
    # name entry.js (single rollup input src/entry.tsx + entryFileNames 'entry.js');
    # index.html never enters the build output.
    $WebEntry = Join-Path $Root 'web/dist/entry.js'
    if (-not (Test-Path $WebEntry)) { Die 'incomplete web build output: web/dist/entry.js does not exist' }
}
#@endif

Log 'build complete'
