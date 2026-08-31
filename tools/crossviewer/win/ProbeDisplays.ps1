<#
.SYNOPSIS
  Report the real monitor layout, from inside the interactive session.

.DESCRIPTION
  Diagnostic for the cross-viewer harness. Run it as a Session-1 scheduled task before the
  first batch on any new machine: it prints every attached display with its orientation and
  says which one the harness would capture on, and why.

  Running it over SSH reports the Session 0 pseudo-display instead of the real monitors -
  the script detects that and says so rather than handing back a fake layout.

  Writes displays.json alongside the other run artefacts.

.NOTES
  No window-placement self-test here on purpose. An earlier version created a throwaway
  window to exercise Move-WindowToDisplay; two variants were tried and both were worse than
  the problem they checked. notepad.exe on Windows 11 is an MSIX stub whose launcher exits
  immediately, so its MainWindowHandle is null forever; a WinForms form has a real handle but
  keeps the PowerShell host alive with no message pump, which left the scheduled task hung.
  Move-WindowToDisplay is therefore exercised for real by AcrobatLeg.ps1 against Acrobat's
  own window, and its result is recorded per-file in acrobat-results.json.
#>
[CmdletBinding()]
param([string]$OutputDir = 'H:\redline-crossviewer\out')

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Displays.ps1')

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$info = Get-CrossviewerDisplays

Write-Output "session_id: $((Get-Process -Id $PID).SessionId)"
Write-Output "looks_like_session0: $($info.looks_like_session0)"
if ($info.looks_like_session0) {
    Write-Output 'WARNING: this is the Session 0 pseudo-display, NOT the real monitor layout.'
    Write-Output '         Run this via its scheduled task (redline-crossviewer-displays), not over SSH.'
}
foreach ($d in $info.displays) {
    Write-Output ("  {0} primary={1} {2}x{3} at ({4},{5}) {6}" -f $d.device, $d.primary, $d.width, $d.height, $d.x, $d.y, $d.orientation)
}

$target = $null
$selectError = $null
try { $target = Select-TargetDisplay -Displays $info.displays }
catch { $selectError = $_.Exception.Message }

if ($target) { Write-Output "TARGET: $($target.device) $($target.width)x$($target.height) - $($target.reason)" }
else { Write-Output "TARGET: none - $selectError" }

$payload = [ordered]@{
    probed_at           = (Get-Date).ToString('o')
    machine             = $env:COMPUTERNAME
    session_id          = (Get-Process -Id $PID).SessionId
    looks_like_session0 = $info.looks_like_session0
    displays            = $info.displays
    target              = $target
    select_error        = $selectError
}
$json = $payload | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText((Join-Path $OutputDir 'displays.json'), $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Output "wrote $(Join-Path $OutputDir 'displays.json')"
