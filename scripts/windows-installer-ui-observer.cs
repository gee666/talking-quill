using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Management;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using Microsoft.Win32;

internal static class WindowsInstallerUiObserver
{
    private const uint CreateSuspended = 0x00000004;
    private const uint CreateUnicodeEnvironment = 0x00000400;
    private const uint JobObjectLimitKillOnClose = 0x00002000;
    private const uint WmCommand = 0x0111;
    private const uint WmQuit = 0x0012;
    private const int IdCancel = 2;
    private const int IdYes = 6;
    private const int ObjIdWindow = 0;
    private const uint EventObjectCreate = 0x8000;
    private const uint EventObjectShow = 0x8002;
    private const uint WineventOutOfContext = 0;
    private const int SampleIntervalMs = 5;

    private static readonly object Sync = new object();
    private static readonly ConcurrentQueue<string> MutationEvents = new ConcurrentQueue<string>();
    private static readonly ConcurrentQueue<string> ConsoleEvents = new ConcurrentQueue<string>();
    private static readonly ConcurrentQueue<string> ObserverErrors = new ConcurrentQueue<string>();
    private static readonly ConcurrentDictionary<uint, ProcessRecord> RelevantProcesses = new ConcurrentDictionary<uint, ProcessRecord>();
    private static readonly ConcurrentDictionary<uint, ProcessRecord> StartedProcesses = new ConcurrentDictionary<uint, ProcessRecord>();
    private static readonly List<FileSystemWatcher> FileWatchers = new List<FileSystemWatcher>();
    private static readonly List<RegistryWatch> RegistryWatches = new List<RegistryWatch>();
    private static readonly ManualResetEvent WindowHookReady = new ManualResetEvent(false);
    private static readonly CountdownEvent RegistryWatchesReady = new CountdownEvent(7);
    private static WinEventDelegate windowDelegate;
    private static uint windowHookThreadId;
    private static string installerPath;
    private static string installerName;
    private static string programData;
    private static HashSet<string> baselineProtectedLeaves;
    private static Dictionary<uint, long> baselineProcesses;
    private static ManagementEventWatcher processStartWatcher;
    private static volatile bool launched;
    private static volatile bool stopMonitoring;
    private static long lastSampleTicks;
    private static long maxSampleGapTicks;
    private static int processSamples;
    private static int windowSamples;
    private static int filesystemSamples;
    private static int registrySamples;
    private static int powershellStarts;
    private static bool protectedLeafObserved;
    private static WindowRecord installerWindow;

    private sealed class ProcessRecord
    {
        internal uint Pid;
        internal uint ParentPid;
        internal string Image;
        internal long CreationTime;
    }

    private sealed class RegistryWatch
    {
        internal RegistryKey Key;
        internal Thread Thread;
        internal string Name;
        internal IntPtr EventHandle;
        internal volatile bool Stop;
    }

