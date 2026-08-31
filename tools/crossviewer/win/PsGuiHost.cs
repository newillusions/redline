// A GUI-subsystem host that runs a PowerShell script and tees every stream to a log file.
//
// WHY THIS EXISTS. On 2026-08-29 mr-desktop's interactive session (Session 1) stopped being
// able to start CONSOLE-subsystem processes: a scheduled task running cmd.exe or
// powershell.exe spawns the process and its conhost, both sit at one thread and zero CPU,
// and the command never executes - a trivial `cmd /c echo ok > file` never wrote its file
// in 75 seconds. A GUI-subsystem process launched the same way (wscript.exe) completed in
// three seconds with exit code 0. So process creation is fine; the CONSOLE subsystem in that
// session is wedged, and every leg of this harness runs through powershell.exe.
//
// powershell.exe is console-subsystem and always attaches a console, so it cannot be used
// until the session is reset (owner log off / on, or a reboot). This host is compiled
// /target:winexe - GUI subsystem, no console, no conhost - and runs the SAME leg scripts
// in-process through a PowerShell runspace. It is the difference between the harness being
// blocked on an owner round-trip and the harness running.
//
// Keep it even after the session is fixed: it costs one small binary and makes every future
// run immune to a class of failure that already cost this project a session.
//
// Build (from a context whose console works - an SSH session is fine, that is Session 0):
//   csc.exe /target:winexe /out:PsGuiHost.exe /r:"<System.Management.Automation.dll>" PsGuiHost.cs
//
// Usage:
//   PsGuiHost.exe <script.ps1> <logfile> [-Param value ...]
using System;
using System.IO;
using System.Management.Automation;
using System.Management.Automation.Runspaces;
using System.Text;

static class PsGuiHost
{
    static StreamWriter _log;

    static void Line(string s)
    {
        try { _log.WriteLine(s); _log.Flush(); } catch { }
    }

    static int Main(string[] args)
    {
        if (args.Length < 2) return 2;
        string script = args[0];
        string logPath = args[1];

        // Quote every forwarded argument that is not already a -Switch, so paths with spaces
        // survive. The leg scripts take -InputDir/-OutputDir style parameters.
        var sb = new StringBuilder();
        sb.Append("& '").Append(script.Replace("'", "''")).Append("'");
        for (int i = 2; i < args.Length; i++)
        {
            string a = args[i];
            if (a.StartsWith("-")) sb.Append(' ').Append(a);
            else sb.Append(" '").Append(a.Replace("'", "''")).Append("'");
        }
        string command = sb.ToString();

        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(logPath));
            // UTF-8 without a BOM: the Mac side parses these logs, and 5.1's usual BOM trips
            // strict parsers (the same trap the leg scripts avoid when writing JSON).
            _log = new StreamWriter(logPath, false, new UTF8Encoding(false));
        }
        catch { return 3; }

        Line("[PsGuiHost] " + DateTime.Now.ToString("o"));
        Line("[PsGuiHost] command: " + command);

        int exit = 0;
        try
        {
            using (Runspace rs = RunspaceFactory.CreateRunspace())
            {
                rs.Open();
                using (PowerShell ps = PowerShell.Create())
                {
                    ps.Runspace = rs;
                    ps.AddScript(command);

                    // Mirror every stream into the log as it arrives, so a leg that stalls
                    // still leaves a partial trace to read - a silent log is what made the
                    // original console wedge so hard to diagnose.
                    ps.Streams.Verbose.DataAdded  += (s, e) => Line("VERBOSE: " + ((PSDataCollection<VerboseRecord>)s)[e.Index]);
                    ps.Streams.Warning.DataAdded  += (s, e) => Line("WARNING: " + ((PSDataCollection<WarningRecord>)s)[e.Index]);
                    ps.Streams.Error.DataAdded    += (s, e) => Line("ERROR:   " + ((PSDataCollection<ErrorRecord>)s)[e.Index]);
                    ps.Streams.Information.DataAdded += (s, e) => Line(((PSDataCollection<InformationRecord>)s)[e.Index].ToString());

                    // The SUCCESS stream must be mirrored live too, not collected and
                    // dumped at the end. The legs report progress with Write-Output, so a
                    // buffered success stream means a leg that stalls leaves an EMPTY log
                    // and you cannot tell how far it got - which is exactly what happened
                    // on the first batch attempt and cost a diagnostic round.
                    var output = new PSDataCollection<PSObject>();
                    output.DataAdded += (s, e) => Line(((PSDataCollection<PSObject>)s)[e.Index] == null
                        ? "" : ((PSDataCollection<PSObject>)s)[e.Index].ToString());
                    // BeginInvoke/EndInvoke rather than Invoke: the overload that accepts an
                    // output collection is only on the async pair, and an output collection is
                    // what makes DataAdded fire as the leg runs.
                    IAsyncResult ar = ps.BeginInvoke<PSObject, PSObject>(null, output);
                    ps.EndInvoke(ar);

                    if (ps.HadErrors && ps.Streams.Error.Count > 0) exit = 1;
                }
            }
        }
        catch (Exception ex)
        {
            Line("FATAL: " + ex.ToString());
            exit = 4;
        }

        Line("[PsGuiHost] exit " + exit + " at " + DateTime.Now.ToString("o"));
        try { _log.Close(); } catch { }
        return exit;
    }
}
