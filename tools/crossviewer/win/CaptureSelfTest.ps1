<#
.SYNOPSIS
  Proves a capture method actually captures a GPU-composited window's real content, on a
  throwaway window this script creates and owns - before any of that method's output is
  trusted for a real Acrobat/Revu diagnostic run.

.DESCRIPTION
  Built alongside the Acrobat leg's new PrintWindow/Wgc capture methods (Capture.ps1,
  WgcCapture.ps1) precisely because "the capture looked fine structurally" has already been
  wrong once on this harness (Test-PageVisible's own header describes a capture that passed
  every structural check yet photographed Acrobat's Home screen, not a page) - a method
  needs to be shown working on a KNOWN target before it is trusted on an unknown one.

  Target window: a throwaway, isolated Microsoft Edge window navigated to a solid-colour
  data: URL. Edge/Chromium is, like Acrobat, a GPU-accelerated compositor - it reproduces
  the same class of surface (DirectComposition / hardware-overlay) that BitBlt-based
  capture is documented to fail against, which is exactly the property this self-test needs
  to exercise. A plain WinForms window would NOT do this: GDI-composited surfaces already
  capture fine via CopyFromScreen (proven for Bluebeam Revu), so a GDI target would not
  distinguish a working PrintWindow/Wgc implementation from a broken one.

  For each -Method requested, captures the window, then checks TWO things, not one:
    1. non_blank  - Test-BitmapNonBlank (any content at all, not literally black)
    2. color_match - the centre of the frame is close to the known colour the page was
       given. This is the stronger, real assertion: a method could produce SOME non-black
       noise (a partial/garbled frame) and pass (1) while still being useless. Only a
       method that passes BOTH is evidence it works.

  Never force-kills the throwaway Edge window or process - WM_CLOSE via
  Close-WindowPolitely only, same discipline as every other window this harness touches,
  even though this one is disposable. An isolated -UserDataDir means the window this script
  creates cannot be an owner's pre-existing Edge session; it is still only ever addressed by
  its own HWND, never by name/class match against "any Edge window", so a pre-existing
  owner Edge window is never at risk.

  MUST run in Session 1 (same constraint as every other leg - see AcrobatLeg.ps1's header).
  Registration: Register-CrossviewerTask.ps1 -Enable (registration alone leaves tasks
  Disabled by default now - see that script's header for why).

.EXAMPLE
  PsGuiHost.exe CaptureSelfTest.ps1 selftest.log -OutputDir 'H:\redline-crossviewer\out\selftest'
#>
[CmdletBinding()]
param(
    [string]$OutputDir = 'H:\redline-crossviewer\out\selftest',
    [ValidateSet('CopyFromScreen', 'PrintWindow', 'Wgc')]
    [string[]]$Methods = @('CopyFromScreen', 'PrintWindow', 'Wgc'),
    [string]$TargetDevice = '\\.\DISPLAY1',
    [int]$PreferWidth  = 3840,
    [int]$PreferHeight = 2160,
    [int]$WindowSettleSec = 5,
    [string]$EdgeExe = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe',
    # A colour unlikely to occur by accident in any real UI chrome, so a match is real
    # evidence rather than a coincidence.
    [byte]$ColorR = 0x1F,
    [byte]$ColorG = 0xCE,
    [byte]$ColorB = 0x8A,
    [int]$ColorTolerance = 24
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

. (Join-Path $PSScriptRoot 'Displays.ps1')
. (Join-Path $PSScriptRoot 'Capture.ps1')
if ($Methods -contains 'Wgc') { . (Join-Path $PSScriptRoot 'WgcCapture.ps1') }

function Write-Log {
    param([string]$Message)
    $stamp = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss')
    Write-Output "[$stamp] [selftest] $Message"
}

function Test-CaptureColorMatch {
    <#
      Samples a small centre patch (avoids Edge's own address-bar chrome near the top edge)
      and checks it is close to the expected solid colour within -Tolerance per channel.
      Deliberately a patch average, not a single pixel - a single pixel can land on
      anti-aliasing or a stray compositor artefact even in a genuinely correct capture.
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte]$R,
        [Parameter(Mandatory = $true)][byte]$G,
        [Parameter(Mandatory = $true)][byte]$B,
        [int]$Tolerance = 24
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{ matched = $false; sampled_r = $null; sampled_g = $null; sampled_b = $null; error = 'no capture file' }
    }
    $bmp = $null
    try {
        $bmp = New-Object System.Drawing.Bitmap $Path
        $cx = [int]($bmp.Width * 0.5); $cy = [int]($bmp.Height * 0.6)  # below any top chrome
        $half = 20
        $sr = 0L; $sg = 0L; $sb = 0L; $n = 0
        for ($y = [Math]::Max(0, $cy - $half); $y -lt [Math]::Min($bmp.Height, $cy + $half); $y += 4) {
            for ($x = [Math]::Max(0, $cx - $half); $x -lt [Math]::Min($bmp.Width, $cx + $half); $x += 4) {
                $c = $bmp.GetPixel($x, $y)
                $sr += $c.R; $sg += $c.G; $sb += $c.B; $n++
            }
        }
        if ($n -eq 0) { return [pscustomobject]@{ matched = $false; sampled_r = $null; sampled_g = $null; sampled_b = $null; error = 'empty sample region' } }
        $ar = [int]($sr / $n); $ag = [int]($sg / $n); $ab = [int]($sb / $n)
        $matched = ([Math]::Abs($ar - [int]$R) -le $Tolerance) -and
                   ([Math]::Abs($ag - [int]$G) -le $Tolerance) -and
                   ([Math]::Abs($ab - [int]$B) -le $Tolerance)
        return [pscustomobject]@{ matched = $matched; sampled_r = $ar; sampled_g = $ag; sampled_b = $ab; error = $null }
    } catch {
        return [pscustomobject]@{ matched = $false; sampled_r = $null; sampled_g = $null; sampled_b = $null; error = $_.Exception.Message }
    } finally {
        if ($null -ne $bmp) { $bmp.Dispose() }
    }
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
Enable-DpiAwareness

$hex = '{0:X2}{1:X2}{2:X2}' -f $ColorR, $ColorG, $ColorB
$html = "<html><body style='background:%23$hex;margin:0;height:100vh'></body></html>"
$dataUrl = "data:text/html,$html"

if (-not (Test-Path -LiteralPath $EdgeExe)) {
    $x86 = 'C:\Program Files\Microsoft\Edge\Application\msedge.exe'
    if (Test-Path -LiteralPath $x86) { $EdgeExe = $x86 }
}
if (-not (Test-Path -LiteralPath $EdgeExe)) { throw "msedge.exe not found (checked $EdgeExe and the x64 Program Files path)" }

# Isolated profile so this NEVER attaches to (or closes) an owner Edge session via
# Chromium's single-instance IPC handoff - see this file's header.
$profileDir = Join-Path $env:TEMP ("crossviewer-selftest-{0:yyyyMMdd-HHmmss}" -f (Get-Date))
New-Item -ItemType Directory -Force -Path $profileDir | Out-Null

Write-Log "launching Edge (isolated profile) with a solid #$hex page"
$proc = Start-Process -FilePath $EdgeExe -PassThru -ArgumentList @(
    '--new-window', '--no-first-run', '--no-default-browser-check',
    "--user-data-dir=`"$profileDir`"", $dataUrl
)

$result = [ordered]@{
    engine        = 'selftest'
    target_color  = "#$hex"
    machine       = $env:COMPUTERNAME
    started_at    = (Get-Date).ToString('o')
    display       = $null
    window_found  = $false
    methods       = @()
}

# Declared before the try so the finally block (which always runs, even if display
# selection itself throws) can safely reference it under Set-StrictMode.
$hwnd = [IntPtr]::Zero

try {
    $displays = Get-CrossviewerDisplays
    $target = Select-TargetDisplay -Displays $displays.displays -PreferWidth $PreferWidth -PreferHeight $PreferHeight -TargetDevice $TargetDevice
    $result.display = $target

    # Own-PID scoped: only ever looks at windows owned by the process THIS script started.
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline -and $hwnd -eq [IntPtr]::Zero) {
        Start-Sleep -Milliseconds 500
        $candidates = @(Get-ProcessWindows -ProcessId $proc.Id | Where-Object { $_.ClassName -eq 'Chrome_WidgetWin_1' })
        if ($candidates.Count -gt 0) { $hwnd = ($candidates | Sort-Object -Property Width -Descending)[0].Handle }
    }
    if ($hwnd -eq [IntPtr]::Zero) {
        throw "no Chrome_WidgetWin_1 window appeared for pid $($proc.Id) within 20s"
    }
    $result.window_found = $true
    Write-Log "found self-test window pid=$($proc.Id) hwnd=$hwnd"

    Move-WindowToDisplay -WindowHandle $hwnd -Display $target | Out-Null
    Set-WindowForeground -WindowHandle $hwnd
    Start-Sleep -Seconds $WindowSettleSec

    $onTop = Test-WindowUnobstructed -WindowHandle $hwnd
    if (-not $onTop.unobstructed) {
        throw "self-test window is obscured on $($target.device) - $($onTop.hits)/$($onTop.total) sample points ours, blocker $($onTop.blocker). Refusing to run capture methods against pixels that would not be ours."
    }
    Write-Log "window verified on top ($($onTop.hits)/$($onTop.total) sample points)"

    foreach ($m in $Methods) {
        $entry = [ordered]@{
            method_requested = $m
            method_used      = $null
            captured         = $false
            non_blank        = $false
            color_match      = $false
            sampled_rgb      = $null
            path             = $null
            error            = $null
        }
        try {
            $p = Join-Path $OutputDir "selftest-$m.png"
            $cap = Save-WindowCapture -WindowHandle $hwnd -Path $p -Foreground -VerifyOnTop -Method $m
            if (-not $cap) {
                $entry.error = 'Save-WindowCapture returned null (method failed or window became obscured)'
            } else {
                $entry.captured = $true
                $entry.method_used = $cap.method
                $entry.path = $cap.path
                $entry.non_blank = Test-BitmapNonBlank -Path $cap.path
                $color = Test-CaptureColorMatch -Path $cap.path -R $ColorR -G $ColorG -B $ColorB -Tolerance $ColorTolerance
                $entry.color_match = $color.matched
                $entry.sampled_rgb = if ($null -ne $color.sampled_r) { "$($color.sampled_r),$($color.sampled_g),$($color.sampled_b)" } else { $null }
                if ($color.error) { $entry.error = $color.error }
            }
        } catch {
            $entry.error = $_.Exception.Message
        }
        $verdict = if ($entry.non_blank -and $entry.color_match) { 'PASS' } else { 'FAIL' }
        Write-Log "$m -> $verdict (used=$($entry.method_used) non_blank=$($entry.non_blank) color_match=$($entry.color_match) sampled=$($entry.sampled_rgb) error=$($entry.error))"
        $result.methods += [pscustomobject]$entry
    }
} finally {
    if ($hwnd -ne [IntPtr]::Zero) {
        $close = Close-WindowPolitely -WindowHandle $hwnd -ProcessId $proc.Id -TimeoutSec 15
        Write-Log "close result: closed=$($close.closed) blocked_by_dialog=$($close.blocked_by_dialog)"
        if (-not $close.closed) {
            Write-Log "self-test window did not close within 15s (pid $($proc.Id)) - left running, never force-killed; manual cleanup: close the Edge window titled about the self-test data: URL"
        }
    }
    # Best-effort profile cleanup - never blocks or throws on failure (e.g. Edge still
    # holding a lock on it because WM_CLOSE did not finish).
    try { Remove-Item -LiteralPath $profileDir -Recurse -Force -ErrorAction SilentlyContinue } catch { }
}

$result.finished_at = (Get-Date).ToString('o')
$passCount = @($result.methods | Where-Object { $_.non_blank -and $_.color_match }).Count
Write-Log "summary: $passCount/$($Methods.Count) methods PASS"

$jsonPath = Join-Path $OutputDir 'selftest-results.json'
$json = $result | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($jsonPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Log "wrote $jsonPath"
