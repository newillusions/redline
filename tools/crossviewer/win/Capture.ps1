<#
.SYNOPSIS
  Window enumeration, screen capture and polite window-close helpers for the harness.

.DESCRIPTION
  Dot-source from a leg script. Provides:
    Enable-DpiAwareness      - make coordinates physical pixels, not scaled ones
    Get-ProcessWindows       - every visible top-level window of a process, with class + title
    Get-ProcessDialogs       - just the ones that look like dialogs (#32770)
    Save-WindowCapture       - screenshot a window's screen rectangle to PNG
    Get-CaptureHash          - hash a capture so "has it finished rendering?" is answerable
    Close-WindowPolitely     - WM_CLOSE, then wait; never terminates a process

.NOTES
  WHY CopyFromScreen AND NOT PrintWindow: Revu composites through the GPU, and PrintWindow
  against a hardware-accelerated document view returns a blank or partially-blank bitmap.
  Capturing the screen rectangle the window occupies gets what is actually on the panel,
  which is the whole point of a visual check. The cost is that the window must be foreground
  and unobstructed - the leg maximises and foregrounds it before every capture.

  WHY DPI AWARENESS MATTERS: an unaware process is lied to by Windows - Screen.AllScreens
  reports scaled logical pixels while CopyFromScreen addresses physical ones, so on a scaled
  display the capture silently lands on the wrong rectangle or crops. SetProcessDPIAware must
  be called before any window or screen coordinate is read.
#>

Add-Type -AssemblyName System.Windows.Forms -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.Drawing -ErrorAction SilentlyContinue

if (-not ('Crossviewer.Win' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace Crossviewer {
    public struct WinRect { public int Left; public int Top; public int Right; public int Bottom; }
    public struct POINT { public int X; public int Y; }

    public class WinInfo {
        public IntPtr Handle;
        public string Title;
        public string ClassName;
        public int Left, Top, Width, Height;
    }

    public static class Win {
        [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
        [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
        [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
        [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out WinRect r);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
        [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
        [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
        [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
        [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
        [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
        [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
        [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
        [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
        [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();

        // Z-order pseudo-handles and SetWindowPos flags.
        public static readonly IntPtr HWND_TOPMOST   = new IntPtr(-1);
        public static readonly IntPtr HWND_NOTOPMOST = new IntPtr(-2);
        public const uint SWP_NOSIZE = 0x0001, SWP_NOMOVE = 0x0002, SWP_NOACTIVATE = 0x0010, SWP_SHOWWINDOW = 0x0040;
        public const uint GA_ROOT = 2;

        // Which top-level window owns a screen point. GetAncestor(GA_ROOT) lifts the result
        // from whatever child control is under the cursor to the frame window we can compare
        // against a handle we hold.
        public static IntPtr RootWindowAt(int x, int y) {
            POINT p; p.X = x; p.Y = y;
            IntPtr h = WindowFromPoint(p);
            if (h == IntPtr.Zero) return IntPtr.Zero;
            IntPtr root = GetAncestor(h, GA_ROOT);
            return root == IntPtr.Zero ? h : root;
        }

        public static int ProcessIdOf(IntPtr h) {
            uint pid; GetWindowThreadProcessId(h, out pid); return (int)pid;
        }

        public static string ClassOf(IntPtr h) {
            var c = new StringBuilder(256); GetClassNameW(h, c, c.Capacity); return c.ToString();
        }

        private delegate bool EnumProc(IntPtr h, IntPtr lp);
        [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc cb, IntPtr lp);

        public const uint WM_CLOSE = 0x0010;

        // Visible top-level windows owned by one process, with the details a leg needs to
        // tell "the document window" from "a dialog blocking it".
        public static List<WinInfo> WindowsOfProcess(int pid) {
            var found = new List<WinInfo>();
            EnumWindows(delegate(IntPtr h, IntPtr lp) {
                uint wpid;
                GetWindowThreadProcessId(h, out wpid);
                if ((int)wpid != pid) return true;
                if (!IsWindowVisible(h)) return true;
                var t = new StringBuilder(512); GetWindowTextW(h, t, t.Capacity);
                var c = new StringBuilder(256); GetClassNameW(h, c, c.Capacity);
                WinRect r; GetWindowRect(h, out r);
                // Zero-area windows are message-only or collapsed helpers, never anything to look at.
                if (r.Right - r.Left <= 0 || r.Bottom - r.Top <= 0) return true;
                found.Add(new WinInfo {
                    Handle = h, Title = t.ToString(), ClassName = c.ToString(),
                    Left = r.Left, Top = r.Top, Width = r.Right - r.Left, Height = r.Bottom - r.Top
                });
                return true;
            }, IntPtr.Zero);
            return found;
        }
    }
}
'@ -ReferencedAssemblies System.Drawing, System.Windows.Forms
}

function Enable-DpiAwareness {
    try { [Crossviewer.Win]::SetProcessDPIAware() | Out-Null } catch { }
}

function Get-ProcessWindows {
    param([Parameter(Mandatory = $true)][int]$ProcessId)
    return @([Crossviewer.Win]::WindowsOfProcess($ProcessId))
}

function Get-ProcessDialogs {
    <#
      A Win32 dialog is class #32770. Revu's licence, update and recovery prompts are all
      standard dialogs, so this catches them without knowing their titles in advance. The
      main document window is excluded by handle so a legitimately-dialog-classed main
      window (some apps do this) is not mistaken for a prompt.
    #>
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [IntPtr]$ExcludeHandle = [IntPtr]::Zero
    )
    return @(Get-ProcessWindows -ProcessId $ProcessId |
        Where-Object { $_.ClassName -eq '#32770' -and $_.Handle -ne $ExcludeHandle })
}

function Set-WindowTopmost {
    <#
      Forces a window above every other non-topmost window, WITHOUT needing foreground
      rights. This is the load-bearing half of making a screen capture trustworthy:
      SetForegroundWindow is refused outright for a process that is not already in the
      foreground (Windows only flashes its taskbar button), so a leg driven from a
      scheduled task cannot raise a viewer that way. SetWindowPos to HWND_TOPMOST is not
      subject to that restriction.
      Always pair the $true call with a $false call in a finally block - a window left
      topmost stays pinned over everything the owner does afterwards.
    #>
    param(
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [bool]$On = $true
    )
    $after = if ($On) { [Crossviewer.Win]::HWND_TOPMOST } else { [Crossviewer.Win]::HWND_NOTOPMOST }
    $flags = [Crossviewer.Win]::SWP_NOMOVE -bor [Crossviewer.Win]::SWP_NOSIZE -bor [Crossviewer.Win]::SWP_SHOWWINDOW
    return [Crossviewer.Win]::SetWindowPos($WindowHandle, $after, 0, 0, 0, 0, $flags)
}

function Set-WindowForeground {
    <#
      Best-effort raise: topmost, bring to top, then ask for foreground. The first call is
      the one that actually works from a background process; the other two are free and
      help when the process does happen to hold foreground rights.
    #>
    param([Parameter(Mandatory = $true)][IntPtr]$WindowHandle)
    Set-WindowTopmost -WindowHandle $WindowHandle -On $true | Out-Null
    [Crossviewer.Win]::BringWindowToTop($WindowHandle) | Out-Null
    [Crossviewer.Win]::SetForegroundWindow($WindowHandle) | Out-Null
    Start-Sleep -Milliseconds 400
}

function Test-WindowUnobstructed {
    <#
      Proves, from the PIXELS' side, that the window we are about to photograph is the one
      actually on top of its own rectangle.

      WHY THIS EXISTS - incident 2026-08-29: the first capture run produced a perfectly
      valid 415 KB PNG of Microsoft Teams. Acrobat had been moved and maximised onto the
      target display, but SetForegroundWindow is denied to a background process, so the
      owner's Teams window stayed on top of that rectangle and CopyFromScreen faithfully
      photographed it. Every check the leg made had passed: the window handle was real, the
      rectangle was real, the PNG was large and well-formed. Nothing but looking at the
      image revealed it was the wrong application - and on a workstation the owner is using,
      the wrong application is somebody's private correspondence.

      So a capture is never trusted on the strength of a handle. Sample points spread across
      the rectangle are resolved through WindowFromPoint + GetAncestor(GA_ROOT) and must come
      back as OUR handle. Corners are deliberately inset: rounded borders and drop shadows at
      the exact corner resolve to the desktop even for a perfectly unobstructed window.

      The blocker is reported by window CLASS and PROCESS NAME only - never its title. A
      title like "<person> | Microsoft Teams" would put the owner's correspondent into a log
      file, which is the very exposure this function exists to prevent.
    #>
    param([Parameter(Mandatory = $true)][IntPtr]$WindowHandle)

    $r = New-Object Crossviewer.WinRect
    if (-not [Crossviewer.Win]::GetWindowRect($WindowHandle, [ref]$r)) {
        return [pscustomobject]@{ unobstructed = $false; hits = 0; total = 0; blocker = 'no window rect' }
    }
    $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
    if ($w -le 0 -or $h -le 0) {
        return [pscustomobject]@{ unobstructed = $false; hits = 0; total = 0; blocker = 'zero-area window' }
    }

    # Centre plus four inset quadrant points.
    $points = @(
        @{ x = $r.Left + [int]($w * 0.5); y = $r.Top + [int]($h * 0.5) },
        @{ x = $r.Left + [int]($w * 0.2); y = $r.Top + [int]($h * 0.2) },
        @{ x = $r.Left + [int]($w * 0.8); y = $r.Top + [int]($h * 0.2) },
        @{ x = $r.Left + [int]($w * 0.2); y = $r.Top + [int]($h * 0.8) },
        @{ x = $r.Left + [int]($w * 0.8); y = $r.Top + [int]($h * 0.8) }
    )

    $hits = 0
    $blocker = $null
    foreach ($pt in $points) {
        $root = [Crossviewer.Win]::RootWindowAt($pt.x, $pt.y)
        if ($root -eq $WindowHandle) {
            $hits++
        } elseif ($null -eq $blocker) {
            $cls = ''
            $proc = ''
            try { $cls = [Crossviewer.Win]::ClassOf($root) } catch { }
            $bpid = -1
            try {
                $bpid = [Crossviewer.Win]::ProcessIdOf($root)
                $bp = Get-Process -Id $bpid -ErrorAction SilentlyContinue
                if ($bp) { $proc = $bp.ProcessName }
            } catch { }
            # A window belonging to the SAME process we are capturing is the application's
            # own dialog - naming it is both safe and the only way to diagnose which prompt
            # is interrupting. A FOREIGN window's title stays omitted: that is the owner's
            # business and the exposure this function exists to prevent.
            $title = ''
            try {
                $selfPid = [Crossviewer.Win]::ProcessIdOf($WindowHandle)
                if ($bpid -eq $selfPid) {
                    $tb = New-Object System.Text.StringBuilder 512
                    [Crossviewer.Win]::GetWindowTextW($root, $tb, $tb.Capacity) | Out-Null
                    $title = " title='$($tb.ToString())'"
                }
            } catch { }
            $blocker = "class=$cls process=$proc$title"
        }
    }

    return [pscustomobject]@{
        unobstructed = ($hits -eq $points.Count)
        hits         = $hits
        total        = $points.Count
        blocker      = $blocker
    }
}

function Save-WindowCapture {
    <#
      Screenshots the screen rectangle a window occupies. Returns the path, or $null if the
      window has gone away. Clamps to the virtual desktop so a window hanging off the edge
      of a monitor does not throw.
    #>
    param(
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][string]$Path,
        [switch]$Foreground,
        # Refuse to write anything unless the window provably owns its own rectangle.
        # Callers photographing a shared desktop should ALWAYS pass this.
        [switch]$VerifyOnTop
    )
    if (-not [Crossviewer.Win]::IsWindow($WindowHandle)) { return $null }
    if ($Foreground) { Set-WindowForeground -WindowHandle $WindowHandle }
    if ($VerifyOnTop) {
        $check = Test-WindowUnobstructed -WindowHandle $WindowHandle
        if (-not $check.unobstructed) { return $null }
    }
    $r = New-Object Crossviewer.WinRect
    if (-not [Crossviewer.Win]::GetWindowRect($WindowHandle, [ref]$r)) { return $null }

    $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $left   = [Math]::Max($r.Left,   $vs.Left)
    $top    = [Math]::Max($r.Top,    $vs.Top)
    $right  = [Math]::Min($r.Right,  $vs.Right)
    $bottom = [Math]::Min($r.Bottom, $vs.Bottom)
    $w = $right - $left; $h = $bottom - $top
    if ($w -le 0 -or $h -le 0) { return $null }

    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    try {
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        try {
            $g.CopyFromScreen($left, $top, 0, 0, (New-Object System.Drawing.Size($w, $h)))
        } finally { $g.Dispose() }
        $dir = Split-Path -Parent $Path
        if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $bmp.Dispose() }
    return $Path
}

function Get-CaptureHash {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    return (Get-FileHash -LiteralPath $Path -Algorithm MD5).Hash
}

function Close-WindowPolitely {
    <#
      Posts WM_CLOSE and waits for the process to go. Returns a result object; NEVER calls
      Stop-Process. A viewer that refuses to close is a finding to report (usually it means
      a modal dialog is up), not something to force.
    #>
    param(
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [int]$TimeoutSec = 30
    )
    [Crossviewer.Win]::PostMessageW($WindowHandle, [Crossviewer.Win]::WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if (-not $p) { return [pscustomobject]@{ closed = $true; blocked_by_dialog = $false; dialogs = @() } }
    }
    $dialogs = @(Get-ProcessDialogs -ProcessId $ProcessId -ExcludeHandle $WindowHandle)
    return [pscustomobject]@{ closed = $false; blocked_by_dialog = ($dialogs.Count -gt 0); dialogs = $dialogs }
}
