<#
.SYNOPSIS
  Gracefully close ONLY the Acrobat.exe pids the harness launched.

.DESCRIPTION
  Used by the two-pass run (run-crossviewer.mjs / two-pass.mjs) between the IAC counts pass
  and the command-line render pass, and after each render. The orchestrator writes the pids
  the legs reported launching (`launched_pids` in their results JSON) to -PidFile and runs
  this script; it closes those pids and nothing else.

  SAFETY (owner workstation):
    - A process not listed in -PidFile is never touched, whatever it is running.
    - A listed entry is {pid, start}: the process must be named Acrobat AND its StartTime (UTC
      ticks) must equal `start`. A reused pid number with a different start is never touched.
    - No COM, no AVDoc.Close($true): this script never discards changes in a document it
      did not open. It asks each listed Acrobat to close its main window
      (CloseMainWindow = clicking X) and waits; it never terminates a process.
    - Missing or empty -PidFile = nothing to close, exit 0.
  Exit 0 when every listed pid is gone, 3 when one survives. The caller verifies
  independently (Get-Process by exact image name, with start times) and must not trust this exit code alone.

  Run in an interactive session (Session 1) - same constraint as AcrobatLeg.ps1.
#>
[CmdletBinding()]
param(
    [string]$PidFile = (Join-Path $env:LOCALAPPDATA 'redline-crossviewer\close-pids.json'),
    [int]$GracefulWaitSec = 25,
    # Image name to match. Always 'Acrobat'; a parameter only so the matching logic can be
    # exercised against a harmless process in a test.
    [string]$ImageName = 'Acrobat'
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $PidFile)) {
    Write-Output "no pid file at $PidFile - nothing to close"
    exit 0
}
$raw = (Get-Content -Raw -LiteralPath $PidFile).Trim()
$targets = @()
if ($raw) { $targets = @($raw | ConvertFrom-Json | ForEach-Object { [pscustomobject]@{ pid = [int]$_.pid; start = [string]$_.start } }) }
if ($targets.Count -eq 0) {
    Write-Output 'pid file empty - nothing to close'
    exit 0
}

# Only entries that are listed AND currently the SAME process (name + pid + start time).
function Get-Targets {
    @($targets | ForEach-Object {
        $p = Get-Process -Id $_.pid -ErrorAction SilentlyContinue
        if ($null -ne $p -and $p.ProcessName -eq $ImageName) {
            $st = $null
            try { $st = [string]$p.StartTime.ToUniversalTime().Ticks } catch { }
            if ($st -eq $_.start) { $p }
        }
    })
}

$deadline = (Get-Date).AddSeconds($GracefulWaitSec)
while ((Get-Targets).Count -gt 0 -and (Get-Date) -lt $deadline) {
    foreach ($proc in @(Get-Targets)) {
        try { [void]$proc.CloseMainWindow() } catch { }
    }
    Start-Sleep -Seconds 2
}
$left = @(Get-Targets | ForEach-Object { $_.Id })
if ($left.Count -gt 0) {
    Write-Output "harness-launched Acrobat.exe still running (pid(s): $($left -join ', ')) - left alone, never force-killed"
    exit 3
}
Write-Output 'harness-launched Acrobat.exe gone'
exit 0
