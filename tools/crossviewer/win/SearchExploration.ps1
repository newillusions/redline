<#
.SYNOPSIS
  Hands-on exploration pass of Bluebeam Revu's real Search panel on mr-desktop,
  under the same GUI-automation harness as BluebeamGuiLeg.ps1 (crossviewer).

.DESCRIPTION
  QUEUED — DO NOT RUN until mr-desktop is confirmed free (the crossviewer
  capture agent may be using it; team-lead gates this explicitly). Owner
  authorized hands-on use of the local Revu install for search exploration
  2026-08-31 ("you should also use the local install to run tests and
  explore the functionality as well").

  This is a DISCOVERY pass, not a scripted assertion pass. Documentation and
  official videos (docs/bluebeam-search-behavior-reference.md) describe the
  Search panel's behavior in words, but this script has zero verified ground
  truth on the panel's actual screen layout, control names, or exact click
  coordinates for buttons like Check All / the lightning-bolt Check Options
  menu. Rather than guess pixel coordinates (which would silently misclick
  on a real, expensive-to-recover-from workstation — see BluebeamGuiLeg.ps1's
  own "never force, never click through unknown dialogs" precedent), this
  script:
    1. Automates the SAFE, MECHANICAL parts: launch Revu, place it on the
       target display, open the Search panel (Ctrl+F — matches the
       documented shortcut), type a known query, press Enter, screenshot.
    2. Uses UI Automation (System.Windows.Automation) to ENUMERATE the
       Search panel's control tree — every button/checkbox/list-item name,
       AutomationId, and bounding rect — into the JSON report. This is how
       a follow-up pass (or a human) finds the REAL click targets without
       this script inventing them.
    3. Screenshots the full window at each milestone (scope switched, query
       run, results shown) for every one of the five scopes redline ships
       (Document/Page/Open Docs/Recents/Folder+Subfolders) plus one
       Folder-scope click on a result for a file NOT already open, to
       settle the open question in the reference doc's §8 (does Revu
       auto-open it, same as redline's `openFilePath`, or something else).

  WHAT THIS DOES NOT DO: it does not click Check All, Collapse All, or the
  lightning-bolt menu — those coordinates are unknown, and a wrong click on
  Check Options could apply a bulk action (e.g. Replace Checked) to the
  owner's real corpus files. Those interactions are a DELIBERATE follow-up
  once pass 1's control-tree dump gives real AutomationIds to target
  precisely, not blind coordinates.

.NOTES
  Mirrors BluebeamGuiLeg.ps1's conventions exactly: dot-sources the same
  Displays.ps1/Capture.ps1 helpers (same directory, $PSScriptRoot-relative —
  works once both this PR and the crossviewer PR are merged to main), never
  force-kills Revu, screenshots and reports any unrecognised dialog instead
  of clicking through it, WM_CLOSE-only teardown.

  MUST run in the interactive desktop session (Session 1) via the same
  scheduled-task pattern as the crossviewer harness (Register-CrossviewerTask.ps1
  / Invoke-CrossviewerTask.ps1) — a Session 0 context has no window station.

  -InputDir default below is UNVERIFIED — it is the path HANDOVER.md records
  the crossviewer corpus as staged at, not a path this script (or the
  session that wrote it) has confirmed exists on mr-desktop today. Pass the
  real corpus folder explicitly; do not trust the default blind.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InputDir,
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [string]$Filter = '*.pdf',
    [string]$RevuExe = 'C:\Program Files\Bluebeam Software\Bluebeam Revu\21\Revu\Revu.exe',
    [string]$Query = 'concrete',
    [int]$LaunchTimeoutSec = 120,
    [int]$OpenTimeoutSec   = 90,
    [int]$SettleMs         = 1500,
    [int]$CloseTimeoutSec  = 45,
    [int]$PreferWidth  = 3840,
    [int]$PreferHeight = 2160,
    [string]$TargetDevice = '\\.\DISPLAY1'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

. (Join-Path $PSScriptRoot 'Displays.ps1')
. (Join-Path $PSScriptRoot 'Capture.ps1')

Enable-DpiAwareness
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

function Write-Log {
    param([string]$Message)
    Write-Output ("[{0}] [search-explore] {1}" -f (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss'), $Message)
}

# Same window-identification pattern as BluebeamGuiLeg.ps1 (kept inline, not
# imported — that script's own such functions are not exported as a module).
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

# Dump every descendant control's Name/ControlType/AutomationId/bounding-rect
# under a window handle — the discovery mechanism this pass relies on instead
# of guessed click coordinates. Capped depth-agnostic (FindAll is already
# flat over the full subtree); capped COUNT so a pathological control tree
# can't blow up the report.
function Get-ControlTreeDump {
    param([Parameter(Mandatory = $true)][IntPtr]$WindowHandle, [int]$MaxControls = 500)
    try {
        $root = [System.Windows.Automation.AutomationElement]::FromHandle($WindowHandle)
        if (-not $root) { return @() }
        $all = $root.FindAll(
            [System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition
        )
        $out = @()
        $n = [Math]::Min($all.Count, $MaxControls)
        for ($i = 0; $i -lt $n; $i++) {
            $el = $all.Item($i)
            try {
                $r = $el.Current.BoundingRectangle
                $out += [ordered]@{
                    name            = $el.Current.Name
                    control_type    = $el.Current.ControlType.ProgrammaticName
                    automation_id   = $el.Current.AutomationId
                    class_name      = $el.Current.ClassName
                    is_enabled      = $el.Current.IsEnabled
                    bounds          = [ordered]@{ x = $r.X; y = $r.Y; w = $r.Width; h = $r.Height }
                }
            } catch {
                # A control that throws reading its own properties (COM timing) is
                # skipped, not fatal to the whole dump.
                continue
            }
        }
        return $out
    } catch {
        Write-Log "control-tree dump failed: $($_.Exception.Message)"
        return @()
    }
}

function Send-Keystrokes {
    param([Parameter(Mandatory = $true)][IntPtr]$WindowHandle, [Parameter(Mandatory = $true)][string]$Keys)
    Set-WindowForeground -WindowHandle $WindowHandle
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    Start-Sleep -Milliseconds 400
}

if (-not (Test-Path -LiteralPath $RevuExe)) { throw "Revu.exe not found at $RevuExe" }
if (-not (Test-Path -LiteralPath $InputDir)) { throw "InputDir not found: $InputDir" }
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$dialogDir = Join-Path $OutputDir '_dialogs'

$pdfs = @(Get-ChildItem -LiteralPath $InputDir -Filter $Filter -File | Sort-Object Name)
Write-Log "found $($pdfs.Count) PDF(s) under $InputDir"
if ($pdfs.Count -lt 2) { throw "need at least 2 PDFs (Open-Docs scope needs a second tab) - found $($pdfs.Count) in $InputDir" }

$pre = Get-RevuProcess
if ($pre) {
    throw "Revu is already running (pid $($pre.Id)) - refusing to drive an interactive session. Close it and retry."
}

$displayInfo = Get-CrossviewerDisplays
if ($displayInfo.looks_like_session0) {
    throw 'display enumeration returned the Session 0 pseudo-display - start this via its scheduled task'
}
$target = Select-TargetDisplay -Displays $displayInfo.displays -PreferWidth $PreferWidth -PreferHeight $PreferHeight -TargetDevice $TargetDevice
Write-Log "target display: $($target.device) $($target.width)x$($target.height) - $($target.reason)"

$steps = @()
$legError = $null
$revuProc = $null
$mainWin  = $null

function Record-Step {
    param([string]$Name, [IntPtr]$WindowHandle, [hashtable]$Extra = @{})
    $shotPath = Join-Path $OutputDir "$Name.png"
    $saved = Save-WindowCapture -WindowHandle $WindowHandle -Path $shotPath -Foreground
    $dump  = Get-ControlTreeDump -WindowHandle $WindowHandle
    $entry = [ordered]@{
        step         = $Name
        at           = (Get-Date).ToString('o')
        screenshot   = $saved
        control_count = $dump.Count
        controls     = $dump
    }
    foreach ($k in $Extra.Keys) { $entry[$k] = $Extra[$k] }
    $script:steps += [pscustomobject]$entry
    Write-Log "recorded step '$Name' -> $saved ($($dump.Count) controls)"
}

try {
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

    $startupDialogs = @(Get-ProcessDialogs -ProcessId $revuProc.Id -ExcludeHandle $mainWin.Handle)
    if ($startupDialogs.Count -gt 0) {
        foreach ($dlg in $startupDialogs) {
            $p = Join-Path $dialogDir ("startup-dialog-" + [Math]::Abs($dlg.Handle.ToInt64()) + ".png")
            $saved = Save-WindowCapture -WindowHandle $dlg.Handle -Path $p -Foreground
            Write-Log "STARTUP DIALOG: '$($dlg.Title)' -> $saved"
        }
        throw "Revu showed $($startupDialogs.Count) dialog(s) at startup - captured, not clicked through"
    }

    Move-WindowToDisplay -WindowHandle $mainWin.Handle -Display $target | Out-Null
    Start-Sleep -Seconds 2

    # --- open a SECOND file as a tab (Open Docs scope needs 2+ open) --------------------
    Write-Log "opening second file for Open-Docs scope: $($pdfs[1].Name)"
    Start-Process -FilePath $RevuExe -ArgumentList "`"$($pdfs[1].FullName)`"" | Out-Null
    Start-Sleep -Seconds 5
    $mainWin = Get-RevuMainWindow -ProcessId $revuProc.Id

    # --- open the Search panel (Ctrl+F, per docs/bluebeam-search-behavior-reference.md §1) --
    Send-Keystrokes -WindowHandle $mainWin.Handle -Keys '^f'
    Start-Sleep -Milliseconds $SettleMs
    Record-Step -Name '01-search-panel-opened' -WindowHandle $mainWin.Handle

    # --- run a query against whatever scope is default-selected, screenshot + dump ------
    Send-Keystrokes -WindowHandle $mainWin.Handle -Keys $Query
    Start-Sleep -Milliseconds 300
    Send-Keystrokes -WindowHandle $mainWin.Handle -Keys '{ENTER}'
    Start-Sleep -Milliseconds $SettleMs
    Record-Step -Name '02-query-run-default-scope' -WindowHandle $mainWin.Handle -Extra @{ query = $Query }

    # NOTE: switching to each of Document/Page/Open Docs/Recents/Folder+Subfolders and
    # clicking a not-yet-open Folder result requires knowing the scope dropdown's and
    # result list's real AutomationIds - which step 02's control-tree dump exists to
    # reveal. Deliberately not scripted blind past this point (see header). The dump
    # from steps 01/02 is this pass's actual deliverable; a fast follow-up script (or a
    # human reading the JSON) can add scope-switch/click steps with real AutomationId
    # targets once this report is in hand.

} catch {
    $legError = $_.Exception.Message
    Write-Log "PASS FAILED: $legError"
} finally {
    if ($revuProc -and (Get-Process -Id $revuProc.Id -ErrorAction SilentlyContinue)) {
        $w = Get-RevuMainWindow -ProcessId $revuProc.Id
        if ($w) {
            Write-Log 'closing Revu (WM_CLOSE)'
            $close = Close-WindowPolitely -WindowHandle $w.Handle -ProcessId $revuProc.Id -TimeoutSec $CloseTimeoutSec
            if (-not $close.closed) {
                foreach ($dlg in $close.dialogs) {
                    $p = Join-Path $dialogDir ("close-dialog-" + [Math]::Abs($dlg.Handle.ToInt64()) + ".png")
                    Save-WindowCapture -WindowHandle $dlg.Handle -Path $p -Foreground | Out-Null
                }
                Write-Log "Revu did not close within ${CloseTimeoutSec}s (pid $($revuProc.Id) still running) - NOT force-killed"
                if (-not $legError) { $legError = "Revu did not close within ${CloseTimeoutSec}s; pid $($revuProc.Id) left running deliberately" }
            } else {
                Write-Log 'Revu closed'
            }
        }
    }
}

$payload = [ordered]@{
    pass       = 'search-exploration-1'
    scope      = 'discovery only - see script header for what is deliberately NOT automated yet'
    blocked    = [bool]$legError
    reason     = $legError
    version    = if (Test-Path -LiteralPath $RevuExe) { (Get-Item -LiteralPath $RevuExe).VersionInfo.ProductVersion } else { $null }
    machine    = $env:COMPUTERNAME
    started_at = (Get-Date).ToString('o')
    input_dir  = $InputDir
    output_dir = $OutputDir
    query      = $Query
    display    = $target
    steps      = $steps
}
$jsonPath = Join-Path $OutputDir 'search-exploration-results.json'
[System.IO.File]::WriteAllText($jsonPath, ($payload | ConvertTo-Json -Depth 10), (New-Object System.Text.UTF8Encoding($false)))
Write-Log "wrote $jsonPath"