    private sealed class WindowRecord
    {
        internal IntPtr Handle;
        internal uint Pid;
        internal string Title;
        internal string ClassName;
        internal string Image;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicLimitInformation
    {
        internal long PerProcessUserTimeLimit, PerJobUserTimeLimit;
        internal uint LimitFlags;
        internal UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize;
        internal uint ActiveProcessLimit;
        internal UIntPtr Affinity;
        internal uint PriorityClass, SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters
    {
        internal ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
        internal ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ExtendedLimitInformation
    {
        internal BasicLimitInformation BasicLimitInformation;
        internal IoCounters IoInfo;
        internal UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct StartupInfo
    {
        internal uint cb;
        internal string reserved, desktop, title;
        internal uint x, y, xSize, ySize, xCountChars, yCountChars, fillAttribute, flags;
        internal ushort showWindow, reserved2Length;
        internal IntPtr reserved2, standardInput, standardOutput, standardError;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInformation
    {
        internal IntPtr process, thread;
        internal uint processId, threadId;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct ProcessEntry32
    {
        internal uint size, usage, processId;
        internal IntPtr defaultHeapId;
        internal uint moduleId, threads, parentProcessId;
        internal int priorityClass;
        internal uint flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] internal string executable;
    }

    private delegate bool EnumWindowsDelegate(IntPtr window, IntPtr data);
    private delegate void WinEventDelegate(IntPtr hook, uint eventType, IntPtr window, int objectId, int childId, uint threadId, uint time);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern bool CreateProcess(string applicationName, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint flags, IntPtr environment, string currentDirectory, ref StartupInfo startup, out ProcessInformation process);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool SetInformationJobObject(IntPtr job, int informationClass, IntPtr information, uint length);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool TerminateJobObject(IntPtr job, uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool TerminateProcess(IntPtr process, uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint processId);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern bool Process32First(IntPtr snapshot, ref ProcessEntry32 entry);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern bool Process32Next(IntPtr snapshot, ref ProcessEntry32 entry);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern bool QueryFullProcessImageName(IntPtr process, int flags, StringBuilder name, ref int size);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool GetProcessTimes(IntPtr process, out long creation, out long exit, out long kernel, out long user);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr CreateEvent(IntPtr attributes, bool manualReset, bool initialState, string name);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool SetEvent(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)] private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("advapi32.dll", SetLastError = true)] private static extern int RegNotifyChangeKeyValue(IntPtr key, bool watchSubtree, uint filter, IntPtr eventHandle, bool asynchronous);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsDelegate callback, IntPtr data);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder text, int size);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr module, WinEventDelegate callback, uint processId, uint threadId, uint flags);
    [DllImport("user32.dll")] private static extern bool UnhookWinEvent(IntPtr hook);
    [DllImport("user32.dll")] private static extern int GetMessage(out NativeMessage message, IntPtr window, uint min, uint max);
    [DllImport("user32.dll")] private static extern bool PostThreadMessage(uint threadId, uint message, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeMessage { internal IntPtr window; internal uint message; internal UIntPtr wParam; internal IntPtr lParam; internal uint time; internal int x, y; }

    private static int Main(string[] args)
    {
        if (args.Length != 9) return Fail("Usage: observer <installer> <evidence> <timeout-ms> <arch> <commit> <tree> <source-tree-sha256> <installer-sha256> <provenance-document-sha256>");
        installerPath = Path.GetFullPath(args[0]);
        string evidencePath = Path.GetFullPath(args[1]);
        int timeout;
        if (!File.Exists(installerPath) || !Int32.TryParse(args[2], out timeout) || timeout < 5000 || timeout > 120000 ||
            (args[3] != "x64" && args[3] != "arm64") || !Hex(args[4], 40) || !Hex(args[5], 40) || !Hex(args[6], 64) || !Hex(args[7], 64) || !Hex(args[8], 64)) return Fail("Observer arguments are invalid");
        installerName = Path.GetFileName(installerPath);
        programData = Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData).TrimEnd('\\');
        baselineProtectedLeaves = ProtectedLeaves();
        baselineProcesses = ProcessSnapshot().ToDictionary(value => value.Pid, value => ProcessCreationTime(value.Pid));
        if (baselineProtectedLeaves.Count != 0) return Fail("Protected bootstrap baseline is not empty");
        byte[] before = File.ReadAllBytes(installerPath);
        int subsystem = PeSubsystem(before);
        if (subsystem != 2) return Fail("Installer is not a Windows GUI subsystem executable");
        string beforeHash = Sha256(before);
        if (!String.Equals(beforeHash, args[7], StringComparison.Ordinal)) return Fail("Installer hash does not match provenance");
        string baselineRegistry = RegistrySnapshot();
        if (baselineRegistry != "clean") return Fail("Disposable registry baseline is not clean");
        string baselineFiles = DurableFileSnapshot();
        if (baselineFiles != "clean") return Fail("Disposable filesystem baseline is not clean");

        IntPtr job = IntPtr.Zero;
        ProcessInformation child = new ProcessInformation();
        bool forcedCleanup = false;
        bool graceful = false;
        int exitCode = -1;
        var monitor = new Thread(MonitorLoop) { IsBackground = true, Name = "installer-ui-monitor" };
        var hook = new Thread(WindowHookLoop) { IsBackground = true, Name = "installer-window-hook" };
        try
        {
            StartFileWatchers();
            StartRegistryWatchers();
            if (!RegistryWatchesReady.Wait(5000)) throw new InvalidOperationException("Registry watchers did not become ready");
            StartProcessWatcher();
            hook.Start();
            if (!WindowHookReady.WaitOne(5000)) throw new InvalidOperationException("Window hook did not become ready");
            lastSampleTicks = Stopwatch.GetTimestamp();
            monitor.Start();
            Thread.Sleep(50);
            if (processSamples < 2 || windowSamples < 2 || filesystemSamples < 2 || registrySamples < 2)
                throw new InvalidOperationException("Observers were not ready before launch");

            job = CreateJobObject(IntPtr.Zero, null);
            if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            SetKillOnClose(job);
            var startup = new StartupInfo { cb = (uint)Marshal.SizeOf(typeof(StartupInfo)) };
            var command = new StringBuilder(Quote(installerPath));
            if (!CreateProcess(installerPath, command, IntPtr.Zero, IntPtr.Zero, false, CreateSuspended | CreateUnicodeEnvironment, IntPtr.Zero, Environment.CurrentDirectory, ref startup, out child))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (!AssignProcessToJobObject(job, child.process)) throw new Win32Exception(Marshal.GetLastWin32Error());
            launched = true;
            TrackProcess(child.processId, 0, installerPath, ProcessCreationTime(child.processId));
            if (ResumeThread(child.thread) == UInt32.MaxValue) throw new Win32Exception(Marshal.GetLastWin32Error());
            CloseHandle(child.thread); child.thread = IntPtr.Zero;

            DateTime deadline = DateTime.UtcNow.AddMilliseconds(timeout);
            while (DateTime.UtcNow < deadline && installerWindow == null && ObserverErrors.IsEmpty && ConsoleEvents.IsEmpty && MutationEvents.IsEmpty) Thread.Sleep(5);
            if (installerWindow == null) throw new InvalidOperationException("NSIS assisted-installer GUI window was not observed");
            if (installerWindow.ClassName != "#32770" || installerWindow.Title.IndexOf("Talking Quill", StringComparison.OrdinalIgnoreCase) < 0)
                throw new InvalidOperationException("NSIS GUI title or class is invalid");

            PostMessage(installerWindow.Handle, WmCommand, new IntPtr(IdCancel), IntPtr.Zero);
            while (DateTime.UtcNow < deadline)
            {
                foreach (WindowRecord window in EnumerateWindows())
                {
                    if (window.Pid == installerWindow.Pid && window.Handle != installerWindow.Handle && window.ClassName == "#32770")
                        PostMessage(window.Handle, WmCommand, new IntPtr(IdYes), IntPtr.Zero);
                }
                if (!AnyTrackedProcessAlive()) { graceful = true; break; }
                Thread.Sleep(5);
            }
            if (!graceful) throw new InvalidOperationException("Installer did not exit through UI cancellation within the bound");
            exitCode = ProcessExitCode(child.process);
            Thread.Sleep(100);
            stopMonitoring = true;
            monitor.Join(5000);
            StopWindowHook(hook);
            StopFileWatchers();
            StopRegistryWatchers();
            StopProcessWatcher();

            byte[] after = File.ReadAllBytes(installerPath);
            string afterHash = Sha256(after);
            HashSet<string> finalLeaves = ProtectedLeaves();
            string finalRegistry = RegistrySnapshot();
            string finalFiles = DurableFileSnapshot();
            long maxGapMs = maxSampleGapTicks * 1000 / Stopwatch.Frequency;
            bool pass = beforeHash == afterHash && ObserverErrors.IsEmpty && ConsoleEvents.IsEmpty && MutationEvents.IsEmpty &&
                finalLeaves.SetEquals(baselineProtectedLeaves) && finalRegistry == baselineRegistry && finalFiles == baselineFiles &&
                maxGapMs <= 50 && graceful && !forcedCleanup && !AnyTrackedProcessAlive();
            WriteEvidence(evidencePath, args, before.Length, beforeHash, afterHash, subsystem, maxGapMs, graceful, forcedCleanup, exitCode, pass);
            return pass ? 0 : Fail("Installer UI evidence did not satisfy the release gate");
        }
        catch (Exception error)
        {
            ObserverErrors.Enqueue(error.Message);
            forcedCleanup = true;
            if (job != IntPtr.Zero) TerminateJobObject(job, 125);
            DateTime cleanupDeadline = DateTime.UtcNow.AddSeconds(10);
            while (DateTime.UtcNow < cleanupDeadline && AnyTrackedProcessAlive()) { TerminateTrackedProcesses(); Thread.Sleep(10); }
            if (AnyTrackedProcessAlive()) ObserverErrors.Enqueue("tracked installer processes survived forced cleanup");
            Thread.Sleep(100);
            stopMonitoring = true;
            monitor.Join(5000);
            StopWindowHook(hook);
            StopFileWatchers();
            StopRegistryWatchers();
            StopProcessWatcher();
            try { WriteEvidence(evidencePath, args, before.Length, beforeHash, Sha256(File.ReadAllBytes(installerPath)), subsystem, maxSampleGapTicks * 1000 / Stopwatch.Frequency, graceful, forcedCleanup, exitCode, false); } catch { }
            return Fail(error.ToString());
        }
        finally
        {
            if (child.thread != IntPtr.Zero) CloseHandle(child.thread);
            if (child.process != IntPtr.Zero) CloseHandle(child.process);
            if (job != IntPtr.Zero) CloseHandle(job);
        }
    }

