// Hosted smoke only. Never filters an administrator token or changes UAC policy.
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

public static class TqHostedMediumLauncher
{
    const uint Query = 8, Duplicate = 2;
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct StartupInfo {
        public int cb; public string reserved, desktop, title;
        public int x, y, xSize, ySize, xChars, yChars, fill, flags;
        public short show, reservedSize; public IntPtr reservedBytes, stdin, stdout, stderr;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInfo { public IntPtr process, thread; public uint pid, tid; }
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool GetTokenInformation(IntPtr token, int kind, IntPtr data, int size, out int needed);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool DuplicateTokenEx(IntPtr token, uint access, IntPtr attributes, int level, int type, out IntPtr result);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessWithTokenW(IntPtr token, uint logonFlags, string application, StringBuilder command, uint flags, IntPtr environment, string directory, ref StartupInfo startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr handle, out uint code);
    [DllImport("user32.dll")] static extern IntPtr GetShellWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);

    sealed class Claims {
        public string user; public long logon; public int session, integrity, elevated, elevationType;
        public override string ToString() { return String.Format("user={0} logon={1:x} session={2} integrity=0x{3:x} elevated={4} elevationType={5}", user, logon, session, integrity, elevated, elevationType); }
    }
    static Exception Error(string operation) { return new Win32Exception(Marshal.GetLastWin32Error(), operation); }
    static IntPtr Information(IntPtr token, int kind) {
        int size; GetTokenInformation(token, kind, IntPtr.Zero, 0, out size);
        if (size <= 0) throw Error("GetTokenInformation size " + kind);
        IntPtr data = Marshal.AllocHGlobal(size);
        if (!GetTokenInformation(token, kind, data, size, out size)) { var error = Error("GetTokenInformation " + kind); Marshal.FreeHGlobal(data); throw error; }
        return data;
    }
    static int Number(IntPtr token, int kind) {
        IntPtr data = Information(token, kind);
        try { return Marshal.ReadInt32(data); } finally { Marshal.FreeHGlobal(data); }
    }
    static Claims ReadClaims(IntPtr token) {
        var result = new Claims();
        IntPtr data = Information(token, 1);
        try { result.user = new SecurityIdentifier(Marshal.ReadIntPtr(data)).Value; } finally { Marshal.FreeHGlobal(data); }
        data = Information(token, 10);
        try { result.logon = Marshal.ReadInt64(data, 8); } finally { Marshal.FreeHGlobal(data); }
        data = Information(token, 25);
        try {
            IntPtr sid = Marshal.ReadIntPtr(data);
            int count = Marshal.ReadByte(sid, 1);
            if (count == 0) throw new InvalidOperationException("Invalid integrity SID.");
            result.integrity = Marshal.ReadInt32(sid, 8 + 4 * (count - 1));
        } finally { Marshal.FreeHGlobal(data); }
        result.session = Number(token, 12); result.elevated = Number(token, 20); result.elevationType = Number(token, 18);
        return result;
    }
    static IntPtr Linked(IntPtr token) {
        IntPtr data = Marshal.AllocHGlobal(IntPtr.Size);
        try {
            int needed;
            if (!GetTokenInformation(token, 19, data, IntPtr.Size, out needed)) {
                if (Marshal.GetLastWin32Error() == 1312) return IntPtr.Zero;
                throw Error("TokenLinkedToken");
            }
            return Marshal.ReadIntPtr(data);
        } finally { Marshal.FreeHGlobal(data); }
    }
    static bool SameIdentity(Claims a, Claims b) { return a.user == b.user && a.logon == b.logon && a.session == b.session && a.integrity == b.integrity; }
    static bool Accept(IntPtr token, Claims host) {
        Claims candidate = ReadClaims(token);
        if (candidate.user != host.user || candidate.session != host.session || candidate.integrity != 0x2000 || candidate.elevated != 0) return false;
        if (candidate.logon == host.logon) return true;
        IntPtr linked = Linked(token);
        try { return linked != IntPtr.Zero && SameIdentity(ReadClaims(linked), host); }
        finally { if (linked != IntPtr.Zero) CloseHandle(linked); }
    }
    static IntPtr Select(IntPtr hostToken, string log) {
        Claims host = ReadClaims(hostToken);
        File.AppendAllText(log, "runner " + host + Environment.NewLine);
        IntPtr candidate = Linked(hostToken);
        if (candidate != IntPtr.Zero) {
            try {
                File.AppendAllText(log, "linked " + ReadClaims(candidate) + Environment.NewLine);
                if (Accept(candidate, host)) { IntPtr selected = candidate; candidate = IntPtr.Zero; return selected; }
            } finally { if (candidate != IntPtr.Zero) CloseHandle(candidate); }
        } else File.AppendAllText(log, "Windows supplied no linked token." + Environment.NewLine);
        // Use only the OS-designated desktop shell, not an arbitrary process named explorer.
        uint pid; IntPtr shell = GetShellWindow(); GetWindowThreadProcessId(shell, out pid);
        if (shell != IntPtr.Zero && pid != 0) {
            IntPtr process = OpenProcess(0x1000, false, pid);
            if (process == IntPtr.Zero) throw Error("Open desktop shell");
            try {
                if (!OpenProcessToken(process, Query | Duplicate, out candidate)) throw Error("Open desktop shell token");
                try {
                    File.AppendAllText(log, "shell pid=" + pid + " " + ReadClaims(candidate) + Environment.NewLine);
                    if (Accept(candidate, host)) { IntPtr selected = candidate; candidate = IntPtr.Zero; return selected; }
                } finally { if (candidate != IntPtr.Zero) CloseHandle(candidate); }
            } finally { CloseHandle(process); }
        } else File.AppendAllText(log, "No desktop shell token available." + Environment.NewLine);
        throw new InvalidOperationException("No authenticated medium token is available. This runner needs a real split-token interactive logon with working ShellExecute runas. With UAC disabled, an elevated non-split runner and elevated shell cannot exercise the production controller/worker path. No token filtering or UAC policy changes were attempted.");
    }
    public static int Launch(string executable, string log, uint timeoutMilliseconds) {
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") != "true" || Environment.GetEnvironmentVariable("RUNNER_ENVIRONMENT") != "github-hosted") throw new InvalidOperationException("Hosted runners only.");
        IntPtr host = IntPtr.Zero, medium = IntPtr.Zero, primary = IntPtr.Zero;
        ProcessInfo process = new ProcessInfo();
        try {
            if (!OpenProcessToken(GetCurrentProcess(), Query | Duplicate, out host)) throw Error("Open runner token");
            medium = Select(host, log);
            if (!DuplicateTokenEx(medium, 0x02000000, IntPtr.Zero, 2, 1, out primary)) throw Error("Duplicate medium primary token");
            File.AppendAllText(log, "controller token " + ReadClaims(primary) + Environment.NewLine);
            var startup = new StartupInfo { cb = Marshal.SizeOf(typeof(StartupInfo)), desktop = "winsta0\\default" };
            // No worker flags. The production medium controller must launch and authenticate its worker.
            if (!CreateProcessWithTokenW(primary, 0, executable, new StringBuilder("\"" + executable + "\" /S"), 0, IntPtr.Zero, Path.GetDirectoryName(executable), ref startup, out process)) throw Error("CreateProcessWithTokenW");
            File.AppendAllText(log, "controller pid=" + process.pid + Environment.NewLine);
            uint wait = WaitForSingleObject(process.process, timeoutMilliseconds);
            if (wait == 258) throw new TimeoutException("Controller timed out. No broad process cleanup will be attempted.");
            if (wait != 0) throw Error("Wait for controller");
            uint code; if (!GetExitCodeProcess(process.process, out code)) throw Error("Controller exit code");
            File.AppendAllText(log, "controller exit=" + code + Environment.NewLine);
            return unchecked((int)code);
        } catch (Exception error) { File.AppendAllText(log, error + Environment.NewLine); throw; }
        finally {
            if (process.thread != IntPtr.Zero) CloseHandle(process.thread);
            if (process.process != IntPtr.Zero) CloseHandle(process.process);
            if (primary != IntPtr.Zero) CloseHandle(primary);
            if (medium != IntPtr.Zero) CloseHandle(medium);
            if (host != IntPtr.Zero) CloseHandle(host);
        }
    }
}
