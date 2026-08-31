using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Linq;
using System.Management;
using Microsoft.Win32;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Runtime.InteropServices;

internal static class SuccessfulSetupObserver
{
    sealed class Identity
    {
        internal Process Handle;
        internal int Pid, ParentPid, SessionId;
        internal long CreationUtcTicks;
        internal string ImagePath, Sha256, UserSid, LogonId;
    }

    static int Main(string[] args)
    {
        if (args.Length != 8) return 64;
        string installer = Path.GetFullPath(args[0]), output = Path.GetFullPath(args[1]);
        int timeout = Int32.Parse(args[2]);
        string expectedArchitecture = args[3], expectedCommit = args[4], expectedTree = args[5], expectedMode = args[6], expectedHash = args[7];
        string installerHash = Hash(installer);
        if (!String.Equals(installerHash, expectedHash, StringComparison.Ordinal)) return 68;
        Dictionary<string, string> package = ReadPackageIdentity(installer);
        string preexistingRoot = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Talking Quill");
        string installedSetup = Path.Combine(preexistingRoot, "Uninstall Talking Quill.exe");
        string observedOperation = File.Exists(installedSetup) && Hash(installedSetup) == installerHash ? "repair" : package["packageMode"];
        if (package["architecture"] != expectedArchitecture || package["sourceCommit"] != expectedCommit ||
            package["sourceTree"] != expectedTree || package["packageMode"] != expectedMode) return 69;

        string processName = Path.GetFileName(installer);
        List<Identity> identities = new List<Identity>();
        List<string> interpreters = new List<string>();
        List<string> errors = new List<string>();
        object gate = new object();
        bool pipeObserved = false;
        using (ManagementEventWatcher watcher = new ManagementEventWatcher(new WqlEventQuery(
            "SELECT ProcessName,ProcessID,ParentProcessID,SessionID FROM Win32_ProcessStartTrace")))
        {
            watcher.EventArrived += delegate(object sender, EventArrivedEventArgs value) {
                try {
                    string name = Convert.ToString(value.NewEvent["ProcessName"]) ?? "";
                    int pid = Convert.ToInt32(value.NewEvent["ProcessID"]);
                    int parent = Convert.ToInt32(value.NewEvent["ParentProcessID"]);
                    if (IsInterpreter(name)) { lock (gate) interpreters.Add(name); }
                    if (String.Equals(name, processName, StringComparison.OrdinalIgnoreCase)) {
                        Identity identity = Capture(pid, parent);
                        lock (gate) identities.Add(identity);
                    }
                } catch (Exception error) { lock (gate) errors.Add("start:" + error.GetType().Name); }
            };
            watcher.Stopped += delegate(object sender, StoppedEventArgs value) {
                if (value.Status != ManagementStatus.NoError) lock (gate) errors.Add("watcher:" + value.Status);
            };
            watcher.Start();
            Thread.Sleep(250); // the watcher must be subscribed before launch
            Process controller = Process.Start(new ProcessStartInfo(installer, "/S") { UseShellExecute = true });
            if (controller == null) return 65;
            string authenticationReceipt = null;
            Exception authenticationReceiptError = null;
            Thread receiptReader = new Thread(delegate() {
                try {
                using (NamedPipeClientStream pipe = new NamedPipeClientStream(".", "TalkingQuill.Setup.Receipt." + controller.Id, PipeDirection.In))
                using (StreamReader reader = new StreamReader(pipe, new UTF8Encoding(false, true))) {
                    pipe.Connect(30000);
                    uint serverPid;
                    if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle.DangerousGetHandle(), out serverPid) || serverPid != (uint)controller.Id)
                        throw new InvalidDataException("Authentication receipt pipe server identity mismatch");
                    authenticationReceipt = reader.ReadToEnd();
                }
                } catch (Exception error) { authenticationReceiptError = error; }
            });
            receiptReader.IsBackground = true; receiptReader.Start();
            Identity controllerIdentity = Capture(controller.Id, ParentPid(controller.Id));
            lock (gate) identities.Add(controllerIdentity);
            Stopwatch elapsed = Stopwatch.StartNew();
            while (!controller.HasExited && elapsed.ElapsedMilliseconds < timeout) {
                try {
                    pipeObserved |= Directory.GetFiles(@"\\.\pipe\").Any(path =>
                        String.Equals(Path.GetFileName(path), "TalkingQuill.Setup." + controller.Id, StringComparison.OrdinalIgnoreCase));
                } catch (Exception error) { lock (gate) errors.Add("pipe:" + error.GetType().Name); }
                Thread.Sleep(2);
            }
            watcher.Stop();
            receiptReader.Join(35000);
            if (!controller.HasExited) { try { controller.Kill(); } catch {} return 66; }

