param([switch]$SkipChecks)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    if (-not $SkipChecks) {
        & cargo fmt -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed' }
        & cargo clippy --locked --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
        & cargo test --locked
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    }
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    Write-Output (Join-Path $root 'target\release\eve-combat-assistant.exe')
} finally {
    Pop-Location
}
