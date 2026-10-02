# Owned fixture: hold only the installed E2E executable open for reading,
# denying writes. Never terminate an application or change its permissions.
$ErrorActionPreference = 'Stop'
$stream = [IO.File]::Open($env:LOGINSIGHT_LOCK_FILE, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
try {
  [IO.File]::WriteAllText($env:LOGINSIGHT_LOCK_READY, 'ready')
  $deadline = [DateTime]::UtcNow.AddMinutes(3)
  while (-not [IO.File]::Exists($env:LOGINSIGHT_LOCK_RELEASE)) {
    if ([DateTime]::UtcNow -gt $deadline) { throw 'Lock fixture timed out.' }
    Start-Sleep -Milliseconds 100
  }
} finally { $stream.Dispose() }
