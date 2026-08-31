<#
.SYNOPSIS
  Bluebeam Revu leg of the cross-viewer harness, driven through the GUI.

.DESCRIPTION
  Opens every staged PDF in a real Bluebeam Revu window on the landscape display, waits for
  the page to finish rendering, screenshots the window, and closes Revu politely. The
  screenshots feed the same vision-review pass as the Acrobat renders.

  WHY THE GUI AND NOT THE SCRIPT ENGINE: Revu's Script Engine is gated behind Revu's top
  subscription tier. On this machine ScriptEngine.exe 21.10.0.19316 exits -4 with
  "This feature requires a maximum subscription level" for ANY invocation including /?, so
  no script content is ever reached and better script code cannot help. Owner decision
  2026-08-29 was to build the GUI fallback rather than buy the upgrade. BluebeamLeg.ps1
  keeps the licence probe; this script is the leg that actually produces renders.

  WHAT THIS LEG CAN AND CANNOT PROVE. Revu regenerates markup appearances from the
  annotation dictionary rather than blitting our stored /AP, so a page that looks right in
  a real Revu window is genuine evidence the data model is right - that is the whole reason
  Revu matters more than Acrobat here. What the GUI route CANNOT give is markup
  ENUMERATION: there is no counterpart to the Acrobat leg's per-page annotation count,
  because that needs the scripting API. So this leg answers "does Revu draw it correctly?"
  and not "does Revu parse N markups?". The structural half of that question is already
  covered by src-tauri/tests/bb_interop_conformance.rs.

  ONE INSTANCE, MANY TABS. Revu is single-instance: launching Revu.exe with a second file
  hands off to the running copy and opens a tab rather than starting a new process. The leg
  works with that instead of against it - launch once, open each file as a tab, capture,
  and close the application once at the end. Relaunching per file would cost roughly 30s of
  Revu startup per PDF and gains nothing.

.NOTES
  NEVER FORCE-KILLS REVU. Closing is WM_CLOSE and a wait. If Revu will not close, or a
  dialog is blocking it, the leg screenshots the dialog and REPORTS it - it does not click
  through prompts it does not recognise, because an unknown dialog on the owner's own
  workstation may be a licence, update or recovery prompt whose wrong button has real
  consequences. Same reasoning as AcrobatLeg.ps1's refusal to kill Acrobat.

  MUST run in the interactive desktop session (Session 1) via its scheduled task. A Session 0
  context has no window station: Revu would have nowhere to draw and screen capture would
  return black.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InputDir,
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [string]$Filter = '*.pdf',
    [string]$RevuExe = 'C:\Program Files\Bluebeam Software\Bluebeam Revu\21\Revu\Revu.exe',
    [int]$LaunchTimeoutSec = 120,
    [int]$OpenTimeoutSec   = 90,
    [int]$RenderTimeoutSec = 60,
    [int]$CloseTimeoutSec  = 45,
    [int]$PreferWidth  = 3840,
    [int]$PreferHeight = 2160,
    # Default to the owner's chosen review monitor, same as the Acrobat leg.
    [string]$TargetDevice = '\\.\DISPLAY1'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

. (Join-Path $PSScriptRoot 'Displays.ps1')
. (Join-Path $PSScriptRoot 'Capture.ps1')

Enable-DpiAwareness

