param([switch]$InstallMissing)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$evidence = [ordered]@{ imageOS = $env:ImageOS; imageVersion = $env:ImageVersion; interactive = [Environment]::UserInteractive; webview2 = $null; installedByCheck = $false; error = $null }
New-Item -ItemType Directory -Force -Path output | Out-Null

# Microsoft documents these keys separately from the installed Edge browser:
# https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution
function Get-WebViewRuntimeVersion {
  foreach ($key in @(
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
  )) {
    $value = (Get-ItemProperty -LiteralPath $key -Name pv -ErrorAction SilentlyContinue).pv
    $parsed = $null
    if ([version]::TryParse($value, [ref]$parsed) -and $parsed -gt [version]'0.0.0.0') { return $value }
  }
  return $null
}

try {
  Get-Command powershell.exe, Get-CimInstance | Out-Null
  if (-not $evidence.interactive) { throw 'Native acceptance requires an interactive Windows desktop session for WM_CLOSE.' }
  $evidence.webview2 = Get-WebViewRuntimeVersion
  if (-not $evidence.webview2 -and $InstallMissing) {
    if (-not $env:RUNNER_TEMP -or $env:GITHUB_ACTIONS -ne 'true') { throw 'Automatic runtime installation is restricted to the CI runner.' }
    $bootstrapper = Join-Path $env:RUNNER_TEMP ('webview2-' + [guid]::NewGuid().ToString('N') + '.exe')
    try {
      Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrapper
      $signature = Get-AuthenticodeSignature -LiteralPath $bootstrapper
      if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
        throw 'WebView2 bootstrapper must have a valid Microsoft signature.'
      }
      $installer = Start-Process -FilePath $bootstrapper -ArgumentList '/silent', '/install' -WindowStyle Hidden -Wait -PassThru
      if ($installer.ExitCode -ne 0) { throw "WebView2 installation failed ($($installer.ExitCode))." }
      $evidence.installedByCheck = $true
      $evidence.webview2 = Get-WebViewRuntimeVersion
    } finally {
      if (Test-Path -LiteralPath $bootstrapper) { Remove-Item -LiteralPath $bootstrapper }
    }
  }
  if (-not $evidence.webview2) { throw 'WebView2 Evergreen Runtime is required. Install it from https://developer.microsoft.com/microsoft-edge/webview2/ and rerun.' }
  Write-Output "WebView2 $($evidence.webview2); interactive desktop verified."
} catch {
  $evidence.error = $_.Exception.Message
  throw
} finally {
  $evidence | ConvertTo-Json | Set-Content -LiteralPath output/native-environment.json -Encoding utf8
}
