<#
.SYNOPSIS
  Acrobat leg of the redline cross-viewer harness.

.DESCRIPTION
  Drives a real Adobe Acrobat DC install over its Interapplication Communication (IAC)
  COM interface - AcroExch.App / AcroExch.AVDoc / AcroExch.PDDoc / AcroExch.AVPageView
  plus the document JSObject - to answer, per PDF, the questions the manual cross-viewer
  check used to answer by eye:

    - does Acrobat open the file at all, or does it error / offer to repair it?
    - how many pages does Acrobat believe the document has?
    - how many annotations does Acrobat's own annotation scan find on each page?
    - what does each page actually LOOK like when Acrobat renders it?

  The last one is the point of the exercise: Acrobat regenerates markup appearances
  from the annotation dictionary rather than blitting our stored /AP, so a page that
  renders correctly here is real evidence the data model is right - which a
  PDFium-family renderer cannot give us (see tools/crossviewer-render-matrix.mjs's
  header for why Chrome does not count as an independent engine).

  MUST run in an interactive desktop session (Session 1). Acrobat is a GUI application:
  under a Session 0 service/SSH context AVDoc.Open has no window station to draw into
  and either fails or hangs. Registration + invocation: Register-CrossviewerTask.ps1.

.NOTES
  RENDERING IS BY SCREEN CAPTURE, NOT BY saveAs. The obvious route - the JSObject's
  saveAs(path, 'com.adobe.acrobat.png') - is DEAD as a render primitive here. Measured on
  mr-desktop with Acrobat DC 26.1 on 2026-08-29: the call never returns. Open and the
  annotation scan both succeed (pages and annots are read correctly first), then saveAs
  hangs indefinitely with the process still Responding, no dialog on screen and zero PNGs
  written - first across an 8-file batch, then reproduced on a single file with Acrobat
  fully VISIBLE, which rules out the earlier app.Hide() theory. Two separate runs, 0 PNGs
  from 9 attempts.

  So the page is captured the way a human reviewer would see it: place the window on a
  known landscape display, fit the page, and photograph the screen rectangle. That is a
  WEAKER artifact than a saveAs export (it carries Acrobat's chrome and is display-
  resolution bound) but it is a REAL Acrobat rasterisation of our annotation dictionary,
  which is the property the check actually depends on.

  Never kills Acrobat. Documents are closed via AVDoc.Close and the application via
  App.Exit; a leftover process is reported, not force-terminated (killing Acrobat can
  leave recovery state that poisons the next run, and on this shared workstation it
  could take down a window the owner had open).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InputDir,
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [string]$Filter = '*.pdf',
    [int]$OpenTimeoutSec = 60,
    # Which monitor to put Acrobat on. mr-desktop has a portrait panel; a landscape sheet
    # reviewed there is scaled down hard, so captures must land on a landscape display.
    # DISPLAY1 is the owner's chosen review monitor - default to it rather than to the
    # largest panel, so captures land where the owner can watch them.
    # ITS SIZE DEPENDS ON DPI AWARENESS: an unaware process is told 2560x1440 (the scaled
    # logical size, which is what an earlier session recorded); once SetProcessDPIAware has
    # been called the same panel reports its true 3840x2160. Screen capture addresses
    # PHYSICAL pixels, so the leg must be DPI-aware or captures land on the wrong rectangle.
    [string]$TargetDevice = '\\.\DISPLAY1',
    [int]$PreferWidth  = 3840,
    [int]$PreferHeight = 2160,
    # How long to let a page settle before its capture is accepted as final.
    [int]$SettleTimeoutSec = 12,
    # How long to wait for Acrobat to actually PAINT a page after the document opens.
    # Generous because every run cold-starts Acrobat.
    [int]$PagePaintTimeoutSec = 45,
    # Launch Acrobat with the PDF as a COMMAND-LINE ARGUMENT and attach to the document it
    # opens for itself, instead of driving IAC's AVDoc.Open against an already-running
    # Acrobat. Measured 2026-08-30: with bSDIMode=1 and AVDoc.Open, Acrobat reports
    # pages/annots correctly and GetAVPageView() returns a live view, yet enumerating every
    # top-level window finds exactly ONE - the empty application shell. No document window
    # is ever created, so there is nothing that could paint. This switch tests whether a
    # normal file-open (the path a human takes) produces the window IAC does not.
    [switch]$LaunchViaCommandLine,
    [string]$AcrobatExe = 'C:\Program Files\Adobe\Acrobat DC\Acrobat\Acrobat.exe'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

