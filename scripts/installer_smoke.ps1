param([string]$Root = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$testRoot = Join-Path $env:LOCALAPPDATA 'EVECombatAssistantInstallerSmoke'
$installDirectory = Join-Path $testRoot 'program'
$dataDirectory = Join-Path $env:LOCALAPPDATA 'EVECombatAssistantInstallerSmokeData'
$registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\EVECombatAssistantSmoke'
$windowTitle = 'EVE Combat Assistant [TEST]'
$evidence = Join-Path $Root 'data\installer-smoke'
New-Item -ItemType Directory -Path $testRoot, $evidence -Force | Out-Null
Copy-Item (Join-Path $Root 'build\EVECombatAssistantSmoke-Setup.exe') (Join-Path $testRoot 'setup.exe') -Force

# Read only windows owned by our test installer. PrintWindow does not capture the desktop.
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class InstallerWindow
{
    public delegate bool WindowCallback(IntPtr window, IntPtr parameter);

    [StructLayout(LayoutKind.Sequential)]
    public struct Rect
    {
        public int Left, Top, Right, Bottom;
    }

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(WindowCallback callback, IntPtr parameter);
    [DllImport("user32.dll")]
    private static extern bool EnumChildWindows(IntPtr parent, WindowCallback callback, IntPtr parameter);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr window, StringBuilder text, int capacity);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetClassName(IntPtr window, StringBuilder text, int capacity);
    [DllImport("user32.dll")]
    private static extern int GetDlgCtrlID(IntPtr window);
    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")]
    private static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")]
    private static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
    [DllImport("user32.dll")]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern IntPtr SendMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    public static string Text(IntPtr window)
    {
        var text = new StringBuilder(1024);
        GetWindowText(window, text, text.Capacity);
        return text.ToString();
    }

    public static IntPtr Dialog(string title)
    {
        IntPtr result = IntPtr.Zero;
        EnumWindows((window, parameter) =>
        {
            var className = new StringBuilder(100);
            GetClassName(window, className, className.Capacity);
            if (IsWindowVisible(window) && className.ToString() == "#32770" && Text(window).Contains(title))
            {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static IntPtr Control(IntPtr parent, int id, string text)
    {
        IntPtr result = IntPtr.Zero;
        EnumChildWindows(parent, (window, parameter) =>
        {
            bool matchesId = id == 0 || GetDlgCtrlID(window) == id;
            bool matchesText = text == null || Text(window).Contains(text);
            if (IsWindowVisible(window) && matchesId && matchesText)
            {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static IntPtr Progress(IntPtr parent)
    {
        IntPtr result = IntPtr.Zero;
        EnumChildWindows(parent, (window, parameter) =>
        {
            var className = new StringBuilder(100);
            GetClassName(window, className, className.Capacity);
            if (IsWindowVisible(window) && className.ToString() == "msctls_progress32")
            {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static void Capture(IntPtr window, string path)
    {
        Rect rect;
        if (!GetClientRect(window, out rect))
        {
            throw new Exception("Window disappeared");
        }
        using (var image = new Bitmap(rect.Right, rect.Bottom))
        {
            using (var graphics = Graphics.FromImage(image))
            {
                IntPtr dc = graphics.GetHdc();
                try
                {
                    if (!PrintWindow(window, dc, 1))
                    {
                        throw new Exception("PrintWindow failed");
                    }
                }
                finally
                {
                    graphics.ReleaseHdc(dc);
                }
            }
            image.Save(path, ImageFormat.Png);
        }
    }
}

'@

function Invoke-TestProcess([string]$Executable, [string]$Arguments) {
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Executable
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $info
    $null = $process.Start()
    return $process
}

function Wait-TestCondition([scriptblock]$Condition, [string]$Description) {
    $deadline = [datetime]::UtcNow.AddSeconds(30)
    while ([datetime]::UtcNow -lt $deadline) {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out: $Description"
}

function Invoke-Next([IntPtr]$Window) {
    $null = [InstallerWindow]::PostMessage($Window, 0x0111, [IntPtr]1, [IntPtr]::Zero)
}

function Close-TestWizard {
    # Depending on page timing, one click opens Finish and the next closes it.
    for ($attempt = 0; $attempt -lt 4; $attempt++) {
        $window = [InstallerWindow]::Dialog($windowTitle)
        if ($window -eq [IntPtr]::Zero) { return }
        Invoke-Next $window
        Start-Sleep -Milliseconds 400
    }
    throw 'Test wizard did not close'
}

function Invoke-TestUninstall([bool]$RemoveData) {
    $null = Invoke-TestProcess (Join-Path $installDirectory 'Uninstall.exe') ''
    Wait-TestCondition { [InstallerWindow]::Dialog($windowTitle) -ne [IntPtr]::Zero } 'uninstaller window'
    $window = [InstallerWindow]::Dialog($windowTitle)
    Invoke-Next $window
    Wait-TestCondition { [InstallerWindow]::Control($window, 0, 'локальную базу') -ne [IntPtr]::Zero } 'data checkbox'
    $checkbox = [InstallerWindow]::Control($window, 0, 'локальную базу')
    if ([InstallerWindow]::SendMessage($checkbox, 0x00F0, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32() -ne 0) {
        throw 'Data removal must default to unchecked'
    }
    if ($RemoveData) {
        $null = [InstallerWindow]::SendMessage($checkbox, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)
    }
    Start-Sleep -Milliseconds 200
    $name = if ($RemoveData) { 'uninstall-delete-data.png' } else { 'uninstall-keep-data.png' }
    [InstallerWindow]::Capture($window, (Join-Path $evidence $name))
    Invoke-Next $window
    Wait-TestCondition { -not (Test-Path $registry) -and -not (Test-Path $installDirectory) } 'uninstall files and registry'
    Close-TestWizard
}

# This build has a separate product ID, registry key, shortcuts and data directory.
# Production data under EVECombatAssistant is never used by this test.
$setup = Invoke-TestProcess (Join-Path $testRoot 'setup.exe') ('/D=' + $installDirectory)
Wait-TestCondition { [InstallerWindow]::Dialog($windowTitle) -ne [IntPtr]::Zero } 'welcome'
$window = [InstallerWindow]::Dialog($windowTitle)
Start-Sleep -Milliseconds 300
[InstallerWindow]::Capture($window, (Join-Path $evidence 'installer-welcome.png'))
Invoke-Next $window
Wait-TestCondition { [InstallerWindow]::Control($window, 1019, $null) -ne [IntPtr]::Zero } 'directory page'
[InstallerWindow]::Capture($window, (Join-Path $evidence 'installer-directory.png'))
Invoke-Next $window
$capturedProgress = $false
$bestProgress = 0.0
$deadline = [datetime]::UtcNow.AddSeconds(30)
while (-not (Test-Path $registry)) {
    if ([datetime]::UtcNow -gt $deadline) { throw 'Installer timed out' }
    $bar = [InstallerWindow]::Progress($window)
    if ($bar -ne [IntPtr]::Zero) {
        $position = [InstallerWindow]::SendMessage($bar, 0x0408, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
        $maximum = [InstallerWindow]::SendMessage($bar, 0x0407, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
        $progress = $position / [math]::Max(1, $maximum)
        if (-not $capturedProgress -or ($progress -gt $bestProgress + 0.1 -and $progress -lt 0.95)) {
            [InstallerWindow]::Capture($window, (Join-Path $evidence 'installer-progress.png'))
            $bestProgress = $progress
            $capturedProgress = $true
        }
    }
    Start-Sleep -Milliseconds 50
}
Close-TestWizard
if (-not $setup.WaitForExit(5000) -or $setup.ExitCode -ne 0) { throw 'Installer failed' }
if ((Get-ItemProperty $registry).InstallLocation -ne $installDirectory) { throw 'Wrong install location' }

$app = Invoke-TestProcess (Join-Path $installDirectory 'eve-combat-assistant.exe') ('--demo --data-dir "' + $dataDirectory + '"')
Wait-TestCondition { (Test-Path (Join-Path $dataDirectory 'combat.sqlite3')) -and $app.Responding } 'installed app'
[System.IO.File]::WriteAllText((Join-Path $dataDirectory 'keep.txt'), 'test data')
Invoke-TestUninstall $false
if (-not $app.WaitForExit(5000)) { throw 'Background process survived uninstall' }
if (-not (Test-Path (Join-Path $dataDirectory 'combat.sqlite3'))) { throw 'Keep-data option lost the database' }

# Reinstall and exercise the same checkbox with data removal enabled.
$setup = Invoke-TestProcess (Join-Path $testRoot 'setup.exe') ('/S /D=' + $installDirectory)
if (-not $setup.WaitForExit(30000) -or $setup.ExitCode -ne 0) { throw 'Silent reinstall failed' }
Invoke-TestUninstall $true
if (Test-Path $dataDirectory) { throw 'Purge-data option left application data' }
$shortcuts = Join-Path ([Environment]::GetFolderPath('Programs')) 'EVECombatAssistantSmoke'
if (Test-Path $shortcuts) { throw 'Uninstall left Start Menu shortcuts' }

$result = [pscustomobject]@{
    Installation = 'pass'
    RegistryAndShortcuts = 'pass'
    InstalledAppLaunch = 'pass'
    BackgroundProcessStopped = 'pass'
    KeepDatabase = 'pass'
    DeleteDatabase = 'pass'
    ProgressCaptured = $capturedProgress
}
$result | ConvertTo-Json | Set-Content (Join-Path $evidence 'result.json') -Encoding UTF8
$result | ConvertTo-Json
