using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Management;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using Microsoft.Win32;

internal static class Observer
{
    static bool residueInspectionFailed;
    const int WM_COMMAND = 0x0111, IDCANCEL = 2;
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out int pid);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window, int message, IntPtr wParam, IntPtr lParam);
    [DllImport("kernel32.dll")] static extern IntPtr GetConsoleWindow();

    static int Main(string[] args)
    {
        if (args.Length != 9) return 64;
        string installer = Path.GetFullPath(args[0]), output = Path.GetFullPath(args[1]);
        int timeout = Int32.Parse(args[2]);
        string architecture = args[3], sourceCommit = args[4], sourceTree = args[5], sourceTreeSha256 = args[6];
        string expectedHash = args[7], provenanceHash = args[8];
        string beforeHash = Hash(installer);
        if (!String.Equals(beforeHash, expectedHash, StringComparison.Ordinal)) return 65;
        FileInfo info = new FileInfo(installer);
        string[] residueBefore = Residue();
        List<string> starts = new List<string>();
        bool workerEventObserved = false, watcherFailed = false;
        int mediumPid = 0;
        ManagementEventWatcher watcher = new ManagementEventWatcher(new WqlEventQuery("SELECT * FROM Win32_ProcessStartTrace"));
        watcher.EventArrived += delegate(object sender, EventArrivedEventArgs value) {
            try { int pid = Convert.ToInt32(value.NewEvent["ProcessID"]), parent = Convert.ToInt32(value.NewEvent["ParentProcessID"]); lock (starts) { starts.Add(Convert.ToString(value.NewEvent["ProcessName"]) ?? ""); if (mediumPid != 0 && parent == mediumPid && pid != mediumPid) workerEventObserved = true; } }
            catch { lock (starts) watcherFailed = true; }
        };
        watcher.Stopped += delegate(object sender, StoppedEventArgs value) { if (value.Status != ManagementStatus.NoError) lock (starts) watcherFailed = true; };
        watcher.Start();
        Process medium = Process.Start(new ProcessStartInfo(installer) { UseShellExecute = true });
        if (medium == null) return 66;
        mediumPid = medium.Id; int workerPid = 0;
        bool windowObserved = false, cancelSent = false, pipeObserved = false;
        Stopwatch watch = Stopwatch.StartNew();
        while (!medium.HasExited && watch.ElapsedMilliseconds < timeout)
        {
            IntPtr dialog = FindWindowForProcess(mediumPid, "#32770");
            if (dialog != IntPtr.Zero)
            {
                windowObserved = true;
                cancelSent = PostMessage(dialog, WM_COMMAND, new IntPtr(IDCANCEL), IntPtr.Zero) || cancelSent;
            }
            workerPid = FindChild(mediumPid, workerPid);
            pipeObserved |= PipeExists("TalkingQuill.Setup." + mediumPid);
            Thread.Sleep(10);
        }
        Thread.Sleep(1000);
        watcher.Stop(); watcher.Dispose();
        if (!medium.HasExited) { try { medium.Kill(); } catch {} return 67; }
        Thread.Sleep(250);
        string afterHash = Hash(installer);
        string[] residueAfter = Residue();
        string[] changedResidue = residueAfter.Except(residueBefore, StringComparer.OrdinalIgnoreCase).ToArray();
        string[] processStarts; lock (starts) processStarts = starts.ToArray();
        string[] interpreters = processStarts.Where(IsInterpreter).ToArray();
        bool activeMedium = IsActive(mediumPid), activeWorker = workerPid != 0 && IsActive(workerPid);
        bool workerStarted, observerFailed; lock (starts) { workerStarted = workerPid != 0 || workerEventObserved; observerFailed = watcherFailed; }
        observerFailed |= residueInspectionFailed;
        bool passed = medium.ExitCode == 1223 && windowObserved && cancelSent && !workerStarted && !pipeObserved &&
            beforeHash == afterHash && changedResidue.Length == 0 && !activeMedium && !activeWorker && interpreters.Length == 0 && !observerFailed;
        string json = "{" +
            "\"schemaVersion\":4," +
            "\"installer\":" + Quote(Path.GetFileName(installer)) + ",\"architecture\":" + Quote(architecture) + "," +
            "\"sourceCommit\":" + Quote(sourceCommit) + ",\"sourceTree\":" + Quote(sourceTree) + ",\"sourceTreeSha256\":" + Quote(sourceTreeSha256) + "," +
            "\"installerProvenanceSha256\":" + Quote(expectedHash) + ",\"provenanceDocumentSha256\":" + Quote(provenanceHash) + "," +
            "\"installerSha256Before\":" + Quote(beforeHash) + ",\"installerSha256After\":" + Quote(afterHash) + ",\"bytes\":" + info.Length + "," +
            "\"outerPeSubsystem\":\"windows-gui\",\"outerPeSubsystemValue\":2," +
            "\"setupWindow\":{\"processId\":" + mediumPid + ",\"className\":\"#32770\"}," +
            "\"cancel\":{\"commandSent\":" + Bool(cancelSent) + ",\"exitCode\":" + medium.ExitCode + ",\"workerStarted\":" + Bool(workerStarted) + ",\"pipeObserved\":" + Bool(pipeObserved) + "}," +
            "\"processStarts\":" + Strings(processStarts) + ",\"interpreterProcessStarts\":" + Strings(interpreters) + ",\"observerErrors\":" + Strings(observerFailed ? new[]{"process-watcher-failed"} : new string[0]) + "," +
            "\"activeProcessesAfterTeardown\":" + Strings(new[]{activeMedium ? mediumPid.ToString() : null, activeWorker ? workerPid.ToString() : null}.Where(x => x != null).ToArray()) + "," +
            "\"residueBefore\":" + Strings(residueBefore) + ",\"residueAfter\":" + Strings(residueAfter) + ",\"newResidueAfterCancel\":" + Strings(changedResidue) + "," +
            "\"forcedCleanup\":false,\"passed\":" + Bool(passed) + "}";
        Directory.CreateDirectory(Path.GetDirectoryName(output));
        File.WriteAllText(output, json, new UTF8Encoding(false));
        return passed ? 0 : 68;
    }

    static IntPtr FindWindowForProcess(int pid, string cls) { IntPtr current = IntPtr.Zero; while ((current = FindWindowEx(IntPtr.Zero, current, cls, null)) != IntPtr.Zero) { int owner; GetWindowThreadProcessId(current, out owner); if (owner == pid) return current; } return IntPtr.Zero; }
    static int FindChild(int parent, int previous) { if (previous != 0) return previous; using (ManagementObjectSearcher search = new ManagementObjectSearcher("SELECT ProcessId,ParentProcessId FROM Win32_Process")) foreach (ManagementObject value in search.Get()) if (Convert.ToInt32(value["ParentProcessId"]) == parent) return Convert.ToInt32(value["ProcessId"]); return 0; }
    static bool PipeExists(string name) { try { return Directory.GetFiles(@"\\.\pipe\").Any(path => String.Equals(Path.GetFileName(path), name, StringComparison.OrdinalIgnoreCase)); } catch { residueInspectionFailed = true; return false; } }
    static bool IsActive(int pid) { try { return !Process.GetProcessById(pid).HasExited; } catch { return false; } }
    static bool IsInterpreter(string name) { string value = name.ToLowerInvariant(); return value == "powershell.exe" || value == "pwsh.exe" || value == "cmd.exe" || value == "wscript.exe" || value == "cscript.exe"; }
    static string[] Residue() { List<string> result = new List<string>(); string pf = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), pd = Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData); foreach (string path in new[]{Path.Combine(pf,"Talking Quill"),Path.Combine(pf,".Talking Quill.native-staging"),Path.Combine(pf,".Talking Quill.native-backup"),Path.Combine(pf,".Talking Quill.native-transaction-v2.json"),Path.Combine(pf,"Talking Quill Maintenance.exe"),Path.Combine(pd,"Talking Quill Update Recovery"),Path.Combine(pd,"Talking Quill","KeyboardAuthority"),Path.Combine(pd,"Talking Quill",".KeyboardAuthority.retirement-quarantine"),Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System),"Tasks","TalkingQuillKeyboardAuthority")}) if (Directory.Exists(path) || File.Exists(path)) result.Add(path); foreach (string path in Directory.GetFiles(pf,"Talking Quill Maintenance-*.exe",SearchOption.TopDirectoryOnly)) result.Add(path); foreach (string path in Directory.GetDirectories(pd,".Talking Quill.update-*",SearchOption.TopDirectoryOnly).Concat(Directory.GetDirectories(pd,".Talking Quill.machine-lock-*",SearchOption.TopDirectoryOnly)).Concat(Directory.GetDirectories(pd,".Talking Quill.uninstall-finalizer-*",SearchOption.TopDirectoryOnly)).ToArray()) result.Add(path); using (ManagementObjectSearcher services = new ManagementObjectSearcher("SELECT Name FROM Win32_Service WHERE Name='TalkingQuillKeyboardAuthority'")) if (services.Get().Count != 0) result.Add("SCM:TalkingQuillKeyboardAuthority"); try { ManagementScope scope = new ManagementScope(@"\\.\root\Microsoft\Windows\TaskScheduler"); scope.Connect(); using (ManagementObjectSearcher tasks = new ManagementObjectSearcher(scope, new ObjectQuery("SELECT TaskName FROM MSFT_ScheduledTask WHERE TaskName='TalkingQuillKeyboardAuthority'"))) if (tasks.Get().Count != 0) result.Add("TASK:TalkingQuillKeyboardAuthority"); } catch { residueInspectionFailed = true; } using (RegistryKey lockKey = Registry.LocalMachine.OpenSubKey(@"Software\Talking Quill\RecoveryStateLockV1")) if (lockKey != null) result.Add("HKLM:machine-lock"); using (RegistryKey uninstall = Registry.LocalMachine.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill")) if (uninstall != null) result.Add("HKLM:uninstall"); using (RegistryKey app = Registry.LocalMachine.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe")) if (app != null) result.Add("HKLM:app-path"); return result.OrderBy(x=>x,StringComparer.OrdinalIgnoreCase).ToArray(); }
    static string Hash(string path) { using (SHA256 sha = SHA256.Create()) using (FileStream stream = File.OpenRead(path)) return BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", "").ToLowerInvariant(); }
    static string Quote(string value) { return "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\""; }
    static string Strings(string[] values) { return "[" + String.Join(",", values.Select(Quote)) + "]"; }
    static string Bool(bool value) { return value ? "true" : "false"; }
}