. (Join-Path $PSScriptRoot 'Displays.ps1')
. (Join-Path $PSScriptRoot 'Capture.ps1')

<#
  Acrobat's JSObject is a raw IDispatch. PowerShell's normal late binding cannot call
  its methods - every call comes back "Value does not fall within the expected range."
  (reproduced on Acrobat DC 26.1 during this harness's first Session-1 run). Reflection
  against System.__ComObject with an explicit InvokeMethod binding is the documented
  way in, and is what every working Acrobat-from-PowerShell example uses.

  NOTE this applies ONLY to the JSObject. AcroExch.AVDoc / PDDoc / AVPageView are normal
  dual-interface COM objects and late-bind from PowerShell without help.
#>
function Invoke-Jso {
    param(
        [Parameter(Mandatory = $true)]$Jso,
        [Parameter(Mandatory = $true)][string]$Method,
        [object[]]$Arguments = @()
    )
    [System.__ComObject].InvokeMember(
        $Method,
        [System.Reflection.BindingFlags]::InvokeMethod,
        $null,
        $Jso,
        $Arguments
    )
}

function Write-Log {
    param([string]$Message)
    $stamp = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss')
    Write-Output "[$stamp] [acrobat] $Message"
}

# AVPageView.ZoomTo zoom types (AVZoomType, unchanged since Acrobat 5).
$AVZoomFitPage = 1

# AVDoc.SetViewMode: hide the navigation panel so the page gets the whole window.
$PDUseNone = 1

function Get-AcrobatWindow {
    <#
      The handle to photograph, chosen BY DOCUMENT TITLE rather than by "main window".

      WHY NOT MainWindowHandle - measured 2026-08-29: selecting Acrobat's window that way
      produced a verified, unobstructed, correctly-placed capture of Acrobat's HOME screen
      (Menu / Home / + Create chrome over an empty dark pane) while AllTypes.pdf was open
      and its 20 annotations had already been counted. Acrobat DC runs several processes
      and keeps the Home screen in a separate top-level window from the document, and
      .NET's MainWindowHandle is not reliably the document one. Every structural check
      passed; only the pixels showed the document was not there.

      TITLE WAS NOT THE DISCRIMINATOR UNDER TABBED VIEWING, AND IS UNDER SDI. Measured
      2026-08-29 with tabbed viewing on: Acrobat DC 26.x kept every document in a TAB inside
      one shared frame whose title stayed 'Adobe Acrobat (64-bit)' regardless of what was
      open - exactly one AcrobatSDIWindow existed, so class alone was enough and title was
      useless. With `bSDIMode=1` set (2026-08-30, see docs/TESTING.md) that changes: the
      document gets its OWN top-level window and the empty application shell REMAINS as a
      second AcrobatSDIWindow. The shell is maximised, so it is the LARGEST candidate, so
      picking candidates[0] photographs the shell - which is exactly the empty
      'Menu / home / + Create' frame captured on the first SDI run.

      Selection is therefore: among AcrobatSDIWindow candidates, prefer one whose title is
      NOT the bare application title (the shell), largest first; fall back to the largest
      candidate when every window carries the bare title, which is the tabbed-viewing case
      and preserves the old behaviour exactly.

      Whether the PAGE is actually showing inside the chosen window is still settled
      afterwards by looking at the pixels (Test-PageVisible), never by a window property -
      every window property looked correct while the pane was blank.

      Returns $null when no frame has appeared yet; callers should retry, since the window
      is created asynchronously after AVDoc.Open returns.
    #>
    $candidates = @(Get-AcrobatWindowCandidates | Where-Object { $_.class -eq 'AcrobatSDIWindow' })
    if ($candidates.Count -eq 0) { return $null }
    # 'Adobe Acrobat (64-bit)' / 'Adobe Acrobat' with no document part is the empty shell.
    $documents = @($candidates | Where-Object { $_.title -notmatch '^\s*Adobe Acrobat( \(\d+-bit\))?\s*$' })
    if ($documents.Count -gt 0) { return $documents[0] }
    return $candidates[0]
}

function Test-PageVisible {
    <#
      Answers the only question that matters about a capture: is a PDF PAGE actually being
      displayed, or are we photographing Acrobat's Home screen?

      This exists because on 2026-08-29 a capture passed every structural check - real
      handle, correct monitor, unobstructed, window verified on top, 85 KB well-formed PNG -
      and showed Acrobat's Home chrome over an empty dark pane. No window property
      distinguished that from a successful render. The pixels did.

      The corpus renders on a white page against Acrobat's dark UI, so a displayed page is a
      large bright region. Sampling a coarse grid over the middle of the frame and requiring
      a meaningful fraction of near-white pixels separates "page showing" from "empty pane"
      cheaply and without assuming anything about the markup itself.

      NOTE this is a PRESENCE test, not a correctness test - it proves a page is on screen,
      never that the page is right. Correctness is the vision review's job.
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [double]$MinBrightFraction = 0.10
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{ visible = $false; bright_fraction = 0.0; error = 'no capture file' }
    }
    $bmp = $null
    try {
        $bmp = New-Object System.Drawing.Bitmap $Path
        # Central band only: the toolbars top and bottom are chrome in every case.
        $x0 = [int]($bmp.Width  * 0.15); $x1 = [int]($bmp.Width  * 0.85)
        $y0 = [int]($bmp.Height * 0.15); $y1 = [int]($bmp.Height * 0.85)
        $stepX = [Math]::Max(1, [int](($x1 - $x0) / 60))
        $stepY = [Math]::Max(1, [int](($y1 - $y0) / 60))
        $bright = 0; $total = 0
        for ($y = $y0; $y -lt $y1; $y += $stepY) {
            for ($x = $x0; $x -lt $x1; $x += $stepX) {
                $c = $bmp.GetPixel($x, $y)
                $total++
                if ($c.R -gt 200 -and $c.G -gt 200 -and $c.B -gt 200) { $bright++ }
            }
        }
        $frac = if ($total -gt 0) { [double]$bright / $total } else { 0.0 }
        return [pscustomobject]@{
            visible         = ($frac -ge $MinBrightFraction)
            bright_fraction = [Math]::Round($frac, 4)
            error           = $null
        }
    } catch {
        return [pscustomobject]@{ visible = $false; bright_fraction = 0.0; error = $_.Exception.Message }
    } finally {
        if ($null -ne $bmp) { $bmp.Dispose() }
    }
}

