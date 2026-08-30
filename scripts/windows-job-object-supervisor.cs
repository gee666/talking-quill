using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

internal static class WindowsJobObjectSupervisor
{
    private const uint CreateSuspended = 0x00000004;
    private const uint CreateUnicodeEnvironment = 0x00000400;
    private const uint Infinite = 0xffffffff;
    private const uint JobObjectLimitKillOnJobClose = 0x00002000;
    private const uint WaitObject0 = 0;
    private const uint WaitTimeout = 258;
    private const int SupervisorFailure = 125;
    private const int SupervisorTimeout = 124;

    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters
    {
        internal ulong ReadOperationCount;
        internal ulong WriteOperationCount;
        internal ulong OtherOperationCount;
        internal ulong ReadTransferCount;
        internal ulong WriteTransferCount;
        internal ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicLimitInformation
    {
        internal long PerProcessUserTimeLimit;
        internal long PerJobUserTimeLimit;
        internal uint LimitFlags;
        internal UIntPtr MinimumWorkingSetSize;
        internal UIntPtr MaximumWorkingSetSize;
        internal uint ActiveProcessLimit;
        internal UIntPtr Affinity;
        internal uint PriorityClass;
        internal uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ExtendedLimitInformation
    {
        internal BasicLimitInformation BasicLimitInformation;
        internal IoCounters IoInfo;
        internal UIntPtr ProcessMemoryLimit;
        internal UIntPtr JobMemoryLimit;
        internal UIntPtr PeakProcessMemoryUsed;
        internal UIntPtr PeakJobMemoryUsed;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicAccountingInformation
    {
        internal long TotalUserTime;
        internal long TotalKernelTime;
        internal long ThisPeriodTotalUserTime;
        internal long ThisPeriodTotalKernelTime;
        internal uint TotalPageFaultCount;
        internal uint TotalProcesses;
        internal uint ActiveProcesses;
        internal uint TotalTerminatedProcesses;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct StartupInfo
    {
        internal uint cb;
        internal string reserved;
        internal string desktop;
        internal string title;
        internal uint x;
        internal uint y;
        internal uint xSize;
        internal uint ySize;
        internal uint xCountChars;
        internal uint yCountChars;
        internal uint fillAttribute;
        internal uint flags;
        internal ushort showWindow;
        internal ushort reserved2Length;
        internal IntPtr reserved2;
        internal IntPtr standardInput;
        internal IntPtr standardOutput;
        internal IntPtr standardError;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInformation
    {
        internal IntPtr process;
        internal IntPtr thread;
        internal uint processId;
        internal uint threadId;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr securityAttributes, string name);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(
        IntPtr job, int informationClass, IntPtr information, uint informationLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool QueryInformationJobObject(
        IntPtr job, int informationClass, out BasicAccountingInformation information,
        uint informationLength, IntPtr returnLength);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcess(
        string applicationName, StringBuilder commandLine, IntPtr processAttributes,
        IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment,
        string currentDirectory, ref StartupInfo startupInfo, out ProcessInformation processInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint ResumeThread(IntPtr thread);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateJobObject(IntPtr job, uint exitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    private static int Main(string[] arguments)
    {
        if (arguments.Length != 3)
        {
            Console.Error.WriteLine("Usage: windows-job-object-supervisor <timeout-ms> <executable> <base64-raw-arguments>");
            return SupervisorFailure;
        }

        int timeout;
        if (!Int32.TryParse(arguments[0], out timeout) || timeout < 1)
        {
            Console.Error.WriteLine("The supervisor timeout is invalid.");
            return SupervisorFailure;
        }

        IntPtr job = IntPtr.Zero;
        ProcessInformation child = new ProcessInformation();
        bool assigned = false;
        try
        {
            job = CreateJobObject(IntPtr.Zero, null);
            if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            SetKillOnClose(job);

            string rawArguments = Encoding.UTF8.GetString(Convert.FromBase64String(arguments[2]));
            StringBuilder commandLine = new StringBuilder(Quote(arguments[1]));
            if (rawArguments.Length != 0) commandLine.Append(' ').Append(rawArguments);
            StartupInfo startup = new StartupInfo();
            startup.cb = (uint)Marshal.SizeOf(typeof(StartupInfo));
            if (!CreateProcess(arguments[1], commandLine, IntPtr.Zero, IntPtr.Zero, true,
                CreateSuspended | CreateUnicodeEnvironment, IntPtr.Zero,
                Environment.CurrentDirectory, ref startup, out child))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (!AssignProcessToJobObject(job, child.process))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            assigned = true;
            if (ResumeThread(child.thread) == UInt32.MaxValue)
                throw new Win32Exception(Marshal.GetLastWin32Error());
            CloseHandle(child.thread);
            child.thread = IntPtr.Zero;

            uint wait = WaitForSingleObject(child.process, (uint)timeout);
            bool timedOut = wait == WaitTimeout;
            if (wait != WaitObject0 && !timedOut)
                throw new Win32Exception(Marshal.GetLastWin32Error());

            uint childExit = SupervisorTimeout;
            if (!timedOut && !GetExitCodeProcess(child.process, out childExit))
                throw new Win32Exception(Marshal.GetLastWin32Error());

            if (!TerminateJobObject(job, timedOut ? (uint)SupervisorTimeout : childExit))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            RequireEmptyJob(job, 10000);
            return timedOut ? SupervisorTimeout : unchecked((int)childExit);
        }
        catch (Exception error)
        {
            Console.Error.WriteLine("Windows Job Object supervisor failed: " + error);
            if (assigned)
            {
                TerminateJobObject(job, SupervisorFailure);
                try { RequireEmptyJob(job, 10000); }
                catch (Exception cleanupError)
                {
                    Console.Error.WriteLine("Job descendant cleanup failed: " + cleanupError);
                }
            }
            return SupervisorFailure;
        }
        finally
        {
            if (child.thread != IntPtr.Zero) CloseHandle(child.thread);
            if (child.process != IntPtr.Zero) CloseHandle(child.process);
            if (job != IntPtr.Zero) CloseHandle(job);
        }
    }

    private static void SetKillOnClose(IntPtr job)
    {
        ExtendedLimitInformation limits = new ExtendedLimitInformation();
        limits.BasicLimitInformation.LimitFlags = JobObjectLimitKillOnJobClose;
        int size = Marshal.SizeOf(typeof(ExtendedLimitInformation));
        IntPtr memory = Marshal.AllocHGlobal(size);
        try
        {
            Marshal.StructureToPtr(limits, memory, false);
            if (!SetInformationJobObject(job, 9, memory, (uint)size))
                throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        finally { Marshal.FreeHGlobal(memory); }
    }

    private static void RequireEmptyJob(IntPtr job, int timeoutMilliseconds)
    {
        DateTime deadline = DateTime.UtcNow.AddMilliseconds(timeoutMilliseconds);
        while (true)
        {
            BasicAccountingInformation accounting;
            if (!QueryInformationJobObject(job, 1, out accounting,
                (uint)Marshal.SizeOf(typeof(BasicAccountingInformation)), IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (accounting.ActiveProcesses == 0) return;
            if (DateTime.UtcNow >= deadline)
                throw new TimeoutException("Job descendants did not terminate.");
            Thread.Sleep(50);
        }
    }

    private static string Quote(string value)
    {
        if (value.IndexOf('\"') >= 0) throw new ArgumentException("Executable path contains a quote.");
        return "\"" + value + "\"";
    }
}
