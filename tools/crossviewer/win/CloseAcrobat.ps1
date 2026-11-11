<#
.SYNOPSIS
  Gracefully close ONLY the Acrobat.exe pids the harness launched.

.DESCRIPTION
  Used by the two-pass run (run-crossviewer.mjs / two-pass.mjs) between the IAC counts pass
  and the command-line render pass, and after each render. The orchestrator writes the pids
  the legs reported launching (`launched_pids` in their results JSON) to -PidFile and runs
  this script; it closes those pids and nothing else.

  SAFETY (owner workstation):
    - A pid not listed in -PidFile is never touched, whatever it is running.
    - A listed pid whose process is not named Acrobat is never touched.
    - No COM, no AVDoc.Close($true): this script never discards changes in a document it
      did not open. It asks each listed Acrobat to close its main window
      (CloseMainWindow = clicking X) and waits; it never terminates a process.
    - Missing or empty -PidFile = nothing to close, exit 0.
  Exit 0 when every listed pid is gone, 3 when one survives. The caller verifies
  independently (tasklist by exact image name) and must not trust this exit code alone.

  Run in an interactive session (Session 1) - same constraint as AcrobatLeg.ps1.
#>
[CmdletBinding()]
param(
    [string]$PidFile = (Join-Path $env:LOCALAPPDATA 'redline-crossviewer\close-pids.json'),
    [int]$GracefulWaitSec = 25
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $PidFile)) {
    Write-Output "no pid file at $PidFile - nothing to close"
    exit 0
}
$raw = (Get-Content -Raw -LiteralPath $PidFile).Trim()
$targets = @()
if ($raw) { $targets = @($raw | ConvertFrom-Json | ForEach-Object { [int]$_ }) }
if ($targets.Count -eq 0) {
    Write-Output 'pid file empty - nothing to close'
    exit 0
}

# Only pids that are listed AND currently an Acrobat process.
function Get-Targets {
    @($targets | ForEach-Object {
        $p = Get-Process -Id $_ -ErrorAction SilentlyContinue
        if ($null -ne $p -and $p.ProcessName -eq 'Acrobat') { $p }
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
