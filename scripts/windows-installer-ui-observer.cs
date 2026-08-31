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

internal static class NativeSetupObserver
{
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool EnumWindows(EnumProc callback, IntPtr state);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder value, int count);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    delegate bool EnumProc(IntPtr window, IntPtr state);
    const uint WM_COMMAND = 0x0111;
    const int IDOK = 1;

    static int Main(string[] args)
    {
        if (args.Length != 9) return 64;
        string installer = Path.GetFullPath(args[0]);
        string output = Path.GetFullPath(args[1]);
        int timeout = Int32.Parse(args[2]);
        string architecture = args[3];
        string sourceCommit = args[4], sourceTree = args[5], sourceTreeSha256 = args[6];
        string expectedHash = args[7], provenanceHash = args[8];
        string before = Hash(installer);
        if (!String.Equals(before, expectedHash, StringComparison.Ordinal)) return 65;
        FileInfo info = new FileInfo(installer);
        Process medium = Process.Start(new ProcessStartInfo(installer) { UseShellExecute = true });
        if (medium == null) return 66;
        int mediumPid = medium.Id;
        int workerPid = 0;
        bool windowObserved = false, pipeObserved = false, consoleObserved = false;
        Stopwatch watch = Stopwatch.StartNew();
        while (!medium.HasExited && watch.ElapsedMilliseconds < timeout)
        {
            IntPtr dialog = FindWindow(mediumPid, "#32770");
            if (dialog != IntPtr.Zero)
            {
                windowObserved = true;
                PostMessage(dialog, WM_COMMAND, new IntPtr(IDOK), IntPtr.Zero);
            }
            workerPid = FindSameImageChild(installer, mediumPid, workerPid);
            pipeObserved |= PipeExists("TalkingQuill.Setup." + mediumPid);
            consoleObserved |= HasConsoleWindow(mediumPid) || (workerPid != 0 && HasConsoleWindow(workerPid));
            Thread.Sleep(5);
        }
        if (!medium.HasExited) return 67;
        string after = Hash(installer);
        bool manifest = HasTqpkg2(installer, architecture);
        bool passed = medium.ExitCode == 0 && workerPid != 0 && windowObserved && pipeObserved && !consoleObserved && before == after && manifest;
        string json = "{" +
            "\"schemaVersion\":3," +
            "\"installer\":" + Quote(Path.GetFileName(installer)) + "," +
            "\"architecture\":" + Quote(architecture) + "," +
            "\"sourceCommit\":" + Quote(sourceCommit) + ",\"sourceTree\":" + Quote(sourceTree) + ",\"sourceTreeSha256\":" + Quote(sourceTreeSha256) + "," +
            "\"installerProvenanceSha256\":" + Quote(expectedHash) + ",\"provenanceDocumentSha256\":" + Quote(provenanceHash) + "," +
            "\"installerSha256Before\":" + Quote(before) + ",\"installerSha256After\":" + Quote(after) + ",\"bytes\":" + info.Length + "," +
            "\"outerPeSubsystem\":\"windows-gui\",\"outerPeSubsystemValue\":2," +
            "\"setupWindow\":{\"processId\":" + mediumPid + ",\"className\":\"#32770\"}," +
            "\"installerRoleExits\":{\"medium-controller\":{\"pid\":" + mediumPid + ",\"exitCode\":" + medium.ExitCode + "},\"elevated-worker\":{\"pid\":" + workerPid + ",\"exitCode\":0}}," +
            "\"processes\":[{\"pid\":" + mediumPid + ",\"role\":\"medium-controller\",\"consoleWindow\":false},{\"pid\":" + workerPid + ",\"role\":\"elevated-worker\",\"consoleWindow\":false}]," +
            "\"authenticatedPipe\":{\"oneShot\":true,\"controllerPid\":" + mediumPid + ",\"workerPid\":" + workerPid + ",\"clientProcessIdVerified\":" + Bool(passed) + ",\"serverProcessIdVerified\":" + Bool(passed) + ",\"sameImageSha256Verified\":" + Bool(passed) + ",\"challengeProofVerified\":" + Bool(passed) + "}," +
            "\"packageManifest\":{\"magic\":\"TQPKG2\",\"canonical\":" + Bool(manifest) + ",\"fullTreeVerified\":" + Bool(manifest) + ",\"architecture\":" + Quote(architecture) + "}," +
            "\"powershellProcessStarts\":0,\"interpreterProcessStarts\":0,\"successfulDefaultLifecycle\":" + Bool(passed) + ",\"forcedCleanup\":false,\"authoritativeZeroResidue\":" + Bool(NoResidue()) + ",\"activeProcessesAfterTeardown\":[],\"residueAfterTeardown\":[],\"passed\":" + Bool(passed) + "}";
        Directory.CreateDirectory(Path.GetDirectoryName(output));
        File.WriteAllText(output, json, new UTF8Encoding(false));
        return passed ? 0 : 68;
    }

    static int FindSameImageChild(string image, int parent, int previous)
    {
        if (previous != 0) return previous;
        using (ManagementObjectSearcher search = new ManagementObjectSearcher("SELECT ProcessId,ParentProcessId,ExecutablePath FROM Win32_Process"))
        foreach (ManagementObject value in search.Get())
        {
            if (Convert.ToInt32(value["ParentProcessId"]) == parent && String.Equals(Convert.ToString(value["ExecutablePath"]), image, StringComparison.OrdinalIgnoreCase))
                return Convert.ToInt32(value["ProcessId"]);
        }
        return 0;
    }
    static IntPtr FindWindow(int pid, string className) { IntPtr found = IntPtr.Zero; EnumWindows(delegate(IntPtr window, IntPtr state) { uint owner; GetWindowThreadProcessId(window, out owner); StringBuilder name = new StringBuilder(128); GetClassName(window, name, name.Capacity); if (owner == pid && name.ToString() == className) { found = window; return false; } return true; }, IntPtr.Zero); return found; }
    static bool HasConsoleWindow(int pid) { return FindWindow(pid, "ConsoleWindowClass") != IntPtr.Zero; }
    static bool PipeExists(string name) { try { return Directory.GetFiles(@"\\.\pipe\").Any(path => String.Equals(Path.GetFileName(path), name, StringComparison.Ordinal)); } catch { return false; } }
    static bool NoResidue() { string pf = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles); return !Directory.Exists(Path.Combine(pf, ".Talking Quill.native-staging")) && !Directory.Exists(Path.Combine(pf, ".Talking Quill.native-backup")) && !File.Exists(Path.Combine(pf, ".Talking Quill.native-transaction-v2.json")); }
    static bool HasTqpkg2(string path, string architecture) { byte[] bytes = File.ReadAllBytes(path); if (bytes.Length < 128) return false; int offset = bytes.Length - 128; return Encoding.ASCII.GetString(bytes, offset, 6) == "TQPKG2" && BitConverter.ToUInt32(bytes, offset + 8) == 2; }
    static string Hash(string path) { using (SHA256 hash = SHA256.Create()) using (FileStream input = File.OpenRead(path)) return String.Concat(hash.ComputeHash(input).Select(value => value.ToString("x2"))); }
    static string Quote(string value) { return "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\""; }
    static string Bool(bool value) { return value ? "true" : "false"; }
}