function Get-AcrobatWindowCandidates {
    <# Every visible top-level window owned by any Acrobat process, largest first. #>
    $procs = @(Get-Process -Name 'Acrobat' -ErrorAction SilentlyContinue)
    $candidates = @()
    foreach ($p in $procs) {
        foreach ($w in @(Get-ProcessWindows -ProcessId $p.Id)) {
            $candidates += [pscustomobject]@{
                handle = $w.Handle; pid = $p.Id; title = $w.Title
                class = $w.ClassName; area = ($w.Width * $w.Height)
                width = $w.Width; height = $w.Height
            }
        }
    }
    return @($candidates | Sort-Object -Property area -Descending)
}

function Save-SettledCapture {
    <#
      Captures a window repeatedly until two consecutive frames are byte-identical, so a
      page that is still painting is never accepted as the final render. Returns an object
      carrying the path and whether it actually settled - an unsettled capture is still
      written (it is better evidence than nothing) but is flagged so the report can say so.
    #>
    param(
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][string]$Path,
        [int]$TimeoutSec = 12
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $prevHash = $null
    $attempts = 0
    # Any frame that passed -VerifyOnTop is genuine Acrobat output. If a later frame is
    # blocked (typically by one of Acrobat's own dialogs) the earlier verified frame is
    # still the best evidence available, so keep it rather than returning nothing.
    $lastGood = $null

    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 600
        $attempts++
        # -VerifyOnTop: a frame is only written when the window provably owns its own
        # rectangle. Without it CopyFromScreen happily photographs whatever is in front -
        # on 2026-08-29 that was the owner's Teams window, and the PNG looked perfect.
        $written = Save-WindowCapture -WindowHandle $WindowHandle -Path $Path -Foreground -VerifyOnTop
        if (-not $written) {
            $why = Test-WindowUnobstructed -WindowHandle $WindowHandle
            $reason = if ($why.unobstructed) { 'window vanished during capture' }
                      else { "window obscured ($($why.hits)/$($why.total) sample points ours; blocker $($why.blocker))" }
            return [pscustomobject]@{ path = $lastGood; settled = $false; attempts = $attempts; error = $reason }
        }
        $lastGood = $Path
        $hash = Get-CaptureHash -Path $Path
        if ($null -ne $prevHash -and $hash -eq $prevHash) {
            return [pscustomobject]@{ path = $Path; settled = $true; attempts = $attempts; error = $null }
        }
        $prevHash = $hash
    }
    return [pscustomobject]@{ path = $lastGood; settled = $false; attempts = $attempts; error = "did not stabilise within ${TimeoutSec}s" }
}

