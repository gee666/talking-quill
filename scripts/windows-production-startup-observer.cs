using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;
using System.Windows.Automation;

public sealed class StartupElement {
    public string Name;
    public string ControlType;
    public int Depth;
    public bool Offscreen;
}
public sealed class StartupWindow {
    public long Handle;
    public int ProcessId;
    public string Title;
    public string ClassName;
    public int Width;
    public int Height;
    public List<StartupElement> Elements = new List<StartupElement>();
    public string AccessibilityError;
}
public sealed class StartupCapture {
    public Process Process;
    private readonly StringBuilder stdout = new StringBuilder();
    private readonly StringBuilder stderr = new StringBuilder();
    private long stdoutChars;
    private long stderrChars;
    private Task outputTask;
    private Task errorTask;
    public const int Limit = 262144;
    public StartupCapture(string executable, string directory) {
        Process = new Process();
        Process.StartInfo = new ProcessStartInfo(executable) {
            WorkingDirectory = directory, UseShellExecute = false,
            RedirectStandardOutput = true, RedirectStandardError = true,
            CreateNoWindow = false
        };
        if (!Process.Start()) throw new Exception("Production process did not start");
        outputTask = Drain(Process.StandardOutput, stdout, true);
        errorTask = Drain(Process.StandardError, stderr, false);
    }
    private Task Drain(System.IO.StreamReader reader, StringBuilder target, bool output) {
        return Task.Run(async delegate {
            char[] buffer = new char[4096];
            for (;;) {
                int count = await reader.ReadAsync(buffer, 0, buffer.Length);
                if (count == 0) return;
                lock (target) {
                    if (output) stdoutChars += count; else stderrChars += count;
                    int keep = Math.Min(count, Limit - target.Length);
                    if (keep > 0) target.Append(buffer, 0, keep);
                }
            }
        });
    }
    public string Output() { lock (stdout) return stdout.ToString(); }
    public string Error() { lock (stderr) return stderr.ToString(); }
    public long OutputChars() { lock (stdout) return stdoutChars; }
    public long ErrorChars() { lock (stderr) return stderrChars; }
    public void Finish() { try { Task.WaitAll(new Task[] { outputTask, errorTask }, 2000); } catch {} }
}
public static class StartupObserver {
    private delegate bool EnumCallback(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumCallback callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int limit);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder text, int limit);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] private static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] private static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
    [StructLayout(LayoutKind.Sequential)] private struct Rect { public int Left, Top, Right, Bottom; }
    public static StartupWindow[] Windows(int processId) {
        var windows = new List<StartupWindow>();
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint pid;
            GetWindowThreadProcessId(window, out pid);
            if (pid != processId || !IsWindowVisible(window)) return true;
            var title = new StringBuilder(512);
            var className = new StringBuilder(128);
            GetWindowText(window, title, title.Capacity);
            GetClassName(window, className, className.Capacity);
            Rect rect;
            if (!GetWindowRect(window, out rect)) return true;
            windows.Add(new StartupWindow { Handle = window.ToInt64(), ProcessId = processId,
                Title = title.ToString(), ClassName = className.ToString(),
                Width = rect.Right - rect.Left, Height = rect.Bottom - rect.Top });
            return windows.Count < 16;
        }, IntPtr.Zero);
        return windows.ToArray();
    }
    public static StartupWindow Accessibility(StartupWindow window) {
        // UIAutomation providers can hang. Never let one block the polling deadline.
        var task = Task.Run(delegate {
            var elements = new List<StartupElement>();
            var queue = new Queue<Tuple<AutomationElement, int>>();
            queue.Enqueue(Tuple.Create(AutomationElement.FromHandle(new IntPtr(window.Handle)), 0));
            while (queue.Count > 0 && elements.Count < 512) {
                var item = queue.Dequeue();
                var current = item.Item1.Current;
                string name = current.Name ?? "";
                elements.Add(new StartupElement { Name = name.Substring(0, Math.Min(name.Length, 256)),
                    ControlType = current.ControlType.ProgrammaticName, Depth = item.Item2,
                    Offscreen = current.IsOffscreen });
                if (item.Item2 >= 16) continue;
                var child = TreeWalker.ControlViewWalker.GetFirstChild(item.Item1);
                while (child != null && queue.Count + elements.Count < 512) {
                    queue.Enqueue(Tuple.Create(child, item.Item2 + 1));
                    child = TreeWalker.ControlViewWalker.GetNextSibling(child);
                }
            }
            return elements;
        });
        try {
            if (task.Wait(3000)) window.Elements = task.Result;
            else window.AccessibilityError = "UIAutomation timed out after 3000ms";
        } catch (Exception error) { window.AccessibilityError = error.GetBaseException().Message; }
        return window;
    }
    public static bool Close(long handle) { return PostMessage(new IntPtr(handle), 0x0010, IntPtr.Zero, IntPtr.Zero); }
    public static void Screenshot(long handle, string path) {
        var task = Task.Run(delegate { CaptureWindow(handle, path); });
        if (!task.Wait(5000)) throw new Exception("Window screenshot timed out after 5000ms");
    }
    private static void CaptureWindow(long handle, string path) {
        IntPtr window = new IntPtr(handle);
        Rect rect;
        if (!GetWindowRect(window, out rect)) throw new Exception("Cannot read screenshot window bounds");
        int width = rect.Right - rect.Left, height = rect.Bottom - rect.Top;
        if (width < 1 || height < 1 || width > 4096 || height > 4096) throw new Exception("Screenshot bounds rejected");
        SetForegroundWindow(window);
        using (var bitmap = new Bitmap(width, height)) {
            using (var graphics = Graphics.FromImage(bitmap)) {
                IntPtr dc = graphics.GetHdc();
                bool printed;
                try { printed = PrintWindow(window, dc, 2); } finally { graphics.ReleaseHdc(dc); }
                if (!printed) throw new Exception("PrintWindow failed");
            }
            bitmap.Save(path, ImageFormat.Png);
        }
    }
}
