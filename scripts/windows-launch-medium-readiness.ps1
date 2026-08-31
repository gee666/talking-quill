param()
$ErrorActionPreference = 'Stop'
$exe = [Environment]::GetEnvironmentVariable('TALKING_QUILL_READINESS_EXE', 'Process')
$arguments = [Environment]::GetEnvironmentVariable('TALKING_QUILL_READINESS_ARGS', 'Process')
if ([string]::IsNullOrWhiteSpace($exe) -or [string]::IsNullOrWhiteSpace($arguments)) { exit 64 }
$source = @'
using System;
using System.Runtime.InteropServices;
public static class TqMediumLaunch {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct STARTUPINFO { public int cb; public string lpReserved; public string lpDesktop; public string lpTitle; public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags; public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError; }
  [StructLayout(LayoutKind.Sequential)] public struct PROCESS_INFORMATION { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr existing, uint access, IntPtr attributes, int level, int type, out IntPtr token);
  [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern bool CreateProcessWithTokenW(IntPtr token, uint logonFlags, string app, string commandLine, uint flags, IntPtr environment, string directory, ref STARTUPINFO startup, out PROCESS_INFORMATION process);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
  [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr handle, out uint exitCode);
  [DllImport("user32.dll")] static extern bool AllowSetForegroundWindow(int processId);
  public static int Launch(IntPtr explorer, string exe, string arguments, string directory) {
    IntPtr source=IntPtr.Zero, primary=IntPtr.Zero; PROCESS_INFORMATION pi=new PROCESS_INFORMATION();
    try {
      if(!OpenProcessToken(explorer, 0x0002|0x0008, out source)) return -Marshal.GetLastWin32Error();
      if(!DuplicateTokenEx(source, 0x0001|0x0002|0x0008|0x0080|0x0100, IntPtr.Zero, 2, 1, out primary)) return -Marshal.GetLastWin32Error();
      STARTUPINFO si=new STARTUPINFO(); si.cb=Marshal.SizeOf(si); si.lpDesktop="winsta0\\default"; si.dwFlags=1; si.wShowWindow=0;
      string commandLine="\""+exe+"\" "+arguments;
      if(!CreateProcessWithTokenW(primary, 1, exe, commandLine, 0x00000400|0x08000000, IntPtr.Zero, directory, ref si, out pi)) return -Marshal.GetLastWin32Error();
      AllowSetForegroundWindow(pi.dwProcessId);
      if(WaitForSingleObject(pi.hProcess, 120000) != 0) return -1460;
      uint exitCode;
      if(!GetExitCodeProcess(pi.hProcess, out exitCode)) return -Marshal.GetLastWin32Error();
      return unchecked((int)exitCode);
    } finally { if(pi.hThread!=IntPtr.Zero)CloseHandle(pi.hThread); if(pi.hProcess!=IntPtr.Zero)CloseHandle(pi.hProcess); if(primary!=IntPtr.Zero)CloseHandle(primary); if(source!=IntPtr.Zero)CloseHandle(source); }
  }
}
'@
Add-Type -TypeDefinition $source
$explorer = Get-Process explorer -ErrorAction Stop | Where-Object { $_.SessionId -eq (Get-Process -Id $PID).SessionId } | Select-Object -First 1
if ($null -eq $explorer) { exit 69 }
$result = [TqMediumLaunch]::Launch($explorer.Handle, $exe, $arguments, [IO.Path]::GetDirectoryName($exe))
if ($result -lt 0) { Write-Error "medium launch failed with Win32 $(-$result)"; exit 70 }
exit $result
