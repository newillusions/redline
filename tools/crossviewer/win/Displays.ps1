<#
.SYNOPSIS
  Display enumeration and window placement for the cross-viewer harness.

.DESCRIPTION
  Dot-source this from a leg script. Provides:
    Get-CrossviewerDisplays  - enumerate the real monitors
    Select-TargetDisplay     - pick the landscape 4K panel (or the best available)
    Move-WindowToDisplay     - move and maximise a window onto a chosen display

  WHY THIS EXISTS: mr-desktop has more than one monitor and at least one is in PORTRAIT
  orientation. A PDF page reviewed on a portrait panel is scaled differently and, for a
  landscape drawing sheet, much smaller - so renders and screenshots must be captured on
  the landscape 4K display for the visual check to mean anything. Owner instruction,
  2026-08-29.

.NOTES
  MUST run inside the interactive session (Session 1). Enumerating displays over SSH
  returns a single fake 1024x768 device named "WinDisc" - the Session 0 pseudo-display -
  because a Session 0 context cannot see Session 1's window station. Measured on
  mr-desktop 2026-08-29: SSH reported `WinDisc 1024x768 primary=True` while
  Win32_VideoController simultaneously reported the NVIDIA card driving 3840 x 2160.
  Treat any enumeration that returns exactly one 1024x768 "WinDisc" as PROOF you are in
  the wrong session, not as a real monitor layout.
#>

Add-Type -AssemblyName System.Windows.Forms -ErrorAction SilentlyContinue

# RECT and the P/Invoke signatures MUST be declared in ONE Add-Type call. Splitting them
# across two calls compiles them into two separate assemblies, and the second has no
# reference to the first - so GetWindowRect's `out Crossviewer.RECT` fails to compile with
# "The type or namespace name 'RECT' does not exist in the namespace 'Crossviewer'".
# Use -TypeDefinition (full source) rather than -MemberDefinition (generated wrapper) so
# the struct and the class land in the same compilation unit.
if (-not ('Crossviewer.NativeWindow' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace Crossviewer {
    public struct RECT {
        public int Left; public int Top; public int Right; public int Bottom;
    }

    public static class NativeWindow {
        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter,
            int X, int Y, int cx, int cy, uint uFlags);

        [DllImport("user32.dll")]
        public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

        [DllImport("user32.dll")]
        public static extern bool IsWindowVisible(IntPtr hWnd);

        [DllImport("user32.dll")]
        public static extern bool SetForegroundWindow(IntPtr hWnd);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    }
}
'@
}

# ShowWindow commands
$script:SW_RESTORE  = 9
$script:SW_MAXIMIZE = 3
# SetWindowPos flags: no z-order change, show the window
$script:SWP_NOZORDER   = 0x0004
$script:SWP_SHOWWINDOW = 0x0040

function Get-CrossviewerDisplays {
    <#
      Returns one object per monitor with bounds, orientation and a flag marking the
      Session 0 pseudo-display so callers can fail loudly rather than silently laying
      windows out on a device that does not exist.
    #>
    $screens = [System.Windows.Forms.Screen]::AllScreens
    $out = foreach ($s in $screens) {
        $b = $s.Bounds
        [pscustomobject]@{
            device      = $s.DeviceName
            primary     = $s.Primary
            x           = $b.X
            y           = $b.Y
            width       = $b.Width
            height      = $b.Height
            orientation = if ($b.Width -ge $b.Height) { 'landscape' } else { 'portrait' }
            pixels      = [int64]$b.Width * [int64]$b.Height
        }
    }
    # The Session 0 pseudo-display is a single 1024x768 device; nothing real looks like that.
    $isSession0 = (@($out).Count -eq 1 -and $out[0].width -eq 1024 -and $out[0].height -eq 768)
    [pscustomobject]@{
        displays          = @($out)
        looks_like_session0 = $isSession0
    }
}

