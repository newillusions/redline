<#
  Pure process-ancestry helpers for AcrobatLeg.ps1 (dot-sourced). They take process LISTS as
  arguments (Win32_Process-shaped objects: ProcessId, ParentProcessId, CreationDate, Name) and
  touch no live system state, so the matching rules can be unit-tested on any platform.
#>

function Select-OwnedDescendants {
    <#
      Pids of Acrobat.exe processes that descend from the launch process, EXCLUDING the root.

      Returns nothing unless the process now at RootPid is exactly the one we launched
      (LiveStart == RootStart): if our Acrobat exited and Windows reused the pid for another
      process, nothing is walked and nothing is reported.

      Each descendant must have started at or after its parent (a process cannot be created
      before its parent), so a child row whose ParentProcessId points at a REUSED parent pid
      is rejected. A 1 ms tolerance absorbs the precision gap between Get-Process.StartTime
      and the WMI CreationDate.
    #>
    param(
        [Parameter(Mandatory = $true)][object[]]$Processes,
        [Parameter(Mandatory = $true)][int]$RootPid,
        [Parameter(Mandatory = $true)][datetime]$RootStart,
        [Parameter(Mandatory = $true)][datetime]$LiveStart,
        [string]$ImageName = 'Acrobat.exe'
    )
    if ([math]::Abs(($LiveStart.ToUniversalTime() - $RootStart.ToUniversalTime()).TotalMilliseconds) -gt 1) { return @() }

    $start = @{ $RootPid = $RootStart.ToUniversalTime() }
    $queue = New-Object System.Collections.Generic.Queue[int]
    $queue.Enqueue($RootPid)
    $owned = New-Object System.Collections.Generic.List[int]
    while ($queue.Count -gt 0) {
        $parent = $queue.Dequeue()
        $parentStart = $start[$parent]
        foreach ($p in $Processes) {
            if ([int]$p.ParentProcessId -ne $parent) { continue }
            $id = [int]$p.ProcessId
            if ($start.ContainsKey($id)) { continue }
            $created = ([datetime]$p.CreationDate).ToUniversalTime()
            if ($created -lt $parentStart.AddMilliseconds(-1)) { continue }   # older than its "parent": reused pid
            $start[$id] = $created
            $queue.Enqueue($id)
            if ($p.Name -eq $ImageName) { $owned.Add($id) }
        }
    }
    return @($owned)
}

function Get-NewAcrobatRootCount {
    <#
      How many independent process trees the given NEW Acrobat processes form (a process whose
      parent is not itself in the set is a root). One Acrobat instance with helper processes is
      one tree; an owner's Acrobat started in the same window as ours is a second tree.
    #>
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$Processes)
    $ids = @{}
    foreach ($p in $Processes) { $ids[[int]$p.ProcessId] = $true }
    $roots = 0
    foreach ($p in $Processes) { if (-not $ids.ContainsKey([int]$p.ParentProcessId)) { $roots++ } }
    return $roots
}
