param([Parameter(Mandatory=$true)][string]$NodePath,
      [Parameter(Mandatory=$true)][string]$ArgumentsBase64)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# A private kernel Job Object owns only the gated child and its descendants.
# KILL_ON_JOB_CLOSE also contains them if the supervisor itself is interrupted.
# No process-name matching, recycled PID lookup or antivirus setting is used.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

public static class PreviewJob {
  [StructLayout(LayoutKind.Sequential)] struct BasicLimit {
    public long ProcessTime, JobTime;
    public uint Flags;
    public UIntPtr MinWorkingSet, MaxWorkingSet;
    public uint ActiveLimit;
    public UIntPtr Affinity;
    public uint Priority, Scheduling;
  }
  [StructLayout(LayoutKind.Sequential)] struct IoCounters {
    public ulong ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes;
  }
  [StructLayout(LayoutKind.Sequential)] struct ExtendedLimit {
    public BasicLimit Basic;
    public IoCounters Io;
    public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
  }
  [StructLayout(LayoutKind.Sequential)] struct Accounting {
    public long UserTime, KernelTime, PeriodUserTime, PeriodKernelTime;
    public uint PageFaults, TotalProcesses, ActiveProcesses, TerminatedProcesses;
  }
  [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attributes, string name);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job, int kind, ref ExtendedLimit info, int size);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr job, int kind, out Accounting info, int size, IntPtr returned);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job, uint code);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  static void Check(bool ok) { if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error()); }

  static string Quote(string value) {
    var result = new StringBuilder("\"");
    int slashes = 0;
    foreach (char c in value) {
      if (c == '\\') { slashes++; continue; }
      result.Append('\\', c == '"' ? slashes * 2 + 1 : slashes);
      result.Append(c); slashes = 0;
    }
    result.Append('\\', slashes * 2); result.Append('"');
    return result.ToString();
  }

  public static int Run(string node, string[] args) {
    IntPtr job = CreateJobObject(IntPtr.Zero, null);
    if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
    Process child = null;
    try {
      var limits = new ExtendedLimit(); limits.Basic.Flags = 0x2000; // KILL_ON_JOB_CLOSE
      Check(SetInformationJobObject(job, 9, ref limits, Marshal.SizeOf(limits)));
      var command = new StringBuilder();
      foreach (var arg in args) { if (command.Length > 0) command.Append(' '); command.Append(Quote(arg)); }
      child = Process.Start(new ProcessStartInfo(node, command.ToString()) {
        UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true
      });
      // The handle came from Process.Start, not from a later PID lookup.
      Check(AssignProcessToJobObject(job, child.Handle));
      child.StandardInput.WriteLine("run"); child.StandardInput.Close();
      var stop = Task.Run(() => Console.ReadLine());
      while (!child.WaitForExit(50)) { if (stop.IsCompleted) return 130; }
      return child.ExitCode;
    } finally {
      try {
        Check(TerminateJobObject(job, 130));
        var deadline = DateTime.UtcNow.AddSeconds(10);
        while (true) {
          Accounting info;
          Check(QueryInformationJobObject(job, 1, out info, Marshal.SizeOf(typeof(Accounting)), IntPtr.Zero));
          if (info.ActiveProcesses == 0) break;
          if (DateTime.UtcNow >= deadline) throw new Exception("Owned process tree did not exit");
          Thread.Sleep(10);
        }
        // Assignment failure must not strand the gated child outside the job.
        if (child != null && !child.HasExited) { child.Kill(); child.WaitForExit(10000); }
      } finally {
        if (child != null) child.Dispose();
        CloseHandle(job);
      }
    }
  }
}
'@
try {
  $taskArguments = [string[]](ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($ArgumentsBase64))))
  exit [PreviewJob]::Run($NodePath, $taskArguments)
} catch {
  [Console]::Error.WriteLine($_.Exception.ToString())
  exit 125
}
