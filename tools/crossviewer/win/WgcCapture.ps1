<#
.SYNOPSIS
  Windows.Graphics.Capture (WGC) window capture - the "robust path" for composited
  surfaces, kept separate from Capture.ps1 and dot-sourced lazily.

.DESCRIPTION
  Provides Save-WindowCaptureWgc: captures a window via the WinRT Windows.Graphics.Capture
  API, which asks the DWM/GPU compositor directly for a window's real composited surface -
  the same mechanism the Windows Snipping Tool / Xbox Game Bar use to capture windows that
  BitBlt (Save-WindowCaptureCopyFromScreen) and PrintWindow can both fail against (a window
  using a hardware overlay / independent-flip swap chain bypasses the desktop's own
  composition surface entirely, which is what BitBlt reads; PrintWindow depends on the
  target window cooperating with PW_RENDERFULLCONTENT, which not every renderer honours).

  EXPERIMENTAL - READ BEFORE WIRING THIS INTO ANY LEG'S DEFAULT PATH.
  This file has been proven to COMPILE (Add-Type succeeds; verified 2026-09-04 via a
  parse/compile-only check over SSH to mr-desktop, no window captured, no Acrobat/Revu
  process touched) but has NOT been proven to CAPTURE CORRECTLY - that needs a live run
  against a real composited window on the actual hardware, which this dispatch was
  explicitly scoped to NOT perform (code + unit-level checks only). Run
  CaptureSelfTest.ps1 first, on an idle machine, before trusting this for any real
  diagnostic capture. Until a self-test run PASSES, treat any Wgc-labelled result as
  UNVERIFIED, not as evidence the composited-surface hypothesis is right or wrong.

  WHY THIS IS ITS OWN FILE, LAZILY LOADED: the WinRT/Direct3D types this needs are heavier
  to activate than the plain user32 P/Invokes in Capture.ps1, and every leg dot-sources
  Capture.ps1 today. Splitting this out means AcrobatLeg.ps1's default Auto path (which
  does not use Wgc - see Capture.ps1's Save-WindowCapture header) never pays that cost, and
  a failure to activate WinRT types on some future machine cannot break a leg that never
  asked for this method. Save-WindowCapture dot-sources this file on demand, only when
  -Method Wgc is actually requested.

  DESIGN CHOICE THAT KEEPS THIS SAFE TO SHIP UNTESTED: every risky, hand-declared COM
  interop surface in classic "raw WGC from C#" samples is a big interface (ID3D11Device has
  43 methods; ID3D11DeviceContext has 100+) where getting one method's VTABLE POSITION
  wrong causes silent memory corruption that C#/PowerShell try/catch CANNOT catch (a true
  vtable mismatch is a hard native crash, not a catchable exception) - not something to
  hand-type from memory without a compiler AND a real GPU to verify against. This
  implementation avoids that entire class of risk:
    - ID3D11Device and IDXGIDevice are declared as MARKER interfaces with ZERO methods -
      never called, only used for COM identity (QueryInterface/marshalling), which is safe
      regardless of any method's position because no method is ever invoked through them.
    - The GPU→CPU pixel copy goes through Windows.Graphics.Imaging.SoftwareBitmap's WinRT-
      projected CreateCopyFromSurfaceAsync, not a hand-rolled CopyResource/Map/Unmap
      sequence against ID3D11DeviceContext - eliminating the need to know that interface's
      vtable at all.
    - The ONLY hand-declared interface with a real method call is
      IGraphicsCaptureItemInterop.CreateForWindow, a single method at the first vtable slot
      after IUnknown, in a small and stable interop header - about as low-risk as raw COM
      interop gets. A wrong GUID there fails the call cleanly (E_NOINTERFACE, a checked
      HRESULT) rather than corrupting anything; it does not carry the same blast radius as
      a wrong vtable position on a big interface.
    - D3D11CreateDevice and CreateDirect3D11DeviceFromDXGIDevice are flat DLL exports (not
      vtable calls) - their risk is limited to an ordinary wrong-signature P/Invoke, which
      fails as a normal catchable exception or a checked HRESULT, never silently.
  Every fallible step below checks its HRESULT/return value explicitly before proceeding;
  nothing assumes success. Any exception is caught by the caller
  (Save-WindowCapture / CaptureSelfTest.ps1) and turned into a clean $null / reported
  failure - this function never expects to bring a leg down.