            Identity[] processes;
            string[] shells, observerErrors;
            lock (gate) {
                processes = identities.GroupBy(value => value.Pid).Select(group => group.First()).ToArray();
                shells = interpreters.ToArray(); observerErrors = errors.ToArray();
            }
            Identity exactController = processes.SingleOrDefault(value => value.Pid == controller.Id);
            Identity worker = processes.FirstOrDefault(value => exactController != null && value.ParentPid == exactController.Pid && value.Pid != exactController.Pid);
            bool exactImages = exactController != null && worker != null &&
                String.Equals(exactController.ImagePath, installer, StringComparison.OrdinalIgnoreCase) &&
                String.Equals(worker.ImagePath, installer, StringComparison.OrdinalIgnoreCase) &&
                exactController.Sha256 == installerHash && worker.Sha256 == installerHash;
            bool tokenLineage = exactController != null && worker != null && exactController.UserSid.Length > 0 &&
                exactController.UserSid == worker.UserSid && exactController.LogonId.Length > 0 &&
                exactController.LogonId == worker.LogonId && exactController.SessionId == worker.SessionId;
            bool receiptAuthenticated = authenticationReceiptError == null && ReceiptMatches(
                authenticationReceipt, controller.Id, worker == null ? 0 : worker.Pid, installerHash);
            bool protocolAuthenticated = controller.ExitCode == 0 && pipeObserved && exactImages && tokenLineage && receiptAuthenticated;
            string installedRoot = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Talking Quill");
            string gateway = Path.Combine(installedRoot, @"resources\helper\talking-quill-helper.exe");
            string owner = Path.Combine(installedRoot, @"resources\helper\talking-quill-keyboard-owner.exe");
            bool installedIdentityBound = File.Exists(gateway) && File.Exists(owner) &&
                Hash(gateway) == package["gatewaySha256"] && Hash(owner) == package["ownerSha256"];
            string maintenance = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Talking Quill Maintenance.exe");
            bool registrationsExact = false, appPathExact = false;
            using (RegistryKey key = Registry.LocalMachine.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill")) {
                registrationsExact = key != null && Convert.ToString(key.GetValue("UninstallString")) == "\"" + maintenance + "\"" &&
                    Convert.ToString(key.GetValue("QuietUninstallString")) == "\"" + maintenance + "\" /S";
            }
            using (RegistryKey key = Registry.LocalMachine.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe")) {
                appPathExact = key != null && Convert.ToString(key.GetValue("")) == Path.Combine(installedRoot, "Talking Quill.exe") &&
                    Convert.ToString(key.GetValue("Path")) == installedRoot;
            }
            registrationsExact = registrationsExact && appPathExact;
            bool terminalTopology = !File.Exists(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), ".Talking Quill.native-transaction-v2.json")) &&
                !Directory.Exists(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), ".Talking Quill.native-staging")) &&
                !Directory.Exists(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), ".Talking Quill.native-backup"));
            bool passed = protocolAuthenticated && installedIdentityBound && registrationsExact && terminalTopology && shells.Length == 0 && observerErrors.Length == 0;
            string json = "{\"schemaVersion\":2,\"installer\":" + Quote(Path.GetFileName(installer)) +
                ",\"installerSha256\":" + Quote(installerHash) + ",\"architecture\":" + Quote(package["architecture"]) +
                ",\"sourceCommit\":" + Quote(package["sourceCommit"]) + ",\"sourceTree\":" + Quote(package["sourceTree"]) +
                ",\"packageMode\":" + Quote(package["packageMode"]) + ",\"operation\":" + Quote(observedOperation) + ",\"targetReleaseBuildDigest\":" + Quote(package["releaseBuildDigest"]) +
                ",\"targetGatewaySha256\":" + Quote(package["gatewaySha256"]) + ",\"targetOwnerSha256\":" + Quote(package["ownerSha256"]) +
                ",\"controllerPid\":" + controller.Id + ",\"authenticatedSetupPids\":[" + (protocolAuthenticated ? controller.Id + "," + worker.Pid : "") +
                "],\"processIdentities\":[" + String.Join(",", processes.Select(IdentityJson)) + "],\"pipeObserved\":" + Bool(pipeObserved) +
                ",\"nativeAuthenticationReceipt\":" + (receiptAuthenticated ? authenticationReceipt : "null") +
                ",\"installedIdentityBound\":" + Bool(installedIdentityBound) + ",\"registrationsExact\":" + Bool(registrationsExact) +
                ",\"terminalTopology\":" + Bool(terminalTopology) + ",\"exitCode\":" + controller.ExitCode + ",\"interpreterProcessStarts\":[" + String.Join(",", shells.Select(Quote)) +
                "],\"observerErrors\":[" + String.Join(",", observerErrors.Select(Quote)) + "],\"passed\":" + Bool(passed) + "}";
            Directory.CreateDirectory(Path.GetDirectoryName(output));
            File.WriteAllText(output, json, new UTF8Encoding(false));
            foreach (Identity identity in processes) if (identity.Handle != null) identity.Handle.Dispose();
            return passed ? 0 : 67;
        }
    }

    static Identity Capture(int pid, int parent)
    {
        Process process = Process.GetProcessById(pid); // retaining Process retains the kernel process handle
        string image = Path.GetFullPath(process.MainModule.FileName);
        return new Identity { Handle = process, Pid = pid, ParentPid = parent, SessionId = process.SessionId,
            CreationUtcTicks = process.StartTime.ToUniversalTime().Ticks, ImagePath = image, Sha256 = Hash(image), UserSid = OwnerSid(pid), LogonId = TokenLogonId(process.Handle) };
    }
    static int ParentPid(int pid) { using (ManagementObject value = new ManagementObject("win32_process.handle='" + pid + "'")) { value.Get(); return Convert.ToInt32(value["ParentProcessId"]); } }
    static string OwnerSid(int pid) { try { using (ManagementObject value = new ManagementObject("win32_process.handle='" + pid + "'")) { object[] result = new object[] { "" }; value.InvokeMethod("GetOwnerSid", result); return Convert.ToString(result[0]) ?? ""; } } catch { return ""; } }
    static string IdentityJson(Identity value) { return "{\"pid\":" + value.Pid + ",\"parentPid\":" + value.ParentPid + ",\"sessionId\":" + value.SessionId + ",\"creationUtcTicks\":" + value.CreationUtcTicks + ",\"imagePath\":" + Quote(value.ImagePath) + ",\"imageSha256\":" + Quote(value.Sha256) + ",\"userSid\":" + Quote(value.UserSid) + ",\"logonId\":" + Quote(value.LogonId) + "}"; }
    static bool ReceiptMatches(string json, int controller, int worker, string packageHash) {
        if (String.IsNullOrEmpty(json)) return false;
        Match transcript = Regex.Match(json, "\\\"transcriptSha256\\\":\\\"([0-9a-f]{64})\\\"");
        Match key = Regex.Match(json, "\\\"evidenceVerifierKey\\\":\\\"([0-9a-f]{64})\\\"");
        Match proof = Regex.Match(json, "\\\"receiptHmac\\\":\\\"([0-9a-f]{64})\\\"");
        bool hmacValid = false;
        if (transcript.Success && key.Success && proof.Success) {
            using (System.Security.Cryptography.HMACSHA256 hmac = new System.Security.Cryptography.HMACSHA256(Hex(key.Groups[1].Value)))
                hmacValid = BitConverter.ToString(hmac.ComputeHash(Hex(transcript.Groups[1].Value))).Replace("-", "").ToLowerInvariant() == proof.Groups[1].Value;
        }
        return hmacValid && Regex.IsMatch(json, "\\\"schemaVersion\\\":1") &&
            Regex.IsMatch(json, "\\\"protocol\\\":\\\"P-256-ECDH/HMAC-SHA256-v1\\\"") &&
            Regex.IsMatch(json, "\\\"controllerPid\\\":" + controller + "(?:[,}])") &&
            Regex.IsMatch(json, "\\\"workerPid\\\":" + worker + "(?:[,}])") &&
            json.Contains("\"packageSha256\":\"" + packageHash + "\"") &&
            json.Contains("\"workerProofVerified\":true") && json.Contains("\"controllerProofSent\":true");
    }
    [StructLayout(LayoutKind.Sequential)] struct LUID { internal uint LowPart; internal int HighPart; }
    [StructLayout(LayoutKind.Sequential)] struct TOKEN_STATISTICS { internal LUID TokenId, AuthenticationId; internal long ExpirationTime; internal uint TokenType, ImpersonationLevel, DynamicCharged, DynamicAvailable, GroupCount, PrivilegeCount; internal LUID ModifiedId; }
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool GetTokenInformation(IntPtr token, int informationClass, out TOKEN_STATISTICS statistics, int length, out int returned);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetNamedPipeServerProcessId(IntPtr pipe, out uint serverProcessId);
    static string TokenLogonId(IntPtr process) {
        IntPtr token; if (!OpenProcessToken(process, 0x0008, out token)) return "";
        try { TOKEN_STATISTICS statistics; int returned; if (!GetTokenInformation(token, 10, out statistics, Marshal.SizeOf(typeof(TOKEN_STATISTICS)), out returned)) return ""; return statistics.AuthenticationId.HighPart.ToString("x8") + statistics.AuthenticationId.LowPart.ToString("x8"); }
        finally { CloseHandle(token); }
    }
    static Dictionary<string, string> ReadPackageIdentity(string path)
    {
        byte[] footer = new byte[128], manifest;
        using (FileStream stream = File.OpenRead(path)) {
            stream.Seek(-128, SeekOrigin.End); if (stream.Read(footer, 0, footer.Length) != footer.Length) throw new InvalidDataException();
            if (Encoding.ASCII.GetString(footer, 0, 6) != "TQPKG2") throw new InvalidDataException();
            long offset = BitConverter.ToInt64(footer, 16), size = BitConverter.ToInt64(footer, 24), manifestSize = BitConverter.ToInt64(footer, 32);
            if (offset < 0 || size <= 0 || manifestSize <= 0 || manifestSize > 8 * 1024 * 1024 || offset + size != stream.Length - 128) throw new InvalidDataException();
            manifest = new byte[(int)manifestSize]; stream.Seek(offset, SeekOrigin.Begin); if (stream.Read(manifest, 0, manifest.Length) != manifest.Length) throw new InvalidDataException();
        }
        string json = new UTF8Encoding(false, true).GetString(manifest);
        Dictionary<string, string> result = new Dictionary<string, string>();
        foreach (string name in new[] { "architecture", "sourceCommit", "sourceTree", "packageMode" }) {
            Match match = Regex.Match(json, "\\\"" + name + "\\\":\\\"([^\\\"]+)\\\"");
            if (!match.Success) throw new InvalidDataException(); result[name] = match.Groups[1].Value;
        }
        Match target = Regex.Match(json, "\\\"target\\\":\\{([^}]*)\\}");
        if (!target.Success) throw new InvalidDataException();
        foreach (string name in new[] { "releaseBuildDigest", "gatewaySha256", "ownerSha256" }) {
            Match match = Regex.Match(target.Groups[1].Value, "\\\"" + name + "\\\":\\\"([^\\\"]+)\\\"");
            if (!match.Success) throw new InvalidDataException(); result[name] = match.Groups[1].Value;
        }
        return result;
    }
    static byte[] Hex(string value) { byte[] output = new byte[value.Length / 2]; for (int index = 0; index < output.Length; index++) output[index] = Convert.ToByte(value.Substring(index * 2, 2), 16); return output; }
    static bool IsInterpreter(string name) { string value = name.ToLowerInvariant(); return value == "powershell.exe" || value == "pwsh.exe" || value == "cmd.exe" || value == "wscript.exe" || value == "cscript.exe"; }
    static string Hash(string path) { using (System.Security.Cryptography.SHA256 sha = System.Security.Cryptography.SHA256.Create()) using (FileStream stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read | FileShare.Delete)) return BitConverter.ToString(sha.ComputeHash(stream)).Replace("-", "").ToLowerInvariant(); }
    static string Quote(string value) { return "\"" + (value ?? "").Replace("\\", "\\\\").Replace("\"", "\\\"") + "\""; }
    static string Bool(bool value) { return value ? "true" : "false"; }
}
