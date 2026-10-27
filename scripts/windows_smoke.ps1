param([string]$Root = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$exe = Join-Path $Root 'target\x86_64-pc-windows-gnu\release\eve-combat-assistant.exe'
$testRoot = Join-Path $env:LOCALAPPDATA 'EVECombatAssistantSmoke'
$evidence = Join-Path $Root 'data\windows-smoke'
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
function Invoke-AppCheck([string[]]$CliArgs, [string]$Name) {
    $stdout = Join-Path $testRoot ($Name + '.out.txt')
    $stderr = Join-Path $testRoot ($Name + '.err.txt')
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $exe
    $info.Arguments = $CliArgs -join ' '
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::new()
    $p.StartInfo = $info
    $null = $p.Start()
    $outRead = $p.StandardOutput.ReadToEndAsync()
    $errRead = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(60000)) { $p.Kill(); throw "$Name timed out" }
    $p.WaitForExit()
    $outRead.Result | Set-Content $stdout -Encoding UTF8
    $errRead.Result | Set-Content $stderr -Encoding UTF8
    if ($p.ExitCode -ne 0) { throw "$Name failed: $($errRead.Result)" }
    return [pscustomobject]@{Name=$Name;ExitCode=$p.ExitCode}
}
$checks = @()
$fixture = Join-Path $Root 'tests\fixtures\target-ocr.png'
$ocrDir = Join-Path $testRoot 'ocr'
$checks += Invoke-AppCheck @('--ocr-test', ('"' + $fixture + '"'), '--data-dir', ('"' + $ocrDir + '"')) 'ocr'
$recognized = [System.IO.File]::ReadAllText((Join-Path $ocrDir 'ocr-result.txt'))
if ($recognized -notmatch 'Rifter' -or $recognized -notmatch 'Haeniken') { throw "OCR text mismatch: $recognized" }
$overlay = Join-Path $Root 'design\overlay-windows.png'
$settings = Join-Path $Root 'design\settings-windows.png'
$uiDir = Join-Path $testRoot 'ui'
$checks += Invoke-AppCheck @('--demo','--data-dir',('"' + $uiDir + '"'),'--screenshot',('"' + $overlay + '"'),'--settings-screenshot',('"' + $settings + '"')) 'ui'
if (-not (Test-Path $overlay) -or -not (Test-Path $settings)) { throw 'Screenshots were not saved' }
$scaleChecks = @()
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$primaryScreen = [System.Windows.Forms.Screen]::PrimaryScreen
$workArea = $primaryScreen.WorkingArea
$screenHeight = $workArea.Height
foreach ($scale in @(75, 175)) {
    $scaleDir = Join-Path $testRoot ('scale-' + $scale)
    New-Item -ItemType Directory -Path $scaleDir -Force | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $scaleDir 'settings.json'), ('{"scale":' + $scale + '}'))
    $scaleImage = Join-Path $scaleDir 'overlay.png'
    $checks += Invoke-AppCheck @('--demo', '--data-dir', ('"' + $scaleDir + '"'), '--screenshot', ('"' + $scaleImage + '"')) ('scale-' + $scale)
    $image = [System.Drawing.Image]::FromFile($scaleImage)
    try {
        if ($image.Height -gt $screenHeight) { throw "Scaled overlay exceeds work area: $($image.Height) > $screenHeight" }
        $scaleChecks += [pscustomobject]@{Scale=$scale;Width=$image.Width;Height=$image.Height}
    } finally { $image.Dispose() }
}
$idleDir = Join-Path $testRoot 'idle'
New-Item -ItemType Directory -Path $idleDir -Force | Out-Null
'{"background":100,"opacity":100,"on_top":true}' | Set-Content (Join-Path $idleDir 'settings.json') -Encoding Ascii
$idleInfo = [System.Diagnostics.ProcessStartInfo]::new()
$idleInfo.FileName = $exe
$idleInfo.Arguments = '--demo --data-dir "' + $idleDir + '"'
$idleInfo.UseShellExecute = $false
$p = [System.Diagnostics.Process]::new()
$p.StartInfo = $idleInfo
$null = $p.Start()
try {
    Start-Sleep -Seconds 2
    $p.Refresh(); $before = $p.TotalProcessorTime.TotalMilliseconds
    Start-Sleep -Seconds 8
    $p.Refresh(); $used = $p.TotalProcessorTime.TotalMilliseconds - $before
    $performance = [pscustomobject]@{WallSeconds=8;CpuMilliseconds=$used;OneCorePercent=($used/8000*100);WorkingSetMiB=[math]::Round($p.WorkingSet64/1MB,1);Responding=$p.Responding}
    if (-not $p.Responding) { throw 'App did not respond during idle check' }
} finally { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
$result = [pscustomobject]@{Checks=$checks;OcrText=$recognized;Idle=$performance;ScaleChecks=$scaleChecks;Overlay=$overlay;Settings=$settings}
$result | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $testRoot 'result.json') -Encoding UTF8
Copy-Item (Join-Path $testRoot '*.txt') -Destination $evidence -Force
Copy-Item (Join-Path $testRoot 'result.json') -Destination $evidence -Force
$result | ConvertTo-Json -Depth 6
