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

.EXAMPLE
  # Run once at the console or over SSH, then drive with Start-ScheduledTask.
  powershell -NoProfile -File Register-CrossviewerTask.ps1 -StagingRoot 'H:\redline-crossviewer'
#>
[CmdletBinding()]
param(
    [string]$StagingRoot = 'H:\redline-crossviewer',
    [string]$TaskPrefix  = 'redline-crossviewer'
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
    Write-Output "registered $Name"
}

Register-Leg -Name "$TaskPrefix-acrobat" -ScriptName 'AcrobatLeg.ps1' `
    -ExtraArgs "-InputDir `"$StagingRoot\in`" -OutputDir `"$StagingRoot\out\acrobat`"" -LogName 'acrobat.log'

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

Write-Output "identity: $identity"
Write-Output "staging:  $StagingRoot"
