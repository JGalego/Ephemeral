<#
.SYNOPSIS
    Ephemeral development bootstrap (Windows).

.DESCRIPTION
    Installs the toolchain and developer tools Ephemeral needs, then verifies
    the environment. Safe to run repeatedly. It does not install Docker:
    Ephemeral detects Docker at runtime and explains its absence, and no part of
    the development workflow requires it (see ADR-0005).

.EXAMPLE
    scripts\bootstrap.ps1
#>

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Step($m) { Write-Host "==> $m" -ForegroundColor White }
function Ok($m)   { Write-Host "  [ok] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [!]  $m" -ForegroundColor Yellow }
function Fail($m) { Write-Host "  [x]  $m" -ForegroundColor Red }
function Note($m) { Write-Host "       $m" -ForegroundColor DarkGray }

Write-Host ""
Write-Host "Ephemeral - development bootstrap" -ForegroundColor White
Write-Host ""

# --- Rust toolchain ---------------------------------------------------------

Step "Rust toolchain"
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    Fail "Rust is not installed."
    Note "Install it from https://rustup.rs and re-run this script:"
    Note "  winget install Rustlang.Rustup"
    exit 1
}

# Installs the pinned toolchain and components from rust-toolchain.toml.
rustup show active-toolchain | Out-Null
Ok (rustc --version)
Ok (cargo --version)

foreach ($component in @('rustfmt', 'clippy')) {
    $installed = rustup component list --installed
    if ($installed -notcontains $component -and -not ($installed | Select-String "^$component")) {
        Step "Installing $component"
        rustup component add $component
    }
}
Ok "rustfmt and clippy available"

# --- Developer tools --------------------------------------------------------

Step "Developer tools"
if (Get-Command cargo-deny -ErrorAction SilentlyContinue) {
    Ok "cargo-deny available"
} else {
    Warn "cargo-deny not installed (supply-chain checks will be skipped locally)"
    Note "install with: cargo install --locked cargo-deny"
}

# --- Optional: container runtime -------------------------------------------

Step "Container runtime (optional)"
if (Get-Command docker -ErrorAction SilentlyContinue) {
    docker info *> $null
    if ($LASTEXITCODE -eq 0) {
        Ok "Docker available and running"
    } else {
        Warn "Docker is installed but the daemon is not reachable"
        Note "Start Docker Desktop and try again."
    }
} else {
    Warn "Docker not found"
    Note "Not required. Ephemeral detects Docker at runtime and explains its"
    Note "absence; app creation, inspection and lifecycle work without it."
}

# --- Build and verify -------------------------------------------------------

Step "Fetching dependencies"
cargo fetch
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Ok "dependencies resolved"

Step "Building the workspace"
cargo build --workspace --all-targets
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Ok "build succeeded"

Step "Running tests"
cargo test --workspace
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Ok "tests passed"

Write-Host ""
Write-Host "Ready." -ForegroundColor Green
Write-Host "  cargo run -p ephemeral-cli -- doctor      # check your environment"
Write-Host "  cargo run -p ephemeral-cli -- states      # show the lifecycle state machine"
Write-Host ""