.NOTES
  GUIDs used (documented, stable, unchanged since these APIs shipped):
    IID_IDXGIDevice                   54ec77fa-1377-44e6-8c32-88fd5f44c84c
    IID_ID3D11Device                  db6f6ddb-ac77-4e88-8253-819df9bbf140
    IID_IGraphicsCaptureItemInterop   3628E81B-3CAC-4C60-B7F4-23CE0E0C3356
    IID_IGraphicsCaptureItem          79C3F95B-31F7-4EC2-A464-632EF5D30760
  If a self-test run reports CreateForWindow failing with E_NOINTERFACE specifically, that
  points at the IGraphicsCaptureItem IID above - re-verify it against
  windows.graphics.capture.interop.h before anything else.

  MEASURED 2026-09-04 (mr-desktop, parse/compile-only check, no window touched): the C#
  Add-Type block above compiles cleanly on the real target machine and all four custom
  types plus both PowerShell functions load - real, on-machine evidence, not just
  syntax-checked in isolation. All seven WinRT capture types also resolve via the
  ContentType=WindowsRuntime activation (WINRT_TYPES_OK).

  BUT: probed from that SAME SSH session, [GraphicsCaptureSession]::IsSupported() threw
  "The specified service does not exist as an installed service." This is UNVERIFIED as a
  real blocker rather than a test artifact - it was probed from Session 0 (an SSH shell has
  no window station), and every other COM/GUI call in this harness (Acrobat's own IAC,
  first and foremost - see AcrobatLeg.ps1's own header) is documented to fail or hang from
  Session 0 for exactly that reason. The Windows Graphics Capture service plausibly needs
  the same interactive desktop session this harness already runs its Session-1 scheduled
  tasks under. NOT YET TESTED from Session 1 - that is CaptureSelfTest.ps1's first job, and
  its result is the thing that actually answers whether this failure is a session artifact
  or a real platform gap. Do not treat this note as proof either way.
#>

if (-not ('Crossviewer.Wgc' -as [type])) {
    Add-Type -AssemblyName System.Runtime.WindowsRuntime -ErrorAction SilentlyContinue

    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace Crossviewer {
    // Marker-only: no method is ever invoked through either interface. Only used so the
    // CLR can marshal/QueryInterface COM pointers by identity. See this file's header for
    // why that makes their vtable layout irrelevant to correctness.
    [ComImport, Guid("54ec77fa-1377-44e6-8c32-88fd5f44c84c"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IDXGIDevice { }

    [ComImport, Guid("db6f6ddb-ac77-4e88-8253-819df9bbf140"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface ID3D11Device { }

    // windows.graphics.capture.interop.h. CreateForWindow is the FIRST method after
    // IUnknown in this interface (vtable slot 3) - a small, stable, specifically-documented
    // header, not a large device-style interface. CreateForMonitor follows it and is
    // deliberately not declared, since it is never called.
    [ComImport, Guid("3628E81B-3CAC-4C60-B7F4-23CE0E0C3356"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IGraphicsCaptureItemInterop {
        [PreserveSig]
        int CreateForWindow(IntPtr window, [In] ref Guid iid, out IntPtr result);
    }

    public static class Wgc {
        // Flat DLL exports, not vtable calls - a wrong signature fails as an ordinary
        // catchable exception, never silently.
        [DllImport("d3d11.dll")]
        public static extern int D3D11CreateDevice(
            IntPtr pAdapter, int driverType, IntPtr software, uint flags,
            IntPtr pFeatureLevels, uint featureLevels, uint sdkVersion,
            out ID3D11Device device, out int pFeatureLevel, out IntPtr immediateContext);

        [DllImport("d3d11.dll")]
        public static extern int CreateDirect3D11DeviceFromDXGIDevice(
            IDXGIDevice dxgiDevice, out IntPtr graphicsDevice);

        [DllImport("ole32.dll")]
        public static extern void CoTaskMemFree(IntPtr ptr);

        public const int D3D_DRIVER_TYPE_HARDWARE = 1;
        public const uint D3D11_CREATE_DEVICE_BGRA_SUPPORT = 0x20;
        public const uint D3D11_SDK_VERSION = 7;

        public static readonly Guid IID_IGraphicsCaptureItem =
            new Guid("79C3F95B-31F7-4EC2-A464-632EF5D30760");
    }
}
'@ -ReferencedAssemblies System.Runtime.WindowsRuntime -ErrorAction Stop
}

function Save-WindowCaptureWgc {
    <#
      See this file's header for the full design and risk notes. Captures exactly ONE frame
      of the given window via Windows.Graphics.Capture and saves it as a PNG. Returns the
      path on success, $null on ANY failure - every step is guarded so a failure here is
      always a clean "this method did not work this time", never a leg-ending exception.
    #>
    param(
        [Parameter(Mandatory = $true)][IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][string]$Path,
        [int]$TimeoutMs = 4000
    )
    if (-not [Crossviewer.Win]::IsWindow($WindowHandle)) { return $null }

    # Forces the CLR to resolve these WinRT types via C:\Windows\System32\WinMetadata -
    # the .NET Framework's built-in (pre-C++/WinRT-era) desktop-to-WinRT interop, which
    # Windows PowerShell 5.1 (the runtime every leg is written for) supports natively.
    try {
        [Windows.Graphics.Capture.GraphicsCaptureItem, Windows.Graphics.Capture, ContentType = WindowsRuntime] | Out-Null
        [Windows.Graphics.Capture.Direct3D11CaptureFramePool, Windows.Graphics.Capture, ContentType = WindowsRuntime] | Out-Null
        [Windows.Graphics.Capture.GraphicsCaptureSession, Windows.Graphics.Capture, ContentType = WindowsRuntime] | Out-Null
        [Windows.Graphics.DirectX.DirectXPixelFormat, Windows.Graphics.DirectX, ContentType = WindowsRuntime] | Out-Null
        [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType = WindowsRuntime] | Out-Null
        [Windows.Graphics.Imaging.BitmapEncoder, Windows.Graphics.Imaging, ContentType = WindowsRuntime] | Out-Null
        [Windows.Storage.Streams.InMemoryRandomAccessStream, Windows.Storage.Streams, ContentType = WindowsRuntime] | Out-Null
    } catch {
        Write-Output "[wgc] WinRT capture types unavailable on this OS build: $($_.Exception.Message)"
        return $null
    }

    if (-not [Windows.Graphics.Capture.GraphicsCaptureSession]::IsSupported()) {
        Write-Output '[wgc] GraphicsCaptureSession.IsSupported() = false - this Windows build/session cannot use WGC'
        return $null
    }

    $device = $null; $ctx = [IntPtr]::Zero; $winrtDevicePtr = [IntPtr]::Zero
    $framePool = $null; $session = $null; $item = $null
    try {
        # 1. A throwaway D3D11 device. BGRA_SUPPORT is required for WinRT interop; we never
        #    call a method on the returned interface (marker-only, see file header) beyond
        #    releasing the immediate-context pointer this signature also hands back.
        $hr = [Crossviewer.Wgc]::D3D11CreateDevice(
            [IntPtr]::Zero, [Crossviewer.Wgc]::D3D_DRIVER_TYPE_HARDWARE, [IntPtr]::Zero,
            [Crossviewer.Wgc]::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            [IntPtr]::Zero, 0, [Crossviewer.Wgc]::D3D11_SDK_VERSION,
            [ref]$device, [ref]0, [ref]$ctx)
        if ($hr -ne 0) { Write-Output "[wgc] D3D11CreateDevice hr=0x$($hr.ToString('X8'))"; return $null }
        if ($ctx -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::Release($ctx) | Out-Null }

        # 2. Wrap it as the WinRT IDirect3DDevice the capture pipeline needs.
        $dxgiDevice = [Crossviewer.IDXGIDevice]$device
        $hr = [Crossviewer.Wgc]::CreateDirect3D11DeviceFromDXGIDevice($dxgiDevice, [ref]$winrtDevicePtr)
        if ($hr -ne 0) { Write-Output "[wgc] CreateDirect3D11DeviceFromDXGIDevice hr=0x$($hr.ToString('X8'))"; return $null }
        $winrtDevice = [Runtime.InteropServices.Marshal]::GetObjectForIUnknown($winrtDevicePtr)

        # 3. GraphicsCaptureItem for this specific HWND, via the one hand-declared
        #    interface method this function calls (see header: low-risk position/GUID).
        #    The activation factory for the GraphicsCaptureItem runtime class implements
        #    IGraphicsCaptureItemInterop (this is the documented desktop-interop pattern for
        #    WGC - the runtime class itself has no public constructor, only this factory
        #    method, precisely because "capture item for an HWND" is a desktop-only concept
        #    WinRT's own activation system does not otherwise expose).
        $interopObj = [System.Runtime.InteropServices.WindowsRuntime.WindowsRuntimeMarshal]::GetActivationFactory([Windows.Graphics.Capture.GraphicsCaptureItem])
        $itemIid = [Crossviewer.Wgc]::IID_IGraphicsCaptureItem
        $itemPtr = [IntPtr]::Zero
        $hr = ([Crossviewer.IGraphicsCaptureItemInterop]$interopObj).CreateForWindow($WindowHandle, [ref]$itemIid, [ref]$itemPtr)
        if ($hr -ne 0) { Write-Output "[wgc] IGraphicsCaptureItemInterop.CreateForWindow hr=0x$($hr.ToString('X8'))"; return $null }
        $item = [Runtime.InteropServices.Marshal]::GetObjectForIUnknown($itemPtr)
        [Runtime.InteropServices.Marshal]::Release($itemPtr) | Out-Null

        $size = $item.Size
        if ($size.Width -le 0 -or $size.Height -le 0) { Write-Output '[wgc] capture item reports zero size'; return $null }

        # 4. One-buffer free-threaded pool + session, no cursor, single-frame capture.
        $pixelFormat = [Windows.Graphics.DirectX.DirectXPixelFormat]::B8G8R8A8UIntNormalized
        $framePool = [Windows.Graphics.Capture.Direct3D11CaptureFramePool]::CreateFreeThreaded($winrtDevice, $pixelFormat, 1, $size)
        $session = $framePool.CreateCaptureSession($item)
        try { $session.IsCursorCaptureEnabled = $false } catch { }
        $session.StartCapture()

        $frame = $null
        $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
        while ([DateTime]::UtcNow -lt $deadline) {
            $frame = $framePool.TryGetNextFrame()
            if ($null -ne $frame) { break }
            Start-Sleep -Milliseconds 100
        }
        if ($null -eq $frame) { Write-Output "[wgc] no frame arrived within ${TimeoutMs}ms"; return $null }

        # 5. Frame surface -> SoftwareBitmap (WinRT-projected GPU->CPU copy - no hand-rolled
        #    CopyResource/Map/Unmap against ID3D11DeviceContext needed, see file header)
        #    -> PNG via BitmapEncoder into an in-memory stream, then to disk.
        $swBmpOp = [Windows.Graphics.Imaging.SoftwareBitmap]::CreateCopyFromSurfaceAsync($frame.Surface)
        $swBmp = Wait-WinRtAsync -Operation $swBmpOp -TimeoutMs 5000
        $frame.Dispose()
        if ($null -eq $swBmp) { Write-Output '[wgc] CreateCopyFromSurfaceAsync produced no bitmap'; return $null }

        $stream = New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
        $encoderId = [Windows.Graphics.Imaging.BitmapEncoder]::PngEncoderId
        $encoderOp = [Windows.Graphics.Imaging.BitmapEncoder]::CreateAsync($encoderId, $stream)
        $encoder = Wait-WinRtAsync -Operation $encoderOp -TimeoutMs 5000
        $encoder.SetSoftwareBitmap($swBmp)
        Wait-WinRtAsync -Operation $encoder.FlushAsync() -TimeoutMs 5000 -NoResult | Out-Null

        $dir = Split-Path -Parent $Path
        if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        $stream.Seek([uint64]0) | Out-Null
        # System.IO.WindowsRuntimeStreamExtensions (System.Runtime.WindowsRuntime.dll) -
        # the standard .NET Framework bridge from a WinRT IRandomAccessStream to a normal
        # managed Stream, not a hand-rolled one.
        $netStream = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($stream)
        $fileStream = [System.IO.File]::Create($Path)
        try { $netStream.CopyTo($fileStream) } finally { $fileStream.Dispose(); $netStream.Dispose() }
        return $Path
    } catch {
        Write-Output "[wgc] capture failed: $($_.Exception.Message)"
        return $null
    } finally {
        try { if ($null -ne $session) { $session.Dispose() } } catch { }
        try { if ($null -ne $framePool) { $framePool.Dispose() } } catch { }
    }
}

function Wait-WinRtAsync {
    <#
      PowerShell has no 'await'. This polls a WinRT IAsyncOperation's Status rather than
      using .AsTask() (which needs an explicit generic-method invocation via reflection to
      call from PowerShell, and is easy to get subtly wrong) - a simple, slower, but
      obviously-correct wait for a single-shot capture where sub-second latency does not
      matter. -NoResult is for IAsyncAction, which has no .GetResults().
    #>
    param(
        [Parameter(Mandatory = $true)]$Operation,
        [int]$TimeoutMs = 5000,
        [switch]$NoResult
    )
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        # AsyncStatus: Started=0, Completed=1, Canceled=2, Error=3
        if ($Operation.Status -ne 0) { break }
        Start-Sleep -Milliseconds 50
    }
    if ($Operation.Status -ne 1) { throw "WinRT async operation did not complete (status=$($Operation.Status))" }
    if ($NoResult) { return $null }
    return $Operation.GetResults()
}
