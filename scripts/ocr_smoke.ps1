param([string]$Root = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$exe = Join-Path $Root 'target\x86_64-pc-windows-gnu\release\eve-combat-assistant.exe'
$testRoot = Join-Path $env:LOCALAPPDATA 'EVECombatAssistantOcrDebugSmoke'
$evidence = Join-Path $Root 'data\ocr-debug-smoke'
New-Item -ItemType Directory -Path $testRoot, $evidence -Force | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class FitSmokeWindow {
    [DllImport("user32.dll")]
    public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("gdi32.dll")]
    private static extern uint GetPixel(IntPtr dc, int x, int y);
    [DllImport("user32.dll")]
    private static extern bool GetWindowRect(IntPtr window, out Rect rect);
    public static Rect WindowBounds(IntPtr window) { Rect rect; GetWindowRect(window,out rect); return rect; }
    public static uint SampleScreen(int x, int y) {
        IntPtr dc=GetDC(IntPtr.Zero);
        try { return GetPixel(dc,x,y); } finally { ReleaseDC(IntPtr.Zero,dc); }
    }
    public static IntPtr FindByTitle(int process, string prefix) {
        IntPtr result=IntPtr.Zero;
        EnumWindows((window,parameter)=>{
            uint owner; GetWindowThreadProcessId(window,out owner);
            var title=new System.Text.StringBuilder(256); GetWindowText(window,title,title.Capacity);
            if(owner==process && title.ToString().StartsWith(prefix)){result=window;return false;}
            return true;
        },IntPtr.Zero);
        return result;
    }
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

$null = [FitSmokeWindow]::SetThreadDpiAwarenessContext([IntPtr](-4))

function Wait-Smoke([int]$Milliseconds) {
    $until = [datetime]::UtcNow.AddMilliseconds($Milliseconds)
    do { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 50 } while ([datetime]::UtcNow -lt $until)
}
function Read-SmokeDocument([IntPtr]$Window, [string]$Name) {
    $path = Join-Path $evidence ($Name + '.png')
    [FitSmokeWindow]::Capture($Window, $path)
    $outDir = Join-Path $testRoot ('read-' + $Name)
    $reader = Invoke-TestApp ('--ocr-test "' + $path + '" --ocr-language rus --data-dir "' + $outDir + '"')
    if (-not $reader.WaitForExit(20000)) { $reader.Kill(); throw 'UI OCR timed out' }
    if ($reader.ExitCode -ne 0) { throw 'UI OCR failed' }
    return Get-Content (Join-Path $outDir 'ocr-result.json') -Encoding UTF8 -Raw | ConvertFrom-Json
}
function Invoke-SmokeTextClick([IntPtr]$Window, [string]$Pattern, [string]$Name) {
    for ($attempt = 0; $attempt -lt 3; $attempt++) {
        $document = Read-SmokeDocument $Window ($Name + '-' + $attempt)
        $line = $document.lines | Where-Object { $_.text -match $Pattern } | Select-Object -First 1
        if ($null -ne $line) {
            # WinRT OCR увеличивает снимок шириной <2000 вдвое.
            $word = $line.words[0]
            $x = [int](($word.x + $word.width / 2) / 2)
            $y = [int](($word.y + $word.height / 2) / 2)
            $point = New-Object FitSmokeWindow+Point
            $point.X = $x; $point.Y = $y
            $null = [FitSmokeWindow]::ClientToScreen($Window, [ref]$point)
            $null = [FitSmokeWindow]::SetForegroundWindow($Window)
            $null = [FitSmokeWindow]::SetCursorPos($point.X, $point.Y)
            Wait-Smoke 100
            [FitSmokeWindow]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
            Wait-Smoke 100
            [FitSmokeWindow]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
            Wait-Smoke 300
            return
        }
        $point = New-Object FitSmokeWindow+Point
        $point.X = 450; $point.Y = 500
        $null = [FitSmokeWindow]::ClientToScreen($Window, [ref]$point)
        $null = [FitSmokeWindow]::SetCursorPos($point.X, $point.Y)
        [FitSmokeWindow]::mouse_event(0x0800, 0, 0, [uint32]::MaxValue - 479, [UIntPtr]::Zero)
        Wait-Smoke 300
    }
    throw "UI label not found: $Pattern"
}

$cardsPath = Join-Path $evidence 'cards.png'
Invoke-CardFixture $cardsPath $false
$form = New-Object System.Windows.Forms.Form
$form.Text = 'EVE OCR Smoke Fixture'
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point 30, 40
$form.ClientSize = New-Object System.Drawing.Size 1400, 460
$form.BackColor = [System.Drawing.Color]::FromArgb(10, 15, 20)
$cardsBox = New-Object System.Windows.Forms.PictureBox
$cardsBox.Location = New-Object System.Drawing.Point 0, 0
$cardsBox.Size = New-Object System.Drawing.Size 1050, 250
$cardsBox.Image = [System.Drawing.Image]::FromFile($cardsPath)
$form.Controls.Add($cardsBox)
$selected = New-Object System.Drawing.Bitmap 260, 110
$graphics = [System.Drawing.Graphics]::FromImage($selected)
$font = New-Object System.Drawing.Font 'Arial', 12
$graphics.Clear($form.BackColor)
$graphics.DrawString('Pilot9', $font, [System.Drawing.Brushes]::White, 20, 15)
$graphics.DrawString('38 km', $font, [System.Drawing.Brushes]::White, 20, 50)
$graphics.Dispose(); $font.Dispose()
$selectedBox = New-Object System.Windows.Forms.PictureBox
$selectedBox.Location = New-Object System.Drawing.Point 1080, 0
$selectedBox.Size = New-Object System.Drawing.Size 260, 110
$selectedBox.Image = $selected
$form.Controls.Add($selectedBox)
$marker=New-Object System.Windows.Forms.Panel
$marker.Location=New-Object System.Drawing.Point 100,300
$marker.Size=New-Object System.Drawing.Size 600,130
$marker.BackColor=[System.Drawing.Color]::FromArgb(40,180,220)
$form.Controls.Add($marker)
$results = @()
try {
    $form.Show()
    Wait-Smoke 500
    $origin = New-Object FitSmokeWindow+Point
    $origin.X = 0; $origin.Y = 0
    $null = [FitSmokeWindow]::ClientToScreen($form.Handle, [ref]$origin)
    foreach ($mode in @('selector-175', 'selector-75-cancel', 'draft-debug', 'capture-failure', 'debug-off')) {
        $null = [FitSmokeWindow]::SetForegroundWindow($form.Handle)
        Wait-Smoke 250
        $directory = Join-Path $testRoot ($mode + '-' + [datetime]::UtcNow.ToString('yyyyMMddHHmmssfff'))
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
        $region = if ($mode -eq 'capture-failure') { @(0, 0, 0, 0) } else { @(($origin.X + 1080), $origin.Y, 260, 110) }
        $selectorMode=$mode.StartsWith('selector')
        $scale=if($mode -eq 'selector-175'){175}elseif($mode -eq 'selector-75-cancel'){75}else{100}
        $ownFit=if($selectorMode){''}else{'INVALID FIT FOR MANUAL CHECK'}
        $config = @{
            ocr = $false; ocr_debug = ($mode -eq 'capture-failure' -or $selectorMode); auto_pilot_lookup = $false; pause_outside_eve = $true
            own_fit = $ownFit; scale = $scale; region = $region
            targets_region = @($origin.X, $origin.Y, 1050, 250); language = 'eng'
        }
        # Settings loader expects UTF-8 without BOM.
        $json = $config | ConvertTo-Json
        [System.IO.File]::WriteAllText((Join-Path $directory 'settings.json'), $json, (New-Object System.Text.UTF8Encoding $false))
        $process = Invoke-TestApp ('--open-settings --settings-tab 3 --data-dir "' + $directory + '"')
        try {
            $deadline = [datetime]::UtcNow.AddSeconds(20)
            do {
                $window = [FitSmokeWindow]::FindSettings($process.Id)
                if ($window -ne [IntPtr]::Zero) { break }
                Wait-Smoke 100
            } while ([datetime]::UtcNow -lt $deadline)
            if ($window -eq [IntPtr]::Zero) { throw 'Settings window missing' }
            $null = [FitSmokeWindow]::SetWindowPos($window, [IntPtr]::Zero, 250, 70, 900, 940, 0x0040)
            Wait-Smoke 600
            if($selectorMode){
                $rootWindow=[FitSmokeWindow]::FindByTitle($process.Id,'EVE Combat Assistant')
                $beforeRoot=[FitSmokeWindow]::WindowBounds($rootWindow)
                Invoke-SmokeTextClick $window 'панель.*текущ' ($mode+'-open')
                $deadline=[datetime]::UtcNow.AddSeconds(15)
                do { Wait-Smoke 150; $selector=[FitSmokeWindow]::FindByTitle($process.Id,'Выделение области') } while($selector -eq [IntPtr]::Zero -and [datetime]::UtcNow -lt $deadline -and -not $process.HasExited)
                if($selector -eq [IntPtr]::Zero){throw 'Selector did not open'}
                Wait-Smoke 350
                $color=[FitSmokeWindow]::SampleScreen(($origin.X+250),($origin.Y+380))
                $green=($color -shr 8) -band 255; $blue=($color -shr 16) -band 255
                if($green -lt 100 -or $blue -lt 130){throw ('Selector background is black or misplaced: color='+$color)}
                if($mode -eq 'selector-75-cancel'){
                    $null=[FitSmokeWindow]::SetForegroundWindow($selector)
                    Wait-Smoke 100
                    [System.Windows.Forms.SendKeys]::SendWait('{ESC}')
                    $expected=$region
                }else{
                    $expected=@(($origin.X+120),($origin.Y+330),250,80)
                    $null=[FitSmokeWindow]::SetForegroundWindow($selector)
                    $null=[FitSmokeWindow]::SetCursorPos($expected[0],$expected[1])
                    Wait-Smoke 120
                    [FitSmokeWindow]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
                    Wait-Smoke 100
                    for($step=1;$step -le 10;$step++){
                        $null=[FitSmokeWindow]::SetCursorPos(($expected[0]+[int]($expected[2]*$step/10)),($expected[1]+[int]($expected[3]*$step/10)))
                        Wait-Smoke 40
                    }
                    [FitSmokeWindow]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
                }
                Wait-Smoke 700
                if([FitSmokeWindow]::FindByTitle($process.Id,'Выделение области') -ne [IntPtr]::Zero){throw 'Selector did not close'}
                $afterRoot=[FitSmokeWindow]::WindowBounds($rootWindow)
                foreach($edge in @('Left','Top','Right','Bottom')){if([math]::Abs($afterRoot.$edge-$beforeRoot.$edge) -gt 3){throw 'Overlay geometry did not restore'}}
                $window=[FitSmokeWindow]::FindSettings($process.Id)
                if($window -eq [IntPtr]::Zero){throw 'Settings did not return after region selection'}
                $null=[FitSmokeWindow]::SetWindowPos($window,[IntPtr]::Zero,250,70,900,940,0x0040)
                Wait-Smoke 300
                Invoke-SmokeTextClick $window 'Сохран' ($mode+'-save')
                Wait-Smoke 500
                $saved=Get-Content (Join-Path $directory 'settings.json') -Raw -Encoding UTF8 | ConvertFrom-Json
                for($coordinate=0;$coordinate -lt 4;$coordinate++){
                    if([math]::Abs($saved.region[$coordinate]-$expected[$coordinate]) -gt 2){throw ('Selected pixel rectangle differs: '+($saved.region -join ',')+' expected '+($expected -join ','))}
                }
                $results+=[pscustomobject]@{mode=$mode;background='visible';region=$saved.region;restored=$true}
                continue
            }
            if ($mode -eq 'draft-debug') { Invoke-SmokeTextClick $window 'Расширенное' ($mode + '-checkbox') }
            Invoke-SmokeTextClick $window 'Проверить области' ($mode + '-check')
            Wait-Smoke 2200
            $window = [FitSmokeWindow]::FindSettings($process.Id)
            if ($window -eq [IntPtr]::Zero -or $process.HasExited -or -not $process.Responding) { throw 'Settings did not return after capture' }
            $null = [FitSmokeWindow]::SetWindowPos($window, [IntPtr]::Zero, 250, 70, 900, 940, 0x0040)
            Wait-Smoke 300
            $preview = Read-SmokeDocument $window ($mode + '-preview')
            $text = ($preview.lines | ForEach-Object { $_.text }) -join "`n"
            $debug = Join-Path $directory 'debug'
            if ($mode -eq 'debug-off') {
                if (Test-Path $debug) { throw 'Debug files exist while checkbox is off' }
            } else {
                $records = Get-Content (Join-Path $debug 'ocr.jsonl') -Encoding UTF8 | ForEach-Object { $_ | ConvertFrom-Json }
                $check = $records | Where-Object { $_.event.kind -eq 'manual_check' } | Select-Object -Last 1
                if ($null -eq $check) { throw 'Manual diagnostic record missing' }
                if ($mode -eq 'draft-debug') {
                    if ($check.event.result.Ok.ship -ne 'Talwar' -or $check.event.result.Ok.pilot -ne 'Pilot9') { throw 'Selected target mismatch' }
                    if (-not (Test-Path (Join-Path $debug $check.event.selected_png)) -or -not (Test-Path (Join-Path $debug $check.event.cards_png))) { throw 'Diagnostic screenshots missing' }
                    if ($check.event.selected_text -notmatch 'Pilot9') { throw 'Capture contains assistant window instead of selected pilot' }
                    if ($text -notmatch 'Найден') { throw 'Preview result missing' }
                } else {
                    if ($null -eq $check.event.result.Err -or $null -eq $check.event.selected_error) { throw 'Capture error not reported' }
                }
            }
            $results += [pscustomobject]@{mode=$mode; restored=$true; diagnostic=$true}
        } finally { if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() } }
    }
} finally {
    $form.Close(); $form.Dispose(); $cardsBox.Image.Dispose(); $selected.Dispose()
}
$results | ConvertTo-Json | Set-Content (Join-Path $evidence 'results.json') -Encoding UTF8
Write-Output 'PASS: visible selection background, zoom 75/175, physical rectangle and Escape; draft check, preview, debug PNG/JSONL, failure restore, logging disabled'
