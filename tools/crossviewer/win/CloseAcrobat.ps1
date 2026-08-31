<#
.SYNOPSIS
  Close every document open in Acrobat and exit the application, via COM only.

.DESCRIPTION
  Recovery for a harness run that was interrupted (task stopped, host killed) while it
  still had a document open over IAC. Acrobat keeps running with that document loaded,
  which makes AcrobatLeg.ps1's "someone else is using Acrobat" guard fire on every
  subsequent run.

  Deliberately API-only: AVDoc.Close then App.Exit. This never terminates a process.
  Force-killing Acrobat leaves autosave/recovery state that prompts on the next launch
  and, on a shared workstation, could destroy work the owner had open.

  Run in an interactive session (Session 1) - same constraint as AcrobatLeg.ps1.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$app = $null
try {
    $app = New-Object -ComObject AcroExch.App
    $n = [int]$app.GetNumAVDocs()
    Write-Output "open documents: $n"
    # Close from the end backwards - closing by index shifts the ones after it.
    for ($i = $n - 1; $i -ge 0; $i--) {
        try {
            $doc = $app.GetAVDoc($i)
            # Never log the filename here - GetFileName() can carry owner/client-sensitive
            # project names, matching the never-log-titles convention every other leg in
            # this harness follows (see DiagWindows.ps1/CloseRevu.ps1).
            $doc.Close($true) | Out-Null   # $true = discard changes, never prompt
            Write-Output "closed [$i]"
        } catch {
            Write-Output "could not close [$i]: $($_.Exception.Message)"
        }
    }
    Write-Output "remaining: $([int]$app.GetNumAVDocs())"
} finally {
    if ($null -ne $app) {
        try { $app.Exit() | Out-Null; Write-Output 'App.Exit called' }
        catch { Write-Output "App.Exit refused: $($_.Exception.Message)" }
        try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) } catch { }
    }
}
