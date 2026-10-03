# Operate only a Retry/Cancel message box owned by this E2E's installer PID.
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class UpdateDialogFixture {
  public delegate bool EnumWindow(IntPtr window, IntPtr parameter);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindow action, IntPtr parameter);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr window, int id);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
}
'@
$installerProcess = [uint32]::Parse($env:LOGINSIGHT_DIALOG_PROCESS)
$buttonId = if ($env:LOGINSIGHT_DIALOG_ACTION -eq 'retry') { 4 } elseif ($env:LOGINSIGHT_DIALOG_ACTION -eq 'cancel') { 2 } else { throw 'Unsupported dialog action.' }
$script:button = [IntPtr]::Zero
$deadline = [DateTime]::UtcNow.AddSeconds(75)
while ($script:button -eq [IntPtr]::Zero) {
  if ([DateTime]::UtcNow -gt $deadline) { throw 'The owned installer Retry/Cancel dialog did not appear.' }
  [UpdateDialogFixture]::EnumWindows({
    param($window, $parameter)
    [uint32]$owner = 0
    [void][UpdateDialogFixture]::GetWindowThreadProcessId($window, [ref]$owner)
    # A normal NSIS wizard also has Cancel; require the Retry button too.
    if ($owner -eq $installerProcess -and [UpdateDialogFixture]::GetDlgItem($window, 4) -ne [IntPtr]::Zero) {
      $script:button = [UpdateDialogFixture]::GetDlgItem($window, $buttonId)
      return $false
    }
    return $true
  }, [IntPtr]::Zero) | Out-Null
  if ($script:button -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 200 }
}
[IO.File]::WriteAllText($env:LOGINSIGHT_DIALOG_READY, 'ready')
# The Node driver first releases its owned file holder when testing Retry.
while (-not [IO.File]::Exists($env:LOGINSIGHT_DIALOG_PROCEED)) {
  if ([DateTime]::UtcNow -gt $deadline) { throw 'The dialog fixture was not released.' }
  Start-Sleep -Milliseconds 100
}
if (-not [UpdateDialogFixture]::PostMessage($script:button, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)) { throw 'Could not click the owned installer button.' }
