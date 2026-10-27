param([Parameter(Mandatory = $true)][string]$InstallDirectory)
$ErrorActionPreference = 'Stop'
$executable = [System.IO.Path]::GetFullPath((Join-Path $InstallDirectory 'eve-combat-assistant.exe'))

# Закрываем только копии из удаляемой папки. Portable-сборки в других местах не трогаем.
foreach ($app in @(Get-Process -Name 'eve-combat-assistant' -ErrorAction SilentlyContinue)) {
    if (-not [string]::Equals($app.Path, $executable, [System.StringComparison]::OrdinalIgnoreCase)) {
        continue
    }
    $null = $app.CloseMainWindow()
    if (-not $app.WaitForExit(3000)) {
        Stop-Process -Id $app.Id -Force
        if (-not $app.WaitForExit(3000)) { throw 'Application is still running' }
    }
}
