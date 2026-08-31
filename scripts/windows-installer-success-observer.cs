using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Management;
using System.Text;
using System.Threading;

internal static class SuccessfulSetupObserver
{
    static int Main(string[] args)
    {
        if (args.Length != 8) return 64;
        string installer = Path.GetFullPath(args[0]), output = Path.GetFullPath(args[1]);
        int timeout = Int32.Parse(args[2]);
        string architecture = args[3], sourceCommit = args[4], sourceTree = args[5], packageMode = args[6], expectedHash = args[7];
        string installerHash = Hash(installer);
        if (!String.Equals(installerHash, expectedHash, StringComparison.Ordinal)) return 68;
        string processName = Path.GetFileName(installer);
        List<int> setupPids = new List<int>();
        List<string> interpreters = new List<string>();
        bool watcherFailed = false, pipeObserved = false;
        using (ManagementEventWatcher watcher = new ManagementEventWatcher(new WqlEventQuery("SELECT ProcessName, ProcessID FROM Win32_ProcessStartTrace")))
        {
            watcher.EventArrived += delegate(object sender, EventArrivedEventArgs value) {
                try {
                    string name = Convert.ToString(value.NewEvent["ProcessName"]) ?? "";
                    int pid = Convert.ToInt32(value.NewEvent["ProcessID"]);
                    lock (setupPids) {
                        if (String.Equals(name, processName, StringComparison.OrdinalIgnoreCase)) setupPids.Add(pid);
                        if (IsInterpreter(name)) interpreters.Add(name);
                    }
                } catch { watcherFailed = true; }
            };
            watcher.Stopped += delegate(object sender, StoppedEventArgs value) { if (value.Status != ManagementStatus.NoError) watcherFailed = true; };
            watcher.Start();
            Process controller = Process.Start(new ProcessStartInfo(installer, "/S") { UseShellExecute = true });
            if (controller == null) return 65;
            lock (setupPids) setupPids.Add(controller.Id);
            Stopwatch elapsed = Stopwatch.StartNew();
            while (!controller.HasExited && elapsed.ElapsedMilliseconds < timeout) {
                try { pipeObserved |= Directory.GetFiles(@"\\.\pipe\").Any(path => String.Equals(Path.GetFileName(path), "TalkingQuill.Setup." + controller.Id, StringComparison.OrdinalIgnoreCase)); }
                catch { watcherFailed = true; }
                Thread.Sleep(5);
            }
            watcher.Stop();
            if (!controller.HasExited) { try { controller.Kill(); } catch {} return 66; }
            int[] pids; string[] shells;
            lock (setupPids) { pids = setupPids.Distinct().ToArray(); shells = interpreters.ToArray(); }
            bool passed = controller.ExitCode == 0 && pids.Length >= 2 && pipeObserved && shells.Length == 0 && !watcherFailed;
            string json = "{\"schemaVersion\":1,\"installer\":" + Quote(Path.GetFileName(installer)) + ",\"installerSha256\":" + Quote(installerHash) + ",\"architecture\":" + Quote(architecture) + ",\"sourceCommit\":" + Quote(sourceCommit) + ",\"sourceTree\":" + Quote(sourceTree) + ",\"packageMode\":" + Quote(packageMode) + ",\"controllerPid\":" + controller.Id + ",\"authenticatedSetupPids\":[" + String.Join(",", pids) + "],\"pipeObserved\":" + Bool(pipeObserved) + ",\"exitCode\":" + controller.ExitCode + ",\"interpreterProcessStarts\":[" + String.Join(",", shells.Select(Quote)) + "],\"passed\":" + Bool(passed) + "}";
            Directory.CreateDirectory(Path.GetDirectoryName(output));
            File.WriteAllText(output, json, new UTF8Encoding(false));
            return passed ? 0 : 67;
        }
    }
    static bool IsInterpreter(string name) { string value = name.ToLowerInvariant(); return value == "powershell.exe" || value == "pwsh.exe" || value == "cmd.exe" || value == "wscript.exe" || value == "cscript.exe"; }
    static string Hash(string path) { using (System.Security.Cryptography.SHA256 sha = System.Security.Cryptography.SHA256.Create()) using (FileStream stream = File.OpenRead(path)) return BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", "").ToLowerInvariant(); }
    static string Quote(string value) { return "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\""; }
    static string Bool(bool value) { return value ? "true" : "false"; }
}
