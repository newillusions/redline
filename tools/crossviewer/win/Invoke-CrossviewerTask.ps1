<#
.SYNOPSIS
  Run ONE cross-viewer scheduled task, with a hard timeout and guaranteed cleanup.

.DESCRIPTION
  The legs must run in the interactive session (Session 1), which is reached by starting a
  scheduled task. This wrapper is the ONLY sanctioned way to do that, because it enforces
  the process rules the harness has to obey on a workstation the owner is sitting at:

    - exactly ONE task is enabled at a time, and it is disabled again the moment the run
      ends - including on timeout, on error, and on Ctrl-C. A harness task left enabled is
      how Acrobat ended up cycling through files on the owner's desktop unannounced.
    - every run has a HARD timeout. A leg that wedges is stopped, not left running.
    - viewer processes started BY THIS RUN (StartTime after the run began) are terminated
      if they survive it; anything older is left strictly alone, because it belongs to the
      owner. This is the one place the harness force-kills, and every kill is logged.

  Run this from an SSH/Session 0 shell - it only starts and supervises the task; the leg
  itself executes in Session 1.

.NOTES
  The leg scripts themselves never kill a viewer (see AcrobatLeg.ps1's notes). Force
  termination lives HERE, scoped by start time, so the rule "never kill the owner's
  Acrobat" and the rule "never leave a wedged harness process behind" can both hold.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$TaskName,
    [int]$TimeoutSec = 120,
    # AcroCEF MUST be in this list. Acrobat DC renders its document pane in CEF child
    # processes; killing only the Acrobat parent leaves those children orphaned, and a pile
    # of orphaned AcroCEF processes poisons every subsequent launch - the next Acrobat paints
    # its native chrome and never renders a page. Measured 2026-08-29: six orphans had
    # accumulated across five runs, and every run after the first captured an empty pane.
    [string[]]$ViewerProcess = @('Acrobat', 'AcroCEF', 'Revu'),
    # Skip the post-run sweep when a human wants to inspect the viewer state left behind.
    [switch]$NoKill
)

$ErrorActionPreference = 'Stop'

function Say { param([string]$m) Write-Output ("[{0}] {1}" -f (Get-Date).ToString('HH:mm:ss'), $m) }

$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $task) { throw "scheduled task '$TaskName' not found" }

# Refuse to start while any OTHER harness task is enabled: two legs sharing Session 1 fight
# over the same window station, and that is exactly what wedged it on 2026-08-29.
$otherEnabled = @(Get-ScheduledTask -TaskName 'redline-crossviewer-*' |
    Where-Object { $_.TaskName -ne $TaskName -and $_.State -ne 'Disabled' })
if ($otherEnabled.Count -gt 0) {
    throw "refusing to start: other harness task(s) not disabled - $(($otherEnabled | ForEach-Object { $_.TaskName }) -join ', ')"
}

$runStart = Get-Date
Say "run start $($runStart.ToString('o'))"

# Anything already running belongs to the owner. Record it so the sweep can tell the
# difference, and warn - the leg will refuse anyway if Acrobat has documents open.
foreach ($n in $ViewerProcess) {
    $pre = @(Get-Process -Name $n -ErrorAction SilentlyContinue)
    if ($pre.Count -gt 0) { Say "WARNING pre-existing $n process(es): $($pre.Count) - these will NOT be touched" }
}

$timedOut = $false
try {
    Enable-ScheduledTask  -TaskName $TaskName | Out-Null
    Say "enabled $TaskName"
    Start-ScheduledTask   -TaskName $TaskName
    Say "started $TaskName (timeout ${TimeoutSec}s)"

    $deadline = $runStart.AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 3
        $state = (Get-ScheduledTask -TaskName $TaskName).State
        if ($state -eq 'Ready') { Say "task finished after $([int]((Get-Date) - $runStart).TotalSeconds)s"; break }
    }
    if ((Get-ScheduledTask -TaskName $TaskName).State -ne 'Ready') {
        $timedOut = $true
        Say "TIMEOUT after ${TimeoutSec}s - stopping task"
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
    }
} finally {
    Disable-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue | Out-Null
    Say "disabled $TaskName"

    if (-not $NoKill) {
        foreach ($n in $ViewerProcess) {
            # StartTime throws or returns null for processes this account cannot fully open
            # (some AcroCEF children behave this way). A process whose start time cannot be
            # read is LEFT ALONE - the whole point of the start-time filter is to never touch
            # anything that might be the owner's.
            $stale = @()
            foreach ($cand in @(Get-Process -Name $n -ErrorAction SilentlyContinue)) {
                $st = $null
                try { $st = $cand.StartTime } catch { $st = $null }
                if ($null -ne $st -and $st -gt $runStart) {
                    $stale += [pscustomobject]@{ proc = $cand; started = $st }
                }
            }
            foreach ($p in $stale) {
                Say "KILL $n pid=$($p.proc.Id) started=$($p.started.ToString('o')) (started by this run, did not exit)"
                Stop-Process -Id $p.proc.Id -Force -ErrorAction SilentlyContinue
            }
            if ($stale.Count -eq 0) { Say "$n : nothing to clean up" }
        }
    }
}

$info = Get-ScheduledTaskInfo -TaskName $TaskName
Say "LastTaskResult=$($info.LastTaskResult) LastRunTime=$($info.LastRunTime)"
if ($timedOut) { Say 'RESULT: TIMED OUT' } else { Say 'RESULT: completed' }
