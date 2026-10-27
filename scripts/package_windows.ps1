param(
    [string]$Executable,
    [string]$LicenseDirectory
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Executable) { $Executable = Join-Path $root 'target\release\eve-combat-assistant.exe' }
if (-not $LicenseDirectory) { $LicenseDirectory = Join-Path $root 'third_party\licenses' }
if (-not (Test-Path $Executable)) { throw "Executable not found: $Executable" }
if (-not (Test-Path (Join-Path $LicenseDirectory 'DEPENDENCIES.json'))) { throw 'Prepare dependency licenses before packaging' }
$dist = Join-Path $root 'dist'
$destination = Join-Path $dist 'EVECombatAssistant'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item $Executable (Join-Path $destination 'eve-combat-assistant.exe') -Force
foreach ($name in @('THIRD_PARTY_NOTICES.md', 'Запустить.cmd', 'Демонстрация.cmd')) {
    Copy-Item (Join-Path $root $name) $destination -Force
}
$licenses = Join-Path $destination 'licenses'
New-Item -ItemType Directory -Path $licenses -Force | Out-Null
Copy-Item (Join-Path $LicenseDirectory '*') $licenses -Recurse -Force
Copy-Item (Join-Path $root 'resources\WINDOWS_README.txt') (Join-Path $destination 'README.txt') -Force
Get-ChildItem $destination -Recurse -File | Where-Object { $_.LastWriteTime.Year -lt 1980 -or $_.LastWriteTime.Year -gt 2107 } | ForEach-Object { $_.LastWriteTime = [datetime]'2000-01-01' }
$archive = Join-Path $dist 'EVECombatAssistant-windows-x64.zip'
Compress-Archive -Path $destination -DestinationPath $archive -Force -CompressionLevel Optimal
Get-FileHash -Path $archive, (Join-Path $destination 'eve-combat-assistant.exe') -Algorithm SHA256 |
    Select-Object Path, Hash | ConvertTo-Json | Set-Content (Join-Path $dist 'SHA256.json') -Encoding UTF8
Write-Output $archive