    private static void MonitorLoop()
    {
        try
        {
            while (!stopMonitoring)
            {
                long now = Stopwatch.GetTimestamp();
                long gap = now - Interlocked.Exchange(ref lastSampleTicks, now);
                if (gap > maxSampleGapTicks) Interlocked.Exchange(ref maxSampleGapTicks, gap);
                List<ProcessRecord> snapshot = ProcessSnapshot();
                if (launched) foreach (ProcessRecord process in snapshot)
                {
                    process.CreationTime = ProcessCreationTime(process.Pid);
                    long baselineCreation;
                    if (!baselineProcesses.TryGetValue(process.Pid, out baselineCreation) || baselineCreation != process.CreationTime)
                        StartedProcesses[process.Pid] = process;
                }
                ResolveTrackedProcesses();
                Interlocked.Increment(ref processSamples);
                foreach (WindowRecord window in EnumerateWindows()) ObserveWindow(window, "sample");
                Interlocked.Increment(ref windowSamples);
                ObserveFilesystemSnapshot(); Interlocked.Increment(ref filesystemSamples);
                if (RegistrySnapshot() != "clean") MutationEvents.Enqueue("registry snapshot changed");
                Interlocked.Increment(ref registrySamples);
                Thread.Sleep(SampleIntervalMs);
            }
        }
        catch (Exception error) { ObserverErrors.Enqueue("monitor: " + error.Message); }
    }