if (-not (Test-Path -LiteralPath $InputDir)) { throw "InputDir not found: $InputDir" }
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$pdfs = @(Get-ChildItem -LiteralPath $InputDir -Filter $Filter -File | Sort-Object Name)
Write-Log "found $($pdfs.Count) PDF(s) under $InputDir"
if ($pdfs.Count -eq 0) { throw "no PDFs matched '$Filter' in $InputDir" }

# Declared before the try so the JSON payload below can still be written when the run
# aborts before display selection.
$app         = $null
$target      = $null
$displayInfo = $null
$results     = @()

# Physical pixels, not scaled ones - must happen before any window rectangle is read.
Enable-DpiAwareness

try {
    if ($LaunchViaCommandLine) {
        if ($pdfs.Count -ne 1) { throw "-LaunchViaCommandLine handles exactly one file; got $($pdfs.Count)" }
        if (-not (Test-Path -LiteralPath $AcrobatExe)) { throw "Acrobat not found at $AcrobatExe" }
        Write-Log "launching Acrobat with $($pdfs[0].Name) on the command line"
        Start-Process -FilePath $AcrobatExe -ArgumentList "`"$($pdfs[0].FullName)`"" | Out-Null
        $lDeadline = (Get-Date).AddSeconds(40)
        while ((Get-Date) -lt $lDeadline) {
            $c = @(Get-AcrobatWindowCandidates | Where-Object { $_.class -eq 'AcrobatSDIWindow' })
            if ($c.Count -gt 0) { break }
            Start-Sleep -Milliseconds 500
        }
        Write-Log 'windows after command-line launch:'
        foreach ($c in @(Get-AcrobatWindowCandidates)) {
            Write-Log "  candidate hwnd=$($c.handle) pid=$($c.pid) $($c.width)x$($c.height) class=$($c.class) title='$($c.title)'"
        }
    }

    Write-Log 'creating AcroExch.App'
    if ($LaunchViaCommandLine) {
        # MEASURED 2026-08-30: once Acrobat has been started NORMALLY (command line),
        # CreateObject('AcroExch.App') fails with 0x80080005 CO_E_SERVER_EXEC_FAILURE -
        # the running instance does not serve automation. So under this switch COM is
        # BEST-EFFORT: if it is unavailable the leg still places and photographs the real
        # document window, and reports pages/annots as null rather than aborting the run.
        try { $app = New-Object -ComObject AcroExch.App }
        catch { $app = $null; Write-Log "AcroExch.App unavailable after command-line launch: $($_.Exception.Message)" }
    } else {
        $app = New-Object -ComObject AcroExch.App
    }

    # IAC attaches to an ALREADY-RUNNING Acrobat rather than starting a private one, so a
    # batch run could churn - and App.Exit() below could close - documents a human has open
    # on this shared workstation. cad-export learned this the expensive way with AutoCAD
    # (obs:8ji54fchnw06p7w9iwe6). Refuse rather than risk it.
    $preExisting = if ($null -ne $app) { [int]$app.GetNumAVDocs() } else { 0 }
    if ($preExisting -gt 0 -and -not $LaunchViaCommandLine) {
        throw "Acrobat already has $preExisting document(s) open - refusing to run so an interactive session is not disturbed. Close them and retry."
    }

    # Acrobat must be VISIBLE: the render is a screen capture of its own window.
    if ($null -ne $app) { try { $app.Show() | Out-Null } catch { Write-Log "Show() refused: $($_.Exception.Message)" } }
    Write-Log "Acrobat IAC up (AVDoc count at start: $preExisting)"

    # Enumerate here rather than on the calling machine: a Session 0 context reports a
    # single fake 1024x768 "WinDisc" device instead of the real monitors.
    $displayInfo = Get-CrossviewerDisplays
    if ($displayInfo.looks_like_session0) {
        throw 'display enumeration returned the Session 0 pseudo-display - this leg is not running in an interactive session; start it via its scheduled task'
    }
    foreach ($d in $displayInfo.displays) {
        Write-Log "display $($d.device) primary=$($d.primary) $($d.width)x$($d.height) at ($($d.x),$($d.y)) $($d.orientation)"
    }
    $target = Select-TargetDisplay -Displays $displayInfo.displays -PreferWidth $PreferWidth -PreferHeight $PreferHeight -TargetDevice $TargetDevice
    Write-Log "target display: $($target.device) $($target.width)x$($target.height) - $($target.reason)"

    foreach ($pdf in $pdfs) {
        $stem = [System.IO.Path]::GetFileNameWithoutExtension($pdf.Name)
        $entry = [ordered]@{
            file           = $pdf.Name
            stem           = $stem
            opened         = $false
            open_error     = $null
            pages          = $null
            annots_total   = $null
            annots_by_page = @()
            renders        = @()
            render_detail  = @()
            render_error   = $null
            display        = $null
            duration_ms    = $null
        }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $avdoc = $null

        try {
            Write-Log "opening $($pdf.Name)"
            $avdoc = if ($LaunchViaCommandLine) { $null } else { New-Object -ComObject AcroExch.AVDoc }
            # Open returns false on a file Acrobat refuses outright. A file Acrobat
            # considers damaged-but-repairable normally opens after a silent repair, which
            # is exactly the signal we want to surface - hence pages/annots are read from
            # what Acrobat actually produced, not from our own expectations.
            if ($LaunchViaCommandLine) {
                # Acrobat already opened the file for itself; adopt that document.
                if ($null -ne $app) {
                    $adDeadline = (Get-Date).AddSeconds(30)
                    while ((Get-Date) -lt $adDeadline) {
                        try { $avdoc = $app.GetActiveDoc() } catch { $avdoc = $null }
                        if ($null -ne $avdoc) { break }
                        Start-Sleep -Milliseconds 500
                    }
                }
                if ($null -eq $avdoc) { Write-Log 'no AVDoc available - capture-only run (pages/annots will be null)' }
                else { Write-Log 'attached to the command-line-opened document' }
            } else {
                $opened = $avdoc.Open($pdf.FullName, 'redline-crossviewer')
                if (-not $opened) { throw 'AVDoc.Open returned false (Acrobat refused the file)' }
            }
            $entry.opened = $true

            $pddoc = $null; $jso = $null
            if ($null -ne $avdoc) {
                $pddoc = $avdoc.GetPDDoc()
                $entry.pages = [int]$pddoc.GetNumPages()
                $jso = $pddoc.GetJSObject()
                if ($null -eq $jso) { throw 'GetJSObject returned null (JavaScript disabled in Acrobat?)' }
            } else {
                # Capture-only: one page is photographed, the page count is not knowable.
                $entry.pages = 1
            }

            # syncAnnotScan forces Acrobat to finish its background annotation scan before
            # getAnnots is trusted; without it getAnnots can return an incomplete set on a
            # freshly opened document.
            if ($null -ne $jso) { try { Invoke-Jso -Jso $jso -Method 'syncAnnotScan' | Out-Null }
            catch { Write-Log "syncAnnotScan: $($_.Exception.Message)" } }

            $byPage = @()
            $total = 0
            for ($p = 0; $p -lt $entry.pages; $p++) {
                $n = 0
                try {
                    $pageAnnots = Invoke-Jso -Jso $jso -Method 'getAnnots' -Arguments @($p)
                    if ($null -ne $pageAnnots) { $n = @($pageAnnots).Count }
                } catch {
                    # getAnnots throws rather than returning empty on a page with none.
                    $n = 0
                }
                $byPage += [ordered]@{ page = $p; annots = $n }
                $total += $n
            }
            $entry.annots_by_page = $byPage
            $entry.annots_total = $total
            Write-Log "$($pdf.Name): pages=$($entry.pages) annots=$total"

            # ---- render: place, fit, photograph -------------------------------------
            # Declared before the try: Set-StrictMode makes the finally below fail on an
            # unassigned variable if the try throws early.
            $win = $null
            $topmostSet = $false
            $sw0 = Get-Date
            try {
                # Give the page area the whole window; the nav panel steals ~250px.
                if ($null -ne $avdoc) { try { $avdoc.SetViewMode($PDUseNone) | Out-Null }
                catch { Write-Log "SetViewMode: $($_.Exception.Message)" } }

                # The document window appears asynchronously after Open returns; wait for
                # the one whose title carries our file name.
                $winDeadline = (Get-Date).AddSeconds(10)
                $win = Get-AcrobatWindow
                while ($null -eq $win -and (Get-Date) -lt $winDeadline) {
                    Start-Sleep -Milliseconds 500
                    $win = Get-AcrobatWindow
                }
                # Log EVERY candidate on every run, not only on failure. Under SDI there
                # are two AcrobatSDIWindows and which one was chosen is the single most
                # useful line in this log; on 2026-08-30 its absence cost a whole run.
                Write-Log 'window candidates:'
                foreach ($c in @(Get-AcrobatWindowCandidates)) {
                    Write-Log "  candidate hwnd=$($c.handle) pid=$($c.pid) $($c.width)x$($c.height) class=$($c.class) title='$($c.title)'"
                }
                if ($null -eq $win) {
                    throw 'Acrobat has no document frame (class AcrobatSDIWindow) to capture'
                }
                Write-Log "document frame hwnd=$($win.handle) pid=$($win.pid) $($win.width)x$($win.height) title='$($win.title)'"

                # Select this document's TAB. Acrobat opens into a shared frame that can
                # still be sitting on the Home screen; without this the pane photographs
                # blank while every window property looks correct.
                if ($null -ne $avdoc) {
                    try { $avdoc.BringToFront() | Out-Null; Write-Log 'AVDoc.BringToFront' }
                    catch { Write-Log "AVDoc.BringToFront: $($_.Exception.Message)" }
                }

                # AVDoc.BringToFront raises the FRAME but does not reliably switch the
                # active TAB. Measured 2026-08-29 on Acrobat DC 26.1: with AllTypes.pdf
                # open, its 20 annotations already counted, GetAVPageView() returning a
                # live page view and ZoomTo(FitPage) accepted, the frame still sat on the
                # HOME screen - the capture shows 'Menu / Home / + Create' over an empty
                # pane with no document tab and no toolbar. Every window property and the
                # verify-on-top gate passed; only the pixels showed the document was not
                # being displayed. The JS Doc.bringToFront() acts on the DOCUMENT rather
                # than on the window, and is the documented way to select its tab.
                #
                # MEASURED RESULT: this does NOT fix the blank pane. The call succeeds and
                # logs cleanly, and the resulting capture is BYTE-IDENTICAL (59,446 bytes)
                # to the run without it. Kept because it is correct and free, and recorded
                # here so the next session does not spend the idle window retrying it.
                # The blank pane is NOT a tab-selection problem.
                if ($null -ne $jso) {
                    try { Invoke-Jso -Jso $jso -Method 'bringToFront' | Out-Null; Write-Log 'jso.bringToFront' }
                    catch { Write-Log "jso.bringToFront: $($_.Exception.Message)" }
                }

                Move-WindowToDisplay -WindowHandle $win.handle -Display $target | Out-Null
                $entry.display = $target.device

                # Maximise the DOCUMENT window inside the (now maximised) app frame.
                if ($null -ne $avdoc) {
                    try { $avdoc.Maximize($true) | Out-Null }
                    catch { Write-Log "AVDoc.Maximize: $($_.Exception.Message)" }
                }

                # Raise Acrobat above whatever the owner left on this monitor, then PROVE
                # it. A scheduled task holds no foreground rights, so without the topmost
                # push Acrobat can sit behind another maximised window while every handle
                # and rectangle the leg reads still looks correct.
                $topmostSet = $true
                Set-WindowForeground -WindowHandle $win.handle
                $clearBy = (Get-Date).AddSeconds(6)
                $onTop = Test-WindowUnobstructed -WindowHandle $win.handle
                while (-not $onTop.unobstructed -and (Get-Date) -lt $clearBy) {
                    Start-Sleep -Milliseconds 500
                    Set-WindowForeground -WindowHandle $win.handle
                    $onTop = Test-WindowUnobstructed -WindowHandle $win.handle
                }
                if (-not $onTop.unobstructed) {
                    # Refuse rather than photograph someone else's window.
                    throw "Acrobat window is obscured on $($target.device) - $($onTop.hits)/$($onTop.total) sample points are ours, blocker $($onTop.blocker). Refusing to capture: the pixels would not be Acrobat's."
                }
                Write-Log "Acrobat window verified on top ($($onTop.hits)/$($onTop.total) sample points)"

                # Capture-only runs have no AVDoc, so no page view and no programmatic
                # zoom - Acrobat's own default zoom is what gets photographed.
                $pageView = if ($null -ne $avdoc) { $avdoc.GetAVPageView() } else { $null }
                if ($null -eq $pageView -and $null -ne $avdoc) { throw 'GetAVPageView returned null' }

                # Fit the whole page: a capture cropped to the visible band of a zoomed-in
                # page would silently hide markup that lives outside it.
                if ($null -ne $pageView) {
                    try { $pageView.ZoomTo($AVZoomFitPage, 0) | Out-Null }
                    catch { Write-Log "ZoomTo(FitPage): $($_.Exception.Message)" }
                }

                # WAIT FOR THE PAGE TO ACTUALLY PAINT before capturing anything.
                # The COM calls above (GetNumPages, getAnnots) answer from the in-memory
                # PDDoc and return in milliseconds while the VIEWER is still blank, so a
                # successful annotation scan is NO evidence that anything is on screen -
                # that mismatch is exactly what produced a run reporting pages=1 annots=20
                # alongside a capture of an empty pane. And because a run leaves no Acrobat
                # behind, every run is a COLD start. Poll the pixels until a page appears,
                # logging the trajectory so a failure says whether nothing ever painted or
                # it was merely slow.
                $pageDeadline = (Get-Date).AddSeconds($PagePaintTimeoutSec)
                $probe = Join-Path $OutputDir '.paint-probe.png'
                $painted = $false
                $lastFrac = -1.0
                while ((Get-Date) -lt $pageDeadline) {
                    $ok = Save-WindowCapture -WindowHandle $win.handle -Path $probe -VerifyOnTop
                    if ($ok) {
                        $v = Test-PageVisible -Path $probe
                        if ($v.bright_fraction -ne $lastFrac) {
                            Write-Log "waiting for page paint: bright fraction $($v.bright_fraction)"
                            $lastFrac = $v.bright_fraction
                        }
                        if ($v.visible) { $painted = $true; break }
                    }
                    Start-Sleep -Milliseconds 800
                }
                Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
                if ($painted) {
                    Write-Log "page painted after $([int]((Get-Date) - $sw0).TotalSeconds)s"
                } else {
                    Write-Log "page never painted within ${PagePaintTimeoutSec}s - capturing anyway so the frame can be inspected"
                }

                $renderPaths = @()
                $detail = @()
                for ($p = 0; $p -lt $entry.pages; $p++) {
                    try { $pageView.GoTo($p) | Out-Null }
                    catch { Write-Log "GoTo page ${p}: $($_.Exception.Message)" }
                    # Re-fit after the page change - page sizes can differ within a document.
                    if ($null -ne $pageView) { try { $pageView.ZoomTo($AVZoomFitPage, 0) | Out-Null } catch { } }

                    $pngName = if ($entry.pages -gt 1) { "${stem}_Page_$($p + 1).png" } else { "$stem.png" }
                    $pngPath = Join-Path $OutputDir $pngName

                    $cap = Save-SettledCapture -WindowHandle $win.handle -Path $pngPath -TimeoutSec $SettleTimeoutSec
                    if ($cap.path) {
                        $size = (Get-Item -LiteralPath $cap.path).Length
                        $vis = Test-PageVisible -Path $cap.path
                        if ($vis.visible) {
                            $renderPaths += $cap.path
                            Write-Log "captured page $($p + 1)/$($entry.pages) -> $pngName ($size bytes, settled=$($cap.settled), frames=$($cap.attempts), bright=$($vis.bright_fraction))"
                        } else {
                            # A capture with no page in it is worse than none: it looks like
                            # a successful render to every downstream consumer.
                            # KEEP the rejected frame under an unmistakable name. Deleting it
                            # throws away the only evidence of WHY a render failed, and the
                            # name guarantees no downstream consumer mistakes it for a render.
                            $rejPath = Join-Path $OutputDir ("REJECTED-" + $pngName)
                            try { Move-Item -LiteralPath $cap.path -Destination $rejPath -Force } catch { }
                            Write-Log "capture REJECTED for page $($p + 1): no page visible (bright fraction $($vis.bright_fraction) < threshold) - kept as REJECTED-$pngName for diagnosis"
                            $cap = [pscustomobject]@{ path = $null; settled = $cap.settled; attempts = $cap.attempts; error = "no page visible (bright fraction $($vis.bright_fraction))" }
                        }
                    } else {
                        Write-Log "capture FAILED for page $($p + 1): $($cap.error)"
                    }
                    $detail += [ordered]@{
                        page     = $p
                        path     = $cap.path
                        settled  = $cap.settled
                        attempts = $cap.attempts
                        error    = $cap.error
                    }
                }
                $entry.renders = $renderPaths
                $entry.render_detail = $detail
                if ($renderPaths.Count -eq 0) { $entry.render_error = 'no page captured' }
            } catch {
                $entry.render_error = $_.Exception.Message
                Write-Log "render failed for $($pdf.Name): $($_.Exception.Message)"
            } finally {
                # A window left topmost stays pinned over everything the owner does next.
                if ($topmostSet -and $null -ne $win) {
                    try { Set-WindowTopmost -WindowHandle $win.handle -On $false | Out-Null } catch { }
                }
            }
        } catch {
            $entry.open_error = $_.Exception.Message
            Write-Log "FAILED $($pdf.Name): $($_.Exception.Message)"
        } finally {
            if ($null -ne $avdoc) {
                if ($null -ne $avdoc) { try { $avdoc.Close($true) | Out-Null } catch { Write-Log "Close: $($_.Exception.Message)" } }
                try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($avdoc) } catch { }
            }
            $sw.Stop()
            $entry.duration_ms = [int]$sw.ElapsedMilliseconds
            $results += [pscustomobject]$entry
        }
    }
} finally {
    if ($null -ne $app) {
        # Snapshot before Exit() so we can check afterward whether it actually worked -
        # App.Exit() has been observed to return without error while the underlying
        # Acrobat/AcroCEF processes keep running (2026-08-31 session, three occurrences in
        # one day). Never force-killed here; only reported, per this leg's own "never
        # force-terminate a viewer" rule (see the header notes).
        $preExitPids = @((Get-Process -Name 'Acrobat', 'AcroCEF' -ErrorAction SilentlyContinue).Id)
        try { $app.Exit() | Out-Null; Write-Log 'App.Exit() called' }
        catch { Write-Log "App.Exit refused: $($_.Exception.Message) (leaving process alone - never force-killed)" }
        try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) } catch { }

        if ($preExitPids.Count -gt 0) {
            Start-Sleep -Seconds 2
            $survivors = @($preExitPids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
            if ($survivors.Count -gt 0) {
                Write-Log "App.Exit() did NOT terminate $($survivors.Count) process(es) (pid(s): $($survivors -join ', ')) - left running, never force-killed. Manual recovery: run CloseAcrobat.ps1 (API-only, safe) or, if that hangs too, verify no owner work is open before a manual Stop-Process."
            } else {
                Write-Log 'Acrobat exited (all tracked processes confirmed gone)'
            }
        }
    }
}

$payload = [ordered]@{
    engine     = 'acrobat'
    render_via = 'window-capture'
    version    = (Get-Item 'C:\Program Files\Adobe\Acrobat DC\Acrobat\Acrobat.exe' -ErrorAction SilentlyContinue).VersionInfo.ProductVersion
    machine    = $env:COMPUTERNAME
    started_at = (Get-Date).ToString('o')
    input_dir  = $InputDir
    output_dir = $OutputDir
    display    = $target
    displays   = if ($null -ne $displayInfo) { $displayInfo.displays } else { @() }
    results    = $results
}
$jsonPath = Join-Path $OutputDir 'acrobat-results.json'
# Windows PowerShell 5.1's Set-Content -Encoding UTF8 emits a BOM, which trips strict
# JSON parsers on the Mac side. Write UTF-8 without a BOM explicitly.
$json = $payload | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($jsonPath, $json, (New-Object System.Text.UTF8Encoding($false)))
Write-Log "wrote $jsonPath"
