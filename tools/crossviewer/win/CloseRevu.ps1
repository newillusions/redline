<#
.SYNOPSIS
  Close a leftover Revu main window, WM_CLOSE only - recovery companion to CloseAcrobat.ps1.

.DESCRIPTION
  Bluebeam has no scripting-API close path (licence-gated Script Engine), so unlike
  CloseAcrobat.ps1 this cannot go through COM. Uses the same Close-WindowPolitely helper
  BluebeamGuiLeg.ps1's own finally block uses: PostMessage(WM_CLOSE), wait, never
  Stop-Process. A window that refuses to close (e.g. an unsaved-changes prompt) is reported,
  not forced - matches every other leg's "never force-terminate a viewer" rule.

  Run in an interactive session (Session 1) via the scheduled-task pattern, same as every
  other leg here.
#>
[CmdletBinding()]
param(
    [int]$CloseTimeoutSec = 30
)

$ErrorActionPreference = 'Stop'

function Write-Log {
    param([string]$Message)
    Write-Output ("[{0}] [close-revu] {1}" -f (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss'), $Message)
}

. (Join-Path $PSScriptRoot 'Capture.ps1')

function Get-RevuMainWindow {
    param([int]$ProcessId)
    $wins = @(Get-ProcessWindows -ProcessId $ProcessId |
              Where-Object { $_.ClassName -ne '#32770' -and $_.Title -and $_.Width -gt 400 -and $_.Height -gt 300 })
    if ($wins.Count -eq 0) { return $null }
    return ($wins | Sort-Object -Property { $_.Width * $_.Height } -Descending)[0]
}

$procs = @(Get-Process -Name 'Revu' -ErrorAction SilentlyContinue | Sort-Object StartTime)
Write-Log "found $($procs.Count) Revu process(es)"
if ($procs.Count -eq 0) { Write-Log 'nothing to do'; exit 0 }

$results = @()
foreach ($p in $procs) {
    $win = Get-RevuMainWindow -ProcessId $p.Id
    if (-not $win) {
        Write-Log "pid $($p.Id): no main window found (already closing or headless) - leaving alone"
        $results += [ordered]@{ pid = $p.Id; had_window = $false; closed = $false }
        continue
    }
    # Class + dimensions only - never the title (may carry owner/client-sensitive text,
    # matches DiagWindows.ps1's convention and the harness's own capture-verification logging).
    Write-Log "pid $($p.Id) window class='$($win.ClassName)' w=$($win.Width) h=$($win.Height) - posting WM_CLOSE"
    $r = Close-WindowPolitely -WindowHandle $win.Handle -ProcessId $p.Id -TimeoutSec $CloseTimeoutSec
    Write-Log "pid $($p.Id): closed=$($r.closed) blocked_by_dialog=$($r.blocked_by_dialog)"
    $results += [ordered]@{ pid = $p.Id; had_window = $true; closed = $r.closed; blocked_by_dialog = $r.blocked_by_dialog }
}

$remaining = @(Get-Process -Name 'Revu' -ErrorAction SilentlyContinue).Count
Write-Log "remaining Revu process(es): $remaining"
$results | ConvertTo-Json -Depth 5 | Write-Output