    private static void WindowHookLoop()
    {
        windowHookThreadId = GetCurrentThreadId();
        windowDelegate = delegate(IntPtr hook, uint eventType, IntPtr window, int objectId, int childId, uint thread, uint time)
        {
            if (objectId == ObjIdWindow && window != IntPtr.Zero &&
                (eventType == EventObjectShow || IsWindowVisible(window)))
            {
                WindowRecord observed = ReadWindow(window);
                if (eventType == EventObjectShow && observed.Pid == 0 && observed.ClassName.Length == 0)
                    ObserverErrors.Enqueue("transient shown window vanished before classification");
                else ObserveWindow(observed, "hook-" + eventType.ToString("x", CultureInfo.InvariantCulture));
            }
        };
        IntPtr handle = SetWinEventHook(EventObjectCreate, EventObjectShow, IntPtr.Zero, windowDelegate, 0, 0, WineventOutOfContext);
        if (handle == IntPtr.Zero) { ObserverErrors.Enqueue("SetWinEventHook failed"); WindowHookReady.Set(); return; }
        WindowHookReady.Set();
        NativeMessage message;
        while (GetMessage(out message, IntPtr.Zero, 0, 0) > 0) { }
        UnhookWinEvent(handle);
    }

    private static void StopWindowHook(Thread thread)
    {
        if (thread == null || !thread.IsAlive) return;
        PostThreadMessage(windowHookThreadId, WmQuit, IntPtr.Zero, IntPtr.Zero);
        thread.Join(5000);
        if (thread.IsAlive) ObserverErrors.Enqueue("window hook did not stop");
    }

    private static void ObserveWindow(WindowRecord window, string source)
    {
        if (window == null || !launched) return;
        string image = Path.GetFileName(window.Image ?? "").ToLowerInvariant();
        if (window.ClassName == "ConsoleWindowClass" || image == "powershell.exe" || image == "pwsh.exe" || image == "conhost.exe")
            ConsoleEvents.Enqueue(source + ":" + window.Pid.ToString(CultureInfo.InvariantCulture) + ":" + window.ClassName + ":" + window.Title);
        if (String.Equals(window.Image, installerPath, StringComparison.OrdinalIgnoreCase) && window.ClassName == "#32770" && window.Title.IndexOf("Talking Quill", StringComparison.OrdinalIgnoreCase) >= 0)
        {
            lock (Sync) { if (installerWindow == null) installerWindow = window; }
        }
    }

    private static List<WindowRecord> EnumerateWindows()
    {
        var windows = new List<WindowRecord>();
        EnumWindows(delegate(IntPtr window, IntPtr data) { if (IsWindowVisible(window)) windows.Add(ReadWindow(window)); return true; }, IntPtr.Zero);
        return windows;
    }

    private static WindowRecord ReadWindow(IntPtr window)
    {
        var title = new StringBuilder(1024); GetWindowText(window, title, title.Capacity);
        var cls = new StringBuilder(256); GetClassName(window, cls, cls.Capacity);
        uint pid; GetWindowThreadProcessId(window, out pid);
        return new WindowRecord { Handle = window, Pid = pid, Title = title.ToString(), ClassName = cls.ToString(), Image = ProcessImage(pid) };
    }

    private static List<ProcessRecord> ProcessSnapshot()
    {
        var result = new List<ProcessRecord>();
        IntPtr snapshot = CreateToolhelp32Snapshot(0x00000002, 0);
        if (snapshot == new IntPtr(-1)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            var entry = new ProcessEntry32 { size = (uint)Marshal.SizeOf(typeof(ProcessEntry32)) };
            if (Process32First(snapshot, ref entry)) do
            {
                string executable = entry.executable ?? String.Empty;
                string lower = executable.ToLowerInvariant();
                bool inspectImage = lower == installerName.ToLowerInvariant() || lower == "powershell.exe" || lower == "pwsh.exe" || lower == "conhost.exe";
                result.Add(new ProcessRecord { Pid = entry.processId, ParentPid = entry.parentProcessId, Image = inspectImage ? (ProcessImage(entry.processId) ?? executable) : executable });
                entry.size = (uint)Marshal.SizeOf(typeof(ProcessEntry32));
            } while (Process32Next(snapshot, ref entry));
        }
        finally { CloseHandle(snapshot); }
        return result;
    }

