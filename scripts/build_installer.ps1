param([string]$Compiler = 'makensis.exe')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$payload = Join-Path $root 'dist\EVECombatAssistant\eve-combat-assistant.exe'
if (-not (Test-Path $payload)) {
    throw 'Run build_windows.ps1 and package_windows.ps1 first'
}
& $Compiler -WX (Join-Path $root 'installer\setup.nsi')
if ($LASTEXITCODE -ne 0) { throw 'Installer build failed' }
Write-Output (Join-Path $root 'dist\EVECombatAssistant-Setup-x64.exe')
