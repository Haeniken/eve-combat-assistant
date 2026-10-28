param(
    [string]$Executable,
    [string]$LicenseDirectory,
    [string]$SeedDirectory
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Executable) { $Executable = Join-Path $root 'target\release\eve-combat-assistant.exe' }
if (-not $LicenseDirectory) { $LicenseDirectory = Join-Path $root 'third_party\licenses' }
if (-not (Test-Path $Executable)) { throw "Executable not found: $Executable" }
if (-not (Test-Path (Join-Path $LicenseDirectory 'DEPENDENCIES.json'))) { throw 'Prepare dependency licenses before packaging' }
if (-not $SeedDirectory) { $SeedDirectory = Join-Path $root 'build\bootstrap-v0.3.4' }
$seedDatabase = Join-Path $SeedDirectory 'seed.sqlite3'
$seedManifest = Join-Path $SeedDirectory 'seed-manifest.json'
if (-not (Test-Path $seedDatabase) -or -not (Test-Path $seedManifest)) { throw 'Prepare the starter database with scripts/build_bootstrap.py first' }
$manifest = Get-Content $seedManifest -Raw | ConvertFrom-Json
$upstream = Get-Content (Join-Path $root 'native\upstream.json') -Raw | ConvertFrom-Json
if ($manifest.schema_version -ne 1 -or $manifest.sde_build -ne $upstream.sde_build -or $manifest.complete_days -lt 90 -or $manifest.prepared_populations -ne $manifest.known_hulls) { throw 'Incomplete or incompatible starter database' }
if ((Get-FileHash $seedDatabase -Algorithm SHA256).Hash -ne $manifest.sha256) { throw 'Starter database hash mismatch' }
$dist = Join-Path $root 'dist'
$destination = Join-Path $dist 'EVECombatAssistant'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item $Executable (Join-Path $destination 'eve-combat-assistant.exe') -Force
$seedDestination = Join-Path $destination 'seed'
New-Item -ItemType Directory -Path $seedDestination -Force | Out-Null
Copy-Item $seedDatabase, $seedManifest $seedDestination -Force
foreach ($name in @('THIRD_PARTY_NOTICES.md', 'Запустить.cmd', 'Демонстрация.cmd')) {
    Copy-Item (Join-Path $root $name) $destination -Force
}
$licenses = Join-Path $destination 'licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
Copy-Item (Join-Path $LicenseDirectory '*') $licenses -Recurse -Force
Copy-Item (Join-Path $root 'resources\WINDOWS_README.txt') (Join-Path $destination 'README.txt') -Force
Copy-Item (Join-Path $root 'docs\pilot-rating.md') (Join-Path $destination 'PILOT_RATING.md') -Force
Copy-Item (Join-Path $root 'docs\fit-metrics.md') (Join-Path $destination 'FIT_METRICS.md') -Force
Get-ChildItem $destination -Recurse -File | Where-Object { $_.LastWriteTime.Year -lt 1980 -or $_.LastWriteTime.Year -gt 2107 } | ForEach-Object { $_.LastWriteTime = [datetime]'2000-01-01' }
$archive = Join-Path $dist 'EVECombatAssistant-windows-x64.zip'
# Create mode streams entries; Compress-Archive uses Update mode and buffers large files.
Add-Type -AssemblyName System.IO.Compression.FileSystem
if (Test-Path $archive) { Remove-Item $archive -Force }
[System.IO.Compression.ZipFile]::CreateFromDirectory($destination, $archive, [System.IO.Compression.CompressionLevel]::Optimal, $true)
Get-FileHash -Path $archive, (Join-Path $destination 'eve-combat-assistant.exe') -Algorithm SHA256 |
    Select-Object Path, Hash | ConvertTo-Json | Set-Content (Join-Path $dist 'SHA256.json') -Encoding UTF8
Write-Output $archive