    private static string ProcessImage(uint pid)
    {
        IntPtr process = OpenProcess(0x1000, false, pid);
        if (process == IntPtr.Zero) return null;
        try { var text = new StringBuilder(32768); int size = text.Capacity; return QueryFullProcessImageName(process, 0, text, ref size) ? text.ToString() : null; }
        finally { CloseHandle(process); }
    }

    private static void TrackProcess(uint pid, uint parent, string image, long creationTime) { RelevantProcesses[pid] = new ProcessRecord { Pid = pid, ParentPid = parent, Image = image, CreationTime = creationTime }; }
    private static void ResolveTrackedProcesses()
    {
        bool changed;
        do
        {
            changed = false;
            foreach (ProcessRecord process in StartedProcesses.Values)
            {
                string name = Path.GetFileName(process.Image ?? String.Empty).ToLowerInvariant();
                bool candidate = String.Equals(process.Image, installerPath, StringComparison.OrdinalIgnoreCase);
                if ((candidate || RelevantProcesses.ContainsKey(process.ParentPid)) && RelevantProcesses.TryAdd(process.Pid, process))
                {
                    if (name == "powershell.exe" || name == "pwsh.exe") Interlocked.Increment(ref powershellStarts);
                    changed = true;
                }
            }
        } while (changed);
    }
    private static long ProcessCreationTime(uint pid) { try { return Process.GetProcessById((int)pid).StartTime.ToUniversalTime().ToFileTimeUtc(); } catch { return 0; } }
    private static bool ProcessAlive(uint pid) { ProcessRecord record; return RelevantProcesses.TryGetValue(pid, out record) && record.CreationTime != 0 && ProcessCreationTime(pid) == record.CreationTime; }
    private static bool AnyTrackedProcessAlive() { ResolveTrackedProcesses(); return RelevantProcesses.Keys.Any(ProcessAlive); }
    private static void TerminateTrackedProcesses()
    {
        ResolveTrackedProcesses();
        foreach (uint pid in RelevantProcesses.Keys.OrderByDescending(value => value))
        {
            if (!ProcessAlive(pid)) continue;
            IntPtr process = OpenProcess(0x0401, false, pid);
            if (process == IntPtr.Zero) { ObserverErrors.Enqueue("could not open tracked process for cleanup: " + pid); continue; }
            try
            {
                long creation, exit, kernel, user;
                ProcessRecord record;
                if (!RelevantProcesses.TryGetValue(pid, out record) || !GetProcessTimes(process, out creation, out exit, out kernel, out user) || creation != record.CreationTime) continue;
                if (!TerminateProcess(process, 125)) ObserverErrors.Enqueue("could not terminate tracked process: " + pid);
            }
            finally { CloseHandle(process); }
        }
    }
    private static int ProcessExitCode(IntPtr handle) { uint code; return GetExitCodeProcess(handle, out code) ? unchecked((int)code) : -1; }
    [DllImport("kernel32.dll", SetLastError = true)] private static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

    private static void StartRegistryWatchers()
    {
        foreach (RegistryView view in new[] { RegistryView.Registry64, RegistryView.Registry32 })
        {
            RegistryKey machine = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, view);
            AddRegistryWatch(machine.OpenSubKey(@"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"), "machine-uninstall-" + view);
            AddRegistryWatch(machine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services"), "services-" + view);
            AddRegistryWatch(machine.OpenSubKey(@"SOFTWARE"), "machine-software-" + view);
            machine.Dispose();
        }
        RegistryKey user = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Default);
        AddRegistryWatch(user.OpenSubKey(@"SOFTWARE"), "user-software");
        user.Dispose();
    }

    private static void AddRegistryWatch(RegistryKey key, string name)
    {
        if (key == null) { ObserverErrors.Enqueue("registry watch key missing: " + name); RegistryWatchesReady.Signal(); return; }
        var watch = new RegistryWatch { Key = key, Name = name, EventHandle = CreateEvent(IntPtr.Zero, false, false, null) };
        if (watch.EventHandle == IntPtr.Zero) { key.Dispose(); ObserverErrors.Enqueue("registry watcher event failed: " + name); RegistryWatchesReady.Signal(); return; }
        watch.Thread = new Thread(delegate()
        {
            bool ready = false;
            while (!watch.Stop)
            {
                int status = RegNotifyChangeKeyValue(watch.Key.Handle.DangerousGetHandle(), true, 0x00000001 | 0x00000004, watch.EventHandle, true);
                if (!ready) { RegistryWatchesReady.Signal(); ready = true; }
                if (watch.Stop) break;
                if (status != 0) { ObserverErrors.Enqueue("registry watcher failed: " + watch.Name + ":" + status); break; }
                uint wait = WaitForSingleObject(watch.EventHandle, 0xffffffff);
                if (watch.Stop) break;
                if (wait != 0) { ObserverErrors.Enqueue("registry watcher wait failed: " + watch.Name + ":" + wait); break; }
                if (launched) MutationEvents.Enqueue("registry parent changed: " + watch.Name);
            }
        }) { IsBackground = true, Name = "registry-" + name };
        RegistryWatches.Add(watch); watch.Thread.Start();
    }