function Write-Log {
    param([string]$Message)
    Write-Output ("[{0}] [bluebeam-gui] {1}" -f (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss'), $Message)
}

# The Revu window we care about is the one with a real title bar and a document-sized frame.
# Revu's splash and its hidden helper windows are excluded by requiring a non-empty title.
function Get-RevuMainWindow {
    param([int]$ProcessId)
    $wins = @(Get-ProcessWindows -ProcessId $ProcessId |
              Where-Object { $_.ClassName -ne '#32770' -and $_.Title -and $_.Width -gt 400 -and $_.Height -gt 300 })
    if ($wins.Count -eq 0) { return $null }
    return ($wins | Sort-Object -Property { $_.Width * $_.Height } -Descending)[0]
}

function Get-RevuProcess {
    return (Get-Process -Name 'Revu' -ErrorAction SilentlyContinue | Sort-Object StartTime | Select-Object -First 1)
}

# Revu's default View/PageLayout comes up on WHATEVER zoom it last remembered (often close
# to Actual Size), not fit-to-window - proven 2026-08-30 by comparing a captured render
# against Revu's OWN page thumbnail: the thumbnail showed the full page including the
# lower-left markup cluster, the main viewport showed only the top ~1/3 (a small scrollbar
# thumb confirms the page ran on well past the bottom of the window). The prior run's "Revu
# is missing the lower-left cluster" finding was this viewport artifact, not a render defect
# - see obs:6vlfocvtwpz58dov3fif and HANDOVER.md 2026-08-30.
#
# Fit Page = Ctrl+9 per Bluebeam's own Keyboard Shortcuts Guide (View section) - Ctrl+0 is
# Fit WIDTH, a different command that would not have fixed this (confirmed against
# https://support.bluebeam.com/resources/pdfs/keyboard-shortcuts.pdf, 2025 edition, not
# assumed). Sent via SendKeys because there is no scripting-API route available on this
# licence tier (see the module header) - so it needs real foreground focus, exactly like a
# human pressing the keys, and only works once the window is provably on top.
function Send-FitPage {
    <#
      SendKeys sends to whatever window currently holds OS KEYBOARD focus - not to
      $WindowHandle, and not to whatever Set-WindowForeground last touched. Foreground raise
      from a background process is not guaranteed (Set-WindowForeground's own docstring:
      "SetForegroundWindow is refused outright for a process that is not already in the
      foreground"), so a silently-denied raise would inject Ctrl+9 into whatever the owner
      has focused instead - a write, not a read, on the owner's own workstation. Proven this
      matters by the same-shaped incident Test-WindowUnobstructed exists to prevent
      (Capture.ps1, 2026-08-29 Teams capture): a handle and a rectangle looking correct is
      not evidence the pixels or the input focus actually belong to us. So the raise is
      verified against GetForegroundWindow() - the exact API that decides where SendKeys
      lands - with a short retry, and refused outright (no SendKeys call at all) if focus
      never lands on our window.
    #>
    param([Parameter(Mandatory = $true)][IntPtr]$WindowHandle)
    Set-WindowForeground -WindowHandle $WindowHandle
    $focusDeadline = (Get-Date).AddSeconds(6)
    while ([Crossviewer.Win]::GetForegroundWindow() -ne $WindowHandle -and (Get-Date) -lt $focusDeadline) {
        Start-Sleep -Milliseconds 500
        Set-WindowForeground -WindowHandle $WindowHandle
    }
    if ([Crossviewer.Win]::GetForegroundWindow() -ne $WindowHandle) {
        throw 'Revu window never gained OS foreground focus - refusing to send Ctrl+9, which would land in whatever window IS focused'
    }
    [System.Windows.Forms.SendKeys]::SendWait('^9')
    # Give Revu's layout engine a moment to re-flow before anything polls for "settled" -
    # otherwise the very first poll can catch the pre-fit frame and call it stable.
    Start-Sleep -Milliseconds 500
}

if (-not (Test-Path -LiteralPath $RevuExe)) { throw "Revu.exe not found at $RevuExe" }
if (-not (Test-Path -LiteralPath $InputDir)) { throw "InputDir not found: $InputDir" }
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$dialogDir = Join-Path $OutputDir '_dialogs'

$pdfs = @(Get-ChildItem -LiteralPath $InputDir -Filter $Filter -File | Sort-Object Name)
Write-Log "found $($pdfs.Count) PDF(s) under $InputDir"
if ($pdfs.Count -eq 0) { throw "no PDFs matched '$Filter' in $InputDir" }

# Refuse to run if the owner already has Revu open - same courtesy as the Acrobat leg. We
# would be opening tabs into, and later closing, their session.
$pre = Get-RevuProcess
if ($pre) {
    throw "Revu is already running (pid $($pre.Id)) - refusing to drive an interactive session. Close it and retry."
}

$displayInfo = Get-CrossviewerDisplays
if ($displayInfo.looks_like_session0) {
    throw 'display enumeration returned the Session 0 pseudo-display - this leg is not running in an interactive session; start it via its scheduled task'
}
foreach ($d in $displayInfo.displays) {
    Write-Log "display $($d.device) primary=$($d.primary) $($d.width)x$($d.height) at ($($d.x),$($d.y)) $($d.orientation)"
}
$target = Select-TargetDisplay -Displays $displayInfo.displays -PreferWidth $PreferWidth -PreferHeight $PreferHeight -TargetDevice $TargetDevice
Write-Log "target display: $($target.device) $($target.width)x$($target.height) - $($target.reason)"

$results = @()
$legError = $null
$revuProc = $null
$mainWin  = $null
$tmpShot  = Join-Path $env:TEMP 'crossviewer-revu-poll.png'

try {
    # --- launch once -------------------------------------------------------------------
    Write-Log "launching $RevuExe with $($pdfs[0].Name)"
    Start-Process -FilePath $RevuExe -ArgumentList "`"$($pdfs[0].FullName)`"" | Out-Null

    $deadline = (Get-Date).AddSeconds($LaunchTimeoutSec)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        $revuProc = Get-RevuProcess
        if ($revuProc) {
            $mainWin = Get-RevuMainWindow -ProcessId $revuProc.Id
            if ($mainWin) { break }
        }
    }
    if (-not $revuProc) { throw "Revu did not start within ${LaunchTimeoutSec}s" }
    if (-not $mainWin)  { throw "Revu started (pid $($revuProc.Id)) but showed no window within ${LaunchTimeoutSec}s" }
    Write-Log "Revu pid $($revuProc.Id), window '$($mainWin.Title)'"

    # A licence, update or first-run dialog at startup blocks everything behind it. Capture
    # it and stop - clicking an unrecognised button on the owner's workstation is not ours to do.
    $startupDialogs = @(Get-ProcessDialogs -ProcessId $revuProc.Id -ExcludeHandle $mainWin.Handle)
    if ($startupDialogs.Count -gt 0) {
        $shots = @()
        foreach ($dlg in $startupDialogs) {
            $p = Join-Path $dialogDir ("startup-dialog-" + [Math]::Abs($dlg.Handle.ToInt64()) + ".png")
            $saved = Save-WindowCapture -WindowHandle $dlg.Handle -Path $p -Foreground -VerifyOnTop
            if ($saved) { $shots += $saved }
            Write-Log "STARTUP DIALOG: '$($dlg.Title)' [$($dlg.ClassName)] -> $saved"
        }
        throw "Revu showed $($startupDialogs.Count) dialog(s) at startup - captured to $dialogDir, not clicked through. Titles: $(($startupDialogs | ForEach-Object { $_.Title }) -join ' | ')"
    }

    Move-WindowToDisplay -WindowHandle $mainWin.Handle -Display $target | Out-Null
    Start-Sleep -Seconds 2
    Write-Log "placed Revu on $($target.device)"

    # --- per file ----------------------------------------------------------------------
    for ($i = 0; $i -lt $pdfs.Count; $i++) {
        $pdf  = $pdfs[$i]
        $stem = [System.IO.Path]::GetFileNameWithoutExtension($pdf.Name)
        $entry = [ordered]@{
            file        = $pdf.Name
            stem        = $stem
            opened      = $false
            title       = $null
            fit_page_sent = $false
            render      = $null
            settled     = $false
            poll_count  = $null
            dialogs     = @()
            error       = $null
            duration_ms = $null
        }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()

        try {
            # The first PDF came up with the launch; the rest are handed to the running
            # instance, which opens each as a new tab.
            if ($i -gt 0) {
                Write-Log "opening $($pdf.Name)"
                Start-Process -FilePath $RevuExe -ArgumentList "`"$($pdf.FullName)`"" | Out-Null
            }

            # Wait for the title bar to name THIS file. Revu titles its window after the
            # active tab, so this is how we know the new tab is the one in front - without
            # it we would happily screenshot the previous document.
            $titleDeadline = (Get-Date).AddSeconds($OpenTimeoutSec)
            $seen = $null
            while ((Get-Date) -lt $titleDeadline) {
                Start-Sleep -Milliseconds 1000
                if (-not (Get-Process -Id $revuProc.Id -ErrorAction SilentlyContinue)) { throw 'Revu exited unexpectedly' }
                $dlgs = @(Get-ProcessDialogs -ProcessId $revuProc.Id -ExcludeHandle $mainWin.Handle)
                if ($dlgs.Count -gt 0) {
                    foreach ($dlg in $dlgs) {
                        $p = Join-Path $dialogDir ("$stem-dialog-" + [Math]::Abs($dlg.Handle.ToInt64()) + ".png")
                        $saved = Save-WindowCapture -WindowHandle $dlg.Handle -Path $p -Foreground -VerifyOnTop
                        $entry.dialogs += [ordered]@{ title = $dlg.Title; class = $dlg.ClassName; capture = $saved }
                        Write-Log "DIALOG while opening $($pdf.Name): '$($dlg.Title)' -> $saved"
                    }
                    throw "Revu raised a dialog while opening this file - captured, not clicked through"
                }
                $w = Get-RevuMainWindow -ProcessId $revuProc.Id
                if ($w) {
                    $mainWin = $w
                    if ($w.Title -like "*$stem*") { $seen = $w.Title; break }
                }
            }
            if (-not $seen) { throw "Revu's window title never named '$stem' within ${OpenTimeoutSec}s (last title: '$($mainWin.Title)')" }
            $entry.opened = $true
            $entry.title  = $seen

            # Keep it maximised on the target panel - a newly-focused tab can restore the frame.
            Move-WindowToDisplay -WindowHandle $mainWin.Handle -Display $target | Out-Null
            Start-Sleep -Milliseconds 800

            # Fit the whole page before anything is photographed - see Send-FitPage's
            # header for why this is load-bearing, not cosmetic.
            try { Send-FitPage -WindowHandle $mainWin.Handle; $entry.fit_page_sent = $true }
            catch { $entry.fit_page_sent = $false; Write-Log "Send-FitPage: $($_.Exception.Message)" }

            # Wait for the page to stop changing. Revu draws progressively - a screenshot
            # taken the instant the tab appears catches a half-rendered page or a spinner.
            # Two consecutive identical captures is the cheapest reliable "it has settled".
            $renderDeadline = (Get-Date).AddSeconds($RenderTimeoutSec)
            $lastHash = $null; $stable = 0; $polls = 0
            while ((Get-Date) -lt $renderDeadline) {
                Start-Sleep -Milliseconds 1500
                $polls++
                $shot = Save-WindowCapture -WindowHandle $mainWin.Handle -Path $tmpShot -Foreground -VerifyOnTop
                if (-not $shot) { continue }
                $h = Get-CaptureHash -Path $shot
                if ($h -and $h -eq $lastHash) { $stable++ } else { $stable = 0 }
                $lastHash = $h
                if ($stable -ge 1) { $entry.settled = $true; break }
            }
            $entry.poll_count = $polls
            if (-not $entry.settled) { Write-Log "$($pdf.Name): render never settled in ${RenderTimeoutSec}s - capturing anyway" }

            $out = Join-Path $OutputDir "$stem.png"
            $saved = Save-WindowCapture -WindowHandle $mainWin.Handle -Path $out -Foreground -VerifyOnTop
            if (-not $saved) { throw 'window capture returned nothing' }
            $entry.render = $saved
            Write-Log "$($pdf.Name): captured -> $saved (settled=$($entry.settled), polls=$polls)"
        } catch {
            $entry.error = $_.Exception.Message
            Write-Log "FAILED $($pdf.Name): $($_.Exception.Message)"
        } finally {
            $sw.Stop()
            $entry.duration_ms = [int]$sw.ElapsedMilliseconds
            $results += [pscustomobject]$entry
        }

        # A modal dialog we refused to click stays up and would fail every remaining file
        # identically. Stop and say so rather than emitting 20 identical failures.
        if ($entry.dialogs.Count -gt 0) {
            $legError = "stopped after $($pdf.Name): an unrecognised Revu dialog is open and was not clicked through"
            Write-Log $legError
            break
        }
    }
} catch {
    $legError = $_.Exception.Message
    Write-Log "LEG FAILED: $legError"
} finally {
    if ($revuProc -and (Get-Process -Id $revuProc.Id -ErrorAction SilentlyContinue)) {
        $w = Get-RevuMainWindow -ProcessId $revuProc.Id
        if ($w) {
            Write-Log 'closing Revu (WM_CLOSE)'
            $close = Close-WindowPolitely -WindowHandle $w.Handle -ProcessId $revuProc.Id -TimeoutSec $CloseTimeoutSec
            if (-not $close.closed) {
                foreach ($dlg in $close.dialogs) {
                    $p = Join-Path $dialogDir ("close-dialog-" + [Math]::Abs($dlg.Handle.ToInt64()) + ".png")
                    $saved = Save-WindowCapture -WindowHandle $dlg.Handle -Path $p -Foreground -VerifyOnTop
                    Write-Log "CLOSE DIALOG: '$($dlg.Title)' -> $saved"
                }
                # Reported, never force-killed. A Revu left open is a nuisance; a Revu killed
                # mid-write is a corrupted session on the owner's machine.
                Write-Log "Revu did not close within ${CloseTimeoutSec}s (pid $($revuProc.Id) still running) - NOT force-killed, reporting instead"
                if (-not $legError) { $legError = "Revu did not close within ${CloseTimeoutSec}s; pid $($revuProc.Id) left running deliberately" }
            } else {
                Write-Log 'Revu closed'
            }
        }
    }
    Remove-Item -LiteralPath $tmpShot -Force -ErrorAction SilentlyContinue
}

$payload = [ordered]@{
    engine     = 'bluebeam-gui'
    method     = 'GUI automation (Script Engine is licence-gated; see BluebeamLeg.ps1)'
    blocked    = $false
    reason     = $legError
    version    = (Get-Item -LiteralPath $RevuExe).VersionInfo.ProductVersion
    machine    = $env:COMPUTERNAME
    started_at = (Get-Date).ToString('o')
    input_dir  = $InputDir
    output_dir = $OutputDir
    display    = $target
    displays   = $displayInfo.displays
    results    = $results
}
$jsonPath = Join-Path $OutputDir 'bluebeam-gui-results.json'
# 5.1's Set-Content -Encoding UTF8 writes a BOM that trips strict JSON parsers on the Mac side.
[System.IO.File]::WriteAllText($jsonPath, ($payload | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding($false)))
Write-Log "wrote $jsonPath"
