param([string]$Root = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$exe = Join-Path $Root 'target\x86_64-pc-windows-gnu\release\eve-combat-assistant.exe'
$testRoot = Join-Path $env:LOCALAPPDATA 'EVECombatAssistantFitSmoke'
$evidence = Join-Path $Root 'data\fit-smoke'
New-Item -ItemType Directory -Path $testRoot, $evidence -Force | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class FitSmokeWindow {
    public delegate bool WindowCallback(IntPtr window, IntPtr parameter);
    [StructLayout(LayoutKind.Sequential)]
    public struct Point { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")]
    private static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")]
    private static extern IntPtr GetDC(IntPtr window);
    [DllImport("user32.dll")]
    private static extern int ReleaseDC(IntPtr window, IntPtr dc);
    [DllImport("gdi32.dll")]
    private static extern bool BitBlt(IntPtr target, int x, int y, int width, int height, IntPtr source, int sx, int sy, uint flags);
    public static void Capture(IntPtr window, string path) {
        Rect rect; GetClientRect(window, out rect);
        using (var image = new System.Drawing.Bitmap(rect.Right, rect.Bottom)) {
            using (var graphics = System.Drawing.Graphics.FromImage(image)) {
                IntPtr target = graphics.GetHdc(); IntPtr source = GetDC(window);
                try { BitBlt(target,0,0,rect.Right,rect.Bottom,source,0,0,0x00CC0020); }
                finally { ReleaseDC(window,source); graphics.ReleaseHdc(target); }
            }
            image.Save(path,System.Drawing.Imaging.ImageFormat.Png);
        }
    }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")]
    private static extern bool EnumWindows(WindowCallback callback, IntPtr parameter);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, System.Text.StringBuilder title, int size);
    public static IntPtr FindSettings(int process) {
        IntPtr result = IntPtr.Zero;
        EnumWindows((window, parameter) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            var title = new System.Text.StringBuilder(256);
            GetWindowText(window, title, title.Capacity);
            if (owner == process && title.ToString().StartsWith("Настройки")) {
                result = window; return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    public static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
}
'@

function Invoke-TestApp([string]$Arguments) {
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $exe
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    return [System.Diagnostics.Process]::Start($info)
}
function Invoke-ClientClick([IntPtr]$Window, [int]$X, [int]$Y) {
    $point = New-Object FitSmokeWindow+Point
    $point.X = $X; $point.Y = $Y
    $null = [FitSmokeWindow]::ClientToScreen($Window, [ref]$point)
    $null = [FitSmokeWindow]::SetForegroundWindow($Window)
    $null = [FitSmokeWindow]::SetCursorPos($point.X, $point.Y)
    [FitSmokeWindow]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    [FitSmokeWindow]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    $position = [IntPtr](($Y -shl 16) -bor $X)
    $null = [FitSmokeWindow]::PostMessage($Window, 0x0200, [IntPtr]::Zero, $position)
    $null = [FitSmokeWindow]::PostMessage($Window, 0x0201, [IntPtr]1, $position)
    Start-Sleep -Milliseconds 100
    $null = [FitSmokeWindow]::PostMessage($Window, 0x0202, [IntPtr]::Zero, $position)
    Start-Sleep -Milliseconds 150
}
function Invoke-CardFixture([string]$Path, [bool]$Reorder) {
    $bitmap = New-Object System.Drawing.Bitmap 1050, 250
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $font = New-Object System.Drawing.Font 'Arial', 12
    $brush = [System.Drawing.Brushes]::White
    try {
        $graphics.Clear([System.Drawing.Color]::FromArgb(10, 15, 20))
        for ($index = 0; $index -lt 10; $index++) {
            $pilot = if ($Reorder) { ($index + 3) % 10 } else { $index }
            $x = [single](($index % 5) * 210 + 50)
            $y = [single]([math]::Floor($index / 5) * 120 + 10)
            $graphics.DrawString(('Pilot' + $pilot), $font, $brush, $x, $y)
            $graphics.DrawString('[CORP]', $font, $brush, $x, ($y + 24))
            $hull = if ($pilot -eq 9) { 'Talwar*' } else { 'Rifter*' }
            $graphics.DrawString($hull, $font, $brush, $x, ($y + 48))
            $graphics.DrawString('38 km', $font, $brush, $x, ($y + 72))
        }
        $bitmap.Save($Path)
    } finally { $font.Dispose(); $graphics.Dispose(); $bitmap.Dispose() }
}

$fit = [System.IO.File]::ReadAllText((Join-Path $Root 'tests\fixtures\talwar.eft'))
$original = [System.Windows.Forms.Clipboard]::GetDataObject()
$backup = New-Object System.Windows.Forms.DataObject
if ($null -ne $original) {
    foreach ($format in $original.GetFormats($false)) {
        $value = $original.GetData($format, $false)
        if ($null -ne $value) { $backup.SetData($format, $false, $value) }
    }
}
$pasteResults = @()
try {
    foreach ($mode in @('button')) {
        $directory = Join-Path $testRoot $mode
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $directory 'settings.json'), '{"auto_pilot_lookup":false,"scale":100}')
        $process = Invoke-TestApp ('--open-settings --settings-tab 1 --data-dir "' + $directory + '"')
        try {
            $deadline = [datetime]::UtcNow.AddSeconds(20)
            do {
                $window = [FitSmokeWindow]::FindSettings($process.Id)
                if ($window -ne [IntPtr]::Zero) { break }
                Start-Sleep -Milliseconds 100
            } while ([datetime]::UtcNow -lt $deadline)
            if ($window -eq [IntPtr]::Zero) { throw 'Settings window was not created' }
            Start-Sleep -Milliseconds 1000
            # clip.exe keeps CF_UNICODETEXT in the system, without a blocked OLE owner.
            $clipInfo = [System.Diagnostics.ProcessStartInfo]::new()
            $clipInfo.FileName = Join-Path $env:SystemRoot 'System32\clip.exe'
            $clipInfo.UseShellExecute = $false
            $clipInfo.RedirectStandardInput = $true
            $clip = [System.Diagnostics.Process]::Start($clipInfo)
            $bytes = [System.Text.Encoding]::Unicode.GetBytes(([char]0xfeff + $fit))
            $clip.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
            $clip.StandardInput.BaseStream.Close()
            $clip.WaitForExit()
            if ($mode -eq 'shortcut') {
                Invoke-ClientClick $window 100 155
                Write-Output ('Shortcut window has focus: ' + ([FitSmokeWindow]::GetForegroundWindow() -eq $window))
                [System.Windows.Forms.SendKeys]::SendWait('^v')
            } else { Invoke-ClientClick $window 95 365 }
            Start-Sleep -Milliseconds 500
            [FitSmokeWindow]::Capture($window, (Join-Path $evidence ($mode + '-after-paste.png')))
            Invoke-ClientClick $window 65 588
            # Первый dogma-расчёт выполняется до записи настроек; ждём результат.
            $deadline = [datetime]::UtcNow.AddSeconds(15)
            do {
                $settings = Get-Content -Raw -Encoding UTF8 (Join-Path $directory 'settings.json') | ConvertFrom-Json
                $fitMatches = ([string]$settings.own_fit).TrimStart([char]0xfeff).Trim().Replace("`r`n", "`n") -eq $fit.Trim().Replace("`r`n", "`n")
                if ($fitMatches) { break }
                Start-Sleep -Milliseconds 100
            } while ([datetime]::UtcNow -lt $deadline)
            if (-not $fitMatches) { throw "Clipboard import failed: $mode" }
            $pasteResults += $mode
        } finally {
            if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
        }
    }
} finally {
    for ($attempt=0; $attempt -lt 5; $attempt++) {
        try { [System.Windows.Forms.Clipboard]::SetDataObject($backup, $true); break }
        catch { if ($attempt -eq 4) { Write-Warning 'Clipboard restore failed' }; Start-Sleep -Milliseconds 100 }
    }
}

$selected = Join-Path $testRoot 'selected.png'
$bitmap = New-Object System.Drawing.Bitmap 350, 100
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$font = New-Object System.Drawing.Font 'Arial', 14
try {
    $graphics.Clear([System.Drawing.Color]::Black)
    $graphics.DrawString("Pilot9`n38 km", $font, [System.Drawing.Brushes]::White, 10, 10)
    $bitmap.Save($selected)
} finally { $font.Dispose(); $graphics.Dispose(); $bitmap.Dispose() }
$ocrResults = @()
foreach ($reorder in @($false, $true)) {
    $cards = Join-Path $testRoot ('cards-' + $reorder + '.png')
    Invoke-CardFixture $cards $reorder
    $process = Invoke-TestApp ('--target-test "' + $selected + '" "' + $cards + '" --data-dir "' + $testRoot + '"')
    if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Target OCR timed out' }
    if ($process.ExitCode -ne 0) { throw 'Target OCR failed' }
    $result = Get-Content -Raw -Encoding UTF8 (Join-Path $testRoot 'target-result.json') | ConvertFrom-Json
    if ($result.target.pilot -ne 'Pilot9' -or $result.target.ship -ne 'Talwar') { throw 'Wrong selected target in ten cards' }
    $ocrResults += $result
}
[pscustomobject]@{Clipboard=$pasteResults;TenTargetOcr=$ocrResults} | ConvertTo-Json -Depth 8 |
    Set-Content (Join-Path $evidence 'result.json') -Encoding UTF8
Write-Output 'PASS: paste button, ten targets in two rows, reordered targets'
