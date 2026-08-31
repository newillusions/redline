<#
.SYNOPSIS
  Bluebeam Revu leg of the redline cross-viewer harness.

.DESCRIPTION
  Intended counterpart to AcrobatLeg.ps1: drive Bluebeam Revu's Script Engine to open each
  staged PDF, report open errors, enumerate the markups Revu itself parses out of the file,
  and export a render for the vision-review pass.

  Revu matters more than Acrobat for this project's purposes: it is the viewer the owner
  actually reviews in, and unlike a generic viewer it REGENERATES markup appearances from
  the annotation dictionary rather than blitting our stored /AP - so it is the one renderer
  that can prove the data model is right rather than just the appearance stream.

.NOTES
  STATUS ON mr-desktop AS OF 2026-08-29: BLOCKED BY LICENCE TIER, not by code.

  Revu 21 is installed (C:\Program Files\Bluebeam Software\Bluebeam Revu\21\Revu\) and ships
  ScriptEngine.exe (version 21.10.0.19316), but invoking it returns, verbatim:

      This feature requires a maximum subscription level. Please upgrade to access advanced
      scripting capabilities

  with exit code -4, for any invocation including `/?`. The Script Engine is gated behind
  Revu's top subscription tier; the installed licence does not include it. No script content
  is reached, so this is not something the harness can work around by writing better script
  code. Bluebeam.Exporter.exe was probed as an alternative and exits -1 with no usable CLI.

  This script therefore currently PROBES and REPORTS rather than pretending to run. It emits
  the same JSON shape as the Acrobat leg with `blocked: true` and the exact error text, so a
  run's report says why Bluebeam is missing instead of silently covering only Acrobat.

  TO UNBLOCK, in rough order of cost:
    1. Upgrade the Revu licence on this machine to the tier that includes scripting, then
       replace the probe below with real Script Engine calls (Open / Markups export / render).
    2. Failing that, automate the Revu GUI in this same Session-1 context: launch Revu.exe
       with the file, wait for its window, screenshot it, and close via WM_CLOSE. That yields
       a real Revu render for the vision-review leg (which is most of the value) but no
       markup enumeration. Deliberately not built here: it is a materially different and
       more brittle mechanism than the other leg, and it should not be bolted on without the
       owner deciding the licence upgrade is off the table.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InputDir,
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [string]$ScriptEngine = 'C:\Program Files\Bluebeam Software\Bluebeam Revu\21\Revu\ScriptEngine.exe'
)

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$probe = [ordered]@{
    engine       = 'bluebeam'
    blocked      = $true
    reason       = $null
    exit_code    = $null
    script_engine = $ScriptEngine
    version      = $null
    machine      = $env:COMPUTERNAME
    checked_at   = (Get-Date).ToString('o')
    results      = @()
}

if (-not (Test-Path -LiteralPath $ScriptEngine)) {
    $probe.reason = "ScriptEngine.exe not found at $ScriptEngine (Revu not installed, or a different major version)"
} else {
    $probe.version = (Get-Item -LiteralPath $ScriptEngine).VersionInfo.ProductVersion
    try {
        $out = & $ScriptEngine '/?' 2>&1 | Out-String
        $probe.exit_code = $LASTEXITCODE
        if ($LASTEXITCODE -eq 0) {
            $probe.blocked = $false
            $probe.reason = 'Script Engine responded - licence tier appears sufficient; wire up the real run (see .NOTES step 1)'
        } else {
            $probe.reason = $out.Trim()
        }
    } catch {
        $probe.exit_code = -1
        $probe.reason = $_.Exception.Message
    }
}

Write-Output "[bluebeam] blocked=$($probe.blocked) exit=$($probe.exit_code)"
Write-Output "[bluebeam] $($probe.reason)"

$jsonPath = Join-Path $OutputDir 'bluebeam-results.json'
$json = $probe | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText($jsonPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Output "[bluebeam] wrote $jsonPath"