function Select-TargetDisplay {
    <#
      Picks the display to place viewer windows on, and RECORDS WHY. Order:
        1. an explicit device name, if the caller pinned one
        2. an exact landscape match for the requested resolution
        3. the largest landscape display
      Refuses to fall back to a portrait panel under any circumstance - a landscape drawing
      sheet reviewed on a portrait monitor is scaled down hard, which defeats the check.

      NOTE ON THE DEFAULT: mr-desktop has NO 3840x2160 monitor. Measured in Session 1 on
      2026-08-29 its real layout is DISPLAY1 2560x1440 landscape, DISPLAY2 1440x2560
      PORTRAIT, DISPLAY3 5120x1440 landscape (primary, an ultrawide). The "4K" figure seen
      over SSH came from Win32_VideoController's VideoModeDescription, which reports the
      Session 0 pseudo-mode rather than any attached panel - do not trust it for layout.
      The preferred resolution is kept as a parameter anyway so the intent survives if a
      real 4K panel is attached later; until then selection falls through to rule 3 and
      says so in `reason`.
    #>
    param(
        [Parameter(Mandatory = $true)]$Displays,
        [int]$PreferWidth  = 3840,
        [int]$PreferHeight = 2160,
        [string]$TargetDevice
    )
    $all = @($Displays)
    if ($TargetDevice) {
        $pinned = @($all | Where-Object { $_.device -eq $TargetDevice })
        if ($pinned.Count -eq 0) {
            throw "pinned display '$TargetDevice' not found (have: $(($all | ForEach-Object { $_.device }) -join ', '))"
        }
        if ($pinned[0].orientation -ne 'landscape') {
            throw "pinned display '$TargetDevice' is portrait - refusing; captures must be on a landscape panel"
        }
        return ($pinned[0] | Add-Member -NotePropertyName reason -NotePropertyValue 'pinned by caller' -PassThru -Force)
    }

    $landscape = @($all | Where-Object { $_.orientation -eq 'landscape' })
    if ($landscape.Count -eq 0) {
        throw "no landscape display found - refusing to capture on a portrait panel (found: $(($all | ForEach-Object { "$($_.device) $($_.width)x$($_.height) $($_.orientation)" }) -join '; '))"
    }

    $exact = @($landscape | Where-Object { $_.width -eq $PreferWidth -and $_.height -eq $PreferHeight })
    if ($exact.Count -gt 0) {
        return ($exact[0] | Add-Member -NotePropertyName reason -NotePropertyValue "exact match for requested ${PreferWidth}x${PreferHeight}" -PassThru -Force)
    }

    $best = ($landscape | Sort-Object -Property pixels -Descending)[0]
    $reason = "no ${PreferWidth}x${PreferHeight} display attached; fell back to the largest landscape display ($($best.width)x$($best.height))"
    return ($best | Add-Member -NotePropertyName reason -NotePropertyValue $reason -PassThru -Force)
}

function Move-WindowToDisplay {
    <#
      Places a window on the target display and maximises it there. Maximise alone is not
      enough: Windows maximises onto whichever monitor the window currently occupies, so
      the window must first be MOVED onto the target, then maximised.
    #>
    param(
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)]$Display,
        [switch]$Maximize = $true
    )
    if ($WindowHandle -eq [IntPtr]::Zero) { return $false }

    # Restore first - a window that is already maximised on another monitor ignores a move.
    [Crossviewer.NativeWindow]::ShowWindow($WindowHandle, $script:SW_RESTORE) | Out-Null
    Start-Sleep -Milliseconds 200

    $flags = $script:SWP_NOZORDER -bor $script:SWP_SHOWWINDOW
    # Inset slightly so the restored frame is unambiguously inside the target monitor
    # before maximising; exact bounds occasionally land a window on the neighbour.
    $ok = [Crossviewer.NativeWindow]::SetWindowPos(
        $WindowHandle, [IntPtr]::Zero,
        $Display.x + 40, $Display.y + 40,
        [int]($Display.width * 0.8), [int]($Display.height * 0.8),
        $flags)
    Start-Sleep -Milliseconds 300

    if ($Maximize) {
        [Crossviewer.NativeWindow]::ShowWindow($WindowHandle, $script:SW_MAXIMIZE) | Out-Null
        Start-Sleep -Milliseconds 300
    }
    return $ok
}