    private static void StopRegistryWatchers()
    {
        foreach (RegistryWatch watch in RegistryWatches) { watch.Stop = true; SetEvent(watch.EventHandle); }
        foreach (RegistryWatch watch in RegistryWatches)
        {
            if (!watch.Thread.Join(5000)) ObserverErrors.Enqueue("registry watcher did not stop: " + watch.Name);
            watch.Key.Dispose(); CloseHandle(watch.EventHandle);
        }
        RegistryWatches.Clear();
    }

    private static void StartProcessWatcher()
    {
        processStartWatcher = new ManagementEventWatcher(new WqlEventQuery("SELECT * FROM Win32_ProcessStartTrace"));
        processStartWatcher.EventArrived += delegate(object sender, EventArrivedEventArgs eventArgs)
        {
            try
            {
                uint pid = Convert.ToUInt32(eventArgs.NewEvent.Properties["ProcessID"].Value, CultureInfo.InvariantCulture);
                uint parent = Convert.ToUInt32(eventArgs.NewEvent.Properties["ParentProcessID"].Value, CultureInfo.InvariantCulture);
                string name = Convert.ToString(eventArgs.NewEvent.Properties["ProcessName"].Value, CultureInfo.InvariantCulture).ToLowerInvariant();
                if (!launched) return;
                long creationTime = ProcessCreationTime(pid);
                long baselineCreation;
                if (baselineProcesses.TryGetValue(pid, out baselineCreation) && baselineCreation == creationTime) return;
                string image = ProcessImage(pid) ?? name;
                StartedProcesses[pid] = new ProcessRecord { Pid = pid, ParentPid = parent, Image = image, CreationTime = creationTime };
                ResolveTrackedProcesses();
            }
            catch (Exception error) { ObserverErrors.Enqueue("process event: " + error.Message); }
        };
        processStartWatcher.Start();
    }

    private static void StopProcessWatcher()
    {
        if (processStartWatcher == null) return;
        try { processStartWatcher.Stop(); } catch (Exception error) { ObserverErrors.Enqueue("process watcher stop: " + error.Message); }
        processStartWatcher.Dispose(); processStartWatcher = null;
    }

    private static void StartFileWatchers()
    {
        foreach (string root in WatchRoots().Where(Directory.Exists).Distinct(StringComparer.OrdinalIgnoreCase))
        {
            var watcher = new FileSystemWatcher(root) { IncludeSubdirectories = true, NotifyFilter = NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.LastWrite | NotifyFilters.CreationTime, InternalBufferSize = 65536 };
            FileSystemEventHandler changed = delegate(object sender, FileSystemEventArgs eventArgs) { ObserveFileEvent(eventArgs.ChangeType.ToString(), eventArgs.FullPath); };
            RenamedEventHandler renamed = delegate(object sender, RenamedEventArgs eventArgs) { ObserveFileEvent("Renamed", eventArgs.OldFullPath + " -> " + eventArgs.FullPath); };
            ErrorEventHandler failed = delegate(object sender, ErrorEventArgs eventArgs) { ObserverErrors.Enqueue("filesystem watcher overflow: " + eventArgs.GetException().Message); };
            watcher.Created += changed; watcher.Changed += changed; watcher.Deleted += changed; watcher.Renamed += renamed; watcher.Error += failed; watcher.EnableRaisingEvents = true; FileWatchers.Add(watcher);
        }
    }

