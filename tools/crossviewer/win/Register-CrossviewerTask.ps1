<#
.SYNOPSIS
  Register the Session-1 scheduled tasks the cross-viewer harness drives.

.DESCRIPTION
  Acrobat and Revu are GUI applications. Launched from an SSH session they land in Session 0,
  which has no window station to render into - COM either fails or hangs. The established
  workaround on this workstation (proven by cad-export's AutoCAD runner) is a scheduled task
  registered against the interactive user, started remotely with Start-ScheduledTask.

  Two principal settings are load-bearing and were both learned the hard way:
    - RunLevel Limited, not Highest. A GUI app runs at medium integrity; an elevated task
      cannot attach to its COM server.
    - A RESOLVED WindowsIdentity string for -UserId. A hand-built "DOMAIN\user" string fails
      registration with "No mapping between account names and security IDs was done".

  The task runs Windows PowerShell 5.1, not pwsh. On mr-desktop `pwsh` is an MSIX
  app-execution alias with no invocable file path (obs:ryzah0kwi09tjeg9ppf8), so a scheduled
  task - which needs a real executable - cannot use it. The leg scripts are written to be
  5.1-compatible for this reason.

  TASKS ARE REGISTERED DISABLED BY DEFAULT. Fixed 2026-09-04 - Register-ScheduledTask has
  no "create disabled" switch of its own, so every prior version of this script left every
  task it (re)registered in Windows' default Ready/enabled state, silently undoing whatever
  a session had deliberately disabled moments earlier. This bit real runs: PR #85's own
  RETURN and the 2026-09-04 retest RETURN both record having to manually re-disable the
  standard 5 tasks after registration rebuilt PsGuiHost.exe, because registering it had
  quietly re-enabled them. Every call below now disables the task immediately after
  registering it, unless -Enable is passed - so re-running this script to pick up new leg
  code (the common case: a leg script changed, PsGuiHost.exe needs rebuilding, or a new task
  like the self-test below is being added) can never re-arm a task on its own. Passing
  -Enable is for the one case that legitimately wants tasks left runnable straight after
  registration - do that deliberately, not as this script's silent default.

.EXAMPLE
  # Run once at the console or over SSH. Registers every task DISABLED - drive one with
  # Start-ScheduledTask, then Disable-ScheduledTask again when done (or -Enable at
  # registration time if every task should be left runnable).
  powershell -NoProfile -File Register-CrossviewerTask.ps1 -StagingRoot 'H:\redline-crossviewer'
#>
[CmdletBinding()]
param(
    [string]$StagingRoot = 'H:\redline-crossviewer',
    [string]$TaskPrefix  = 'redline-crossviewer',
    # Leave every (re)registered task enabled instead of the safe default. Opt in
    # deliberately - see .NOTES above for why the default flipped.
    [switch]$Enable
)

$ErrorActionPreference = 'Stop'

$psExe = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
if (-not (Test-Path -LiteralPath $psExe)) { throw "Windows PowerShell not found at $psExe" }

# The legs run through PsGuiHost.exe, a GUI-subsystem PowerShell host, NOT powershell.exe.
# See PsGuiHost.cs for the full reasoning; the short version is that on 2026-08-29 this
# machine's interactive session lost the ability to start CONSOLE-subsystem processes -
# a scheduled `cmd /c echo ok > file` never wrote its file, while a GUI-subsystem
# wscript.exe task completed in three seconds. powershell.exe is console-subsystem, so
# every leg would hang. PsGuiHost allocates no console and is immune.
# It is compiled here rather than committed as a binary: csc.exe ships with Windows, the
# build takes about a second, and a checked-in .exe in a source repo is worse.
$hostExe = Join-Path $StagingRoot 'scripts\PsGuiHost.exe'
$hostSrc = Join-Path $StagingRoot 'scripts\PsGuiHost.cs'
if (-not (Test-Path -LiteralPath $hostExe)) {
    if (-not (Test-Path -LiteralPath $hostSrc)) { throw "PsGuiHost.cs not staged at $hostSrc" }
    $csc = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    if (-not (Test-Path -LiteralPath $csc)) { throw "csc.exe not found at $csc" }
    # Reference the SAME System.Management.Automation the 5.1 legs are written against.
    $sma = (& $psExe -NoProfile -Command '[psobject].Assembly.Location').Trim()
    $build = & $csc /nologo /target:winexe "/out:$hostExe" "/r:$sma" $hostSrc 2>&1 | Out-String
    if (-not (Test-Path -LiteralPath $hostExe)) { throw "PsGuiHost build failed: $build" }
    Write-Output 'built PsGuiHost.exe'
}

foreach ($d in @($StagingRoot, "$StagingRoot\in", "$StagingRoot\out", "$StagingRoot\scripts", "$StagingRoot\logs")) {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}

$identity  = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$principal = New-ScheduledTaskPrincipal -UserId $identity -LogonType Interactive -RunLevel Limited
$settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
                -ExecutionTimeLimit (New-TimeSpan -Hours 1) -MultipleInstances IgnoreNew

function Register-Leg {
    param([string]$Name, [string]$ScriptName, [string]$ExtraArgs, [string]$LogName)
    $script = "$StagingRoot\scripts\$ScriptName"
    $log    = "$StagingRoot\logs\$LogName"
    # PsGuiHost takes: <script> <log> [leg parameters]. It tees every PowerShell stream into
    # the log itself, which is what the old -Command ... | Tee-Object form was doing.
    $arg    = '"' + $script + '" "' + $log + '" ' + $ExtraArgs
    $action = New-ScheduledTaskAction -Execute $hostExe -Argument $arg -WorkingDirectory $StagingRoot
    Unregister-ScheduledTask -TaskName $Name -Confirm:$false -ErrorAction SilentlyContinue
    Register-ScheduledTask -TaskName $Name -Action $action -Principal $principal -Settings $settings | Out-Null
    # Register-ScheduledTask has no "create disabled" option - it always lands Ready. Undo
    # that immediately unless the caller explicitly asked to leave tasks runnable (-Enable).
    # See this file's header for why this default changed.
    if (-not $Enable) {
        Disable-ScheduledTask -TaskName $Name | Out-Null
        Write-Output "registered $Name (disabled)"
    } else {
        Write-Output "registered $Name (enabled)"
    }
}

# NOT registered with -LaunchViaCommandLine, deliberately. AcrobatLeg.ps1 throws
# "-LaunchViaCommandLine handles exactly one file; got N" for anything but a single PDF
# (see that script's own param block), and this task's -InputDir is the full multi-file
# corpus. Until AcrobatLeg.ps1 gains a per-file command-line-launch loop (a real feature,
# not a config tweak - open one, capture, close, repeat), this task stays on the IAC
# (AVDoc.Open) path: reliable annotation scan across every file, but no render whenever
# Acrobat lands on its Home-screen shell instead of a document window (see the new
# no-document-window preflight in AcrobatLeg.ps1). Use $TaskPrefix-acrobat-one below for a
# real single-file render via command-line launch.
Register-Leg -Name "$TaskPrefix-acrobat" -ScriptName 'AcrobatLeg.ps1' `
    -ExtraArgs "-InputDir `"$StagingRoot\in`" -OutputDir `"$StagingRoot\out\acrobat`"" -LogName 'acrobat.log'

# One-file render gate via command-line launch (docs/TESTING.md "RESOLVED - Acrobat
# renders", 2026-08-30) - formalises what had only existed as an ad-hoc, hand-registered
# task on mr-desktop (not tracked anywhere in this repo) into the tracked registration.
# -LaunchViaCommandLine requires exactly one PDF, hence the dedicated -InputDir\in-one
# rather than the shared corpus dir. No -Method flag: AcrobatLeg.ps1 has no -Method
# parameter at all - Method Auto is hardcoded at its own Save-WindowCapture call sites
# (see Capture.ps1's -Method Auto usage inside AcrobatLeg.ps1), so passing "-Method Auto"
# here would fail with a parameter-binding error, not select a mode.
Register-Leg -Name "$TaskPrefix-acrobat-one" -ScriptName 'AcrobatLeg.ps1' `
    -ExtraArgs "-InputDir `"$StagingRoot\in-one`" -OutputDir `"$StagingRoot\out\acrobat-one`" -TargetDevice \\.\DISPLAY1 -LaunchViaCommandLine" -LogName 'acrobat-one.log'

Register-Leg -Name "$TaskPrefix-bluebeam" -ScriptName 'BluebeamLeg.ps1' `
    -ExtraArgs "-InputDir `"$StagingRoot\in`" -OutputDir `"$StagingRoot\out\bluebeam`"" -LogName 'bluebeam.log'

# The leg that actually produces Revu renders. BluebeamLeg.ps1 above only probes the
# licence-gated Script Engine and reports why it cannot be used; this one drives the GUI.
# Its time limit is its own: a 24-file batch through a real Revu window takes far longer
# than a probe, and the default 1h settings set would cut it off.
Register-Leg -Name "$TaskPrefix-bluebeam-gui" -ScriptName 'BluebeamGuiLeg.ps1' `
    -ExtraArgs "-InputDir `"$StagingRoot\in`" -OutputDir `"$StagingRoot\out\bluebeam-gui`"" -LogName 'bluebeam-gui.log'

Register-Leg -Name "$TaskPrefix-cleanup" -ScriptName 'CloseAcrobat.ps1' `
    -ExtraArgs '' -LogName 'cleanup.log'

# Diagnostic: reports the REAL monitor layout. Worth running on any new machine before the
# first batch - display enumeration over SSH lies (Session 0 pseudo-display), so this is the
# only trustworthy way to see what panels are actually attached.
Register-Leg -Name "$TaskPrefix-displays" -ScriptName 'ProbeDisplays.ps1' `
    -ExtraArgs "-OutputDir `"$StagingRoot\out`"" -LogName 'displays.log'

# Proves a capture method (PrintWindow, Wgc) actually captures real content on a known
# throwaway window BEFORE it is trusted for a real Acrobat diagnostic run - see
# CaptureSelfTest.ps1's own header. Run this first on any machine/session where the Acrobat
# leg's capture method is in question.
Register-Leg -Name "$TaskPrefix-selftest" -ScriptName 'CaptureSelfTest.ps1' `
    -ExtraArgs "-OutputDir `"$StagingRoot\out\selftest`"" -LogName 'selftest.log'

Write-Output "identity: $identity"
Write-Output "staging:  $StagingRoot"
Write-Output "tasks left $(if ($Enable) { 'ENABLED' } else { 'DISABLED' }) (pass -Enable to change)"
