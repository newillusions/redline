<#
.SYNOPSIS
  Diagnostic: dump every top-level window Get-ProcessWindows finds for a set of process
  names, unfiltered. Read-only - no window is touched.
#>
[CmdletBinding()]
param(
    [string[]]$ProcessName = @('Acrobat', 'Revu')
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Capture.ps1')

foreach ($name in $ProcessName) {
    $procs = @(Get-Process -Name $name -ErrorAction SilentlyContinue)
    Write-Output "== $name : $($procs.Count) process(es)"
    foreach ($p in $procs) {
        $wins = @(Get-ProcessWindows -ProcessId $p.Id)
        Write-Output "  pid=$($p.Id) start=$($p.StartTime) windows=$($wins.Count)"
        foreach ($w in $wins) {
            # Class + dimensions only - never the title (may carry owner-sensitive text,
            # matches the harness's own capture-verification logging convention).
            Write-Output "    handle=$($w.Handle) class='$($w.ClassName)' has_title=$([bool]$w.Title) w=$($w.Width) h=$($w.Height)"
        }
    }
}