    private static void StopFileWatchers() { foreach (FileSystemWatcher watcher in FileWatchers) { watcher.EnableRaisingEvents = false; watcher.Dispose(); } FileWatchers.Clear(); }
    private static IEnumerable<string> WatchRoots()
    {
        yield return Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
        yield return programData;
        yield return Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms);
        yield return Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
        yield return Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        yield return Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        yield return Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        yield return Environment.GetFolderPath(Environment.SpecialFolder.Programs);
        yield return Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory);
        yield return Path.GetTempPath().TrimEnd(Path.DirectorySeparatorChar);
        yield return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "Tasks");
    }

    private static void ObserveFileEvent(string kind, string path)
    {
        if (!launched) return;
        if (ProtectedPath(path)) { protectedLeafObserved = true; return; }
        string userTemp = Path.GetTempPath().TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        if (path.IndexOf(userTemp, StringComparison.OrdinalIgnoreCase) >= 0 || RelevantPath(path))
            MutationEvents.Enqueue(kind + ":" + path);
    }

    private static void ObserveFilesystemSnapshot()
    {
        HashSet<string> leaves = ProtectedLeaves();
        if (!leaves.SetEquals(baselineProtectedLeaves)) protectedLeafObserved = true;
        if (leaves.Any(name => !System.Text.RegularExpressions.Regex.IsMatch(name, @"^\.Talking Quill\.Installer-[0-9a-f]{32}$"))) MutationEvents.Enqueue("invalid protected leaf");
        if (DurableFileSnapshot() != "clean") MutationEvents.Enqueue("durable filesystem snapshot changed");
    }

    private static bool RelevantPath(string path)
    {
        string value = path.ToLowerInvariant();
        return value.Contains("talking quill") || value.Contains("talkingquillkeyboardauthority") || value.Contains(".talking-quill") || value.Contains("talking-quill");
    }
    private static bool ProtectedPath(string path) { return path.StartsWith(programData + "\\.Talking Quill.Installer-", StringComparison.OrdinalIgnoreCase); }
    private static HashSet<string> ProtectedLeaves() { return new HashSet<string>(Directory.GetDirectories(programData, ".Talking Quill.Installer-*", SearchOption.TopDirectoryOnly).Select(Path.GetFileName), StringComparer.OrdinalIgnoreCase); }

    private static string DurableFileSnapshot()
    {
        string programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
        string[] paths = {
            Path.Combine(programFiles, "Talking Quill"), Path.Combine(programFiles, ".Talking Quill.stage1-backup"),
            Path.Combine(programFiles, ".Talking Quill.stage1-ambiguous-replacement"), Path.Combine(programFiles, ".Talking Quill.stage1-transaction.json"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Talking Quill"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Talking Quill.lnk"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "Talking Quill"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Talking Quill"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Programs), "Talking Quill"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory), "Talking Quill.lnk"),
            Path.Combine(programData, "Talking Quill", "KeyboardAuthority"), Path.Combine(programData, "Talking Quill", ".KeyboardAuthority.retirement-quarantine"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms), "Talking Quill.lnk"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory), "Talking Quill.lnk"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32", "Tasks", "TalkingQuillKeyboardAuthority") };
        return paths.Any(path => File.Exists(path) || Directory.Exists(path)) ? "dirty" : "clean";
    }

    private static string RegistrySnapshot()
    {
        try
        {
            foreach (RegistryView view in new[] { RegistryView.Registry64, RegistryView.Registry32 })
            using (RegistryKey machine = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, view))
            {
                if (machine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services\TalkingQuillKeyboardAuthority") != null || machine.OpenSubKey(@"SOFTWARE\com.talkingquill.app") != null) return "dirty";
                if (UninstallEntryExists(machine)) return "dirty";
            }
            using (RegistryKey user = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Default))
            {
                if (user.OpenSubKey(@"SOFTWARE\com.talkingquill.app") != null || UninstallEntryExists(user)) return "dirty";
            }
            return "clean";
        }
        catch (Exception error) { ObserverErrors.Enqueue("registry: " + error.Message); return "error"; }
    }

    private static bool UninstallEntryExists(RegistryKey root)
    {
        using (RegistryKey uninstall = root.OpenSubKey(@"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"))
            if (uninstall != null) foreach (string name in uninstall.GetSubKeyNames()) using (RegistryKey entry = uninstall.OpenSubKey(name))
                if (String.Equals(entry == null ? null : entry.GetValue("DisplayName") as string, "Talking Quill", StringComparison.OrdinalIgnoreCase)) return true;
        return false;
    }

    private static void SetKillOnClose(IntPtr job)
    {
        var limits = new ExtendedLimitInformation(); limits.BasicLimitInformation.LimitFlags = JobObjectLimitKillOnClose;
        int size = Marshal.SizeOf(typeof(ExtendedLimitInformation)); IntPtr memory = Marshal.AllocHGlobal(size);
        try { Marshal.StructureToPtr(limits, memory, false); if (!SetInformationJobObject(job, 9, memory, (uint)size)) throw new Win32Exception(Marshal.GetLastWin32Error()); }
        finally { Marshal.FreeHGlobal(memory); }
    }

    private static void WriteEvidence(string path, string[] args, int bytes, string before, string after, int subsystem, long maxGapMs, bool graceful, bool forced, int exitCode, bool pass)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        var processes = RelevantProcesses.Values.OrderBy(value => value.Pid).Select(value => "{\"pid\":" + value.Pid + ",\"parentPid\":" + value.ParentPid + ",\"image\":" + Json(Path.GetFileName(value.Image ?? String.Empty)) + "}");
        string window = installerWindow == null ? "null" : "{\"title\":" + Json(installerWindow.Title) + ",\"className\":" + Json(installerWindow.ClassName) + ",\"processId\":" + installerWindow.Pid + "}";
        string json = "{" +
            "\"schemaVersion\":2,\"installer\":" + Json(installerName) + ",\"architecture\":" + Json(args[3]) + "," +
            "\"sourceCommit\":" + Json(args[4]) + ",\"sourceTree\":" + Json(args[5]) + ",\"sourceTreeSha256\":" + Json(args[6]) + "," +
            "\"installerProvenanceSha256\":" + Json(args[7]) + ",\"provenanceDocumentSha256\":" + Json(args[8]) + "," +
            "\"bytes\":" + bytes + ",\"installerSha256Before\":" + Json(before) + ",\"installerSha256After\":" + Json(after) + "," +
            "\"outerPeSubsystem\":\"windows-gui\",\"outerPeSubsystemValue\":" + subsystem + ",\"nsisWindow\":" + window + "," +
            "\"monitoring\":{\"sampleIntervalMs\":5,\"maximumSampleGapMs\":" + maxGapMs + ",\"processSamples\":" + processSamples + ",\"windowSamples\":" + windowSamples + ",\"filesystemSamples\":" + filesystemSamples + ",\"registrySamples\":" + registrySamples + ",\"errors\":" + JsonArray(ObserverErrors) + "}," +
            "\"processes\":[" + String.Join(",", processes) + "],\"powershellProcessStarts\":" + powershellStarts + ",\"visibleConsoleWindowEvents\":" + JsonArray(ConsoleEvents) + "," +
            "\"filesystemOrRegistryMutationEvents\":" + JsonArray(MutationEvents) + ",\"transientProtectedBootstrapObserved\":" + (protectedLeafObserved ? "true" : "false") + ",\"protectedBootstrapBaselineRestored\":" + (ProtectedLeaves().SetEquals(baselineProtectedLeaves) ? "true" : "false") + "," +
            "\"cancellation\":{\"method\":\"WM_COMMAND/IDCANCEL\",\"graceful\":" + (graceful ? "true" : "false") + ",\"forcedCleanup\":" + (forced ? "true" : "false") + ",\"exitCode\":" + exitCode + "}," +
            "\"activeProcessesAfterTeardown\":" + ActiveProcessesJson() + ",\"noDurableInstallMutation\":" + (MutationEvents.IsEmpty ? "true" : "false") + ",\"exactBaselineRestored\":" + ((RegistrySnapshot() == "clean" && DurableFileSnapshot() == "clean" && ProtectedLeaves().SetEquals(baselineProtectedLeaves)) ? "true" : "false") + ",\"passed\":" + (pass ? "true" : "false") + "}\n";
        string pending = path + ".pending"; File.WriteAllText(pending, json, new UTF8Encoding(false)); if (File.Exists(path)) File.Delete(path); File.Move(pending, path);
    }

    private static string ActiveProcessesJson() { return "[" + String.Join(",", RelevantProcesses.Values.Where(value => ProcessAlive(value.Pid)).OrderBy(value => value.Pid).Select(value => value.Pid.ToString())) + "]"; }
    private static string JsonArray(IEnumerable<string> values) { return "[" + String.Join(",", values.Select(Json)) + "]"; }
    private static string Json(string value) { if (value == null) return "null"; var text = new StringBuilder("\""); foreach (char c in value) { if (c == '\\' || c == '\"') text.Append('\\').Append(c); else if (c == '\n') text.Append("\\n"); else if (c == '\r') text.Append("\\r"); else if (c < 32) text.Append("\\u").Append(((int)c).ToString("x4")); else text.Append(c); } return text.Append('\"').ToString(); }
    private static string Quote(string value) { return "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\""; }
    private static bool Hex(string value, int length) { return value != null && value.Length == length && value.All(c => c >= '0' && c <= '9' || c >= 'a' && c <= 'f'); }
    private static string Sha256(byte[] bytes) { using (SHA256 hash = SHA256.Create()) return String.Concat(hash.ComputeHash(bytes).Select(value => value.ToString("x2"))); }
    private static int PeSubsystem(byte[] bytes) { if (bytes.Length < 256 || BitConverter.ToUInt16(bytes, 0) != 0x5a4d) return -1; int pe = BitConverter.ToInt32(bytes, 0x3c); return pe >= 0 && pe + 94 <= bytes.Length && BitConverter.ToUInt32(bytes, pe) == 0x00004550 ? BitConverter.ToUInt16(bytes, pe + 92) : -1; }
    private static int Fail(string message) { Console.Error.WriteLine(message); return 1; }
}
