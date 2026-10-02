# Keep Cargo's failure intact and retain evidence when the Windows loader fails
# before the Rust test harness starts. Uses only tools already on the runner.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$destination = Join-Path $PWD 'output/windows-native-diagnostics'
New-Item -ItemType Directory -Force -Path $destination | Out-Null

& cargo test --manifest-path src-tauri/Cargo.toml --release --locked --tests -- --test-threads=1 2>&1 |
    Tee-Object -FilePath (Join-Path $destination 'cargo-test.log')
$testExit = $LASTEXITCODE
if ($testExit -eq 0) { exit 0 }

try {
    # Cargo identifies the failed executable even when no harness output exists.
    $log = Get-Content (Join-Path $destination 'cargo-test.log') -Raw
    $failures = [regex]::Matches($log, '(?m)process didn.t exit successfully:\s*`([^`]+?\.exe)(?:\s[^`]*)?`')
    $target = [IO.Path]::GetFullPath((Join-Path $PWD 'src-tauri/target/release/deps'))
    $executables = @($failures | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    $selection = 'cargo-failed-process'
    if ($executables.Count -eq 0) {
        # Explicit fallback only; never label an inferred candidate as the failure.
        $selection = 'newest-unit-test-candidate; failed path absent from Cargo log'
        $executables = @(Get-ChildItem -LiteralPath $target -Filter 'loginsight_lib-*.exe' -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1 -ExpandProperty FullName)
    }
    $identities = @()
    foreach ($executable in $executables) {
        $path = [IO.Path]::GetFullPath($executable)
        if (!$path.StartsWith($target + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Cargo executable path is outside the expected test output directory.'
        }
        $file = Get-Item -LiteralPath $path
        Copy-Item -LiteralPath $path -Destination $destination
        $identities += [ordered]@{ path = $path; selection = $selection; size = $file.Length;
            modifiedUtc = $file.LastWriteTimeUtc.ToString('o'); sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $identities | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $destination 'executables.json') -Encoding utf8
    [ordered]@{
        cargoExitCode = $testExit
        sourceCommit = (& git rev-parse HEAD)
        runnerOS = $env:RUNNER_OS
        runnerArch = $env:RUNNER_ARCH
        imageOS = $env:ImageOS
        imageVersion = $env:ImageVersion
        runId = $env:GITHUB_RUN_ID
        runAttempt = $env:GITHUB_RUN_ATTEMPT
        powershellVersion = $PSVersionTable.PSVersion.ToString()
        windowsVersion = [Environment]::OSVersion.Version.ToString()
        processArchitecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    } | ConvertTo-Json | Set-Content (Join-Path $destination 'runner.json') -Encoding utf8
    & rustc -vV 2>&1 | Out-File (Join-Path $destination 'rustc.txt') -Encoding utf8
    & cargo -V 2>&1 | Out-File (Join-Path $destination 'cargo.txt') -Encoding utf8
    & rustup show 2>&1 | Out-File (Join-Path $destination 'rustup.txt') -Encoding utf8
    Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' |
        Select-Object ProductName, DisplayVersion, CurrentBuild, UBR, BuildLabEx |
        ConvertTo-Json | Set-Content (Join-Path $destination 'windows.json') -Encoding utf8
    $runtimePaths = @('HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\x64')
    @($runtimePaths | Where-Object { Test-Path $_ } | ForEach-Object {
        Get-ItemProperty $_ | Select-Object PSPath, Version, Installed, Major, Minor, Bld, Rbld
    }) | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $destination 'vc-runtime.json') -Encoding utf8
    @('ucrtbase.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'kernel32.dll', 'kernelbase.dll', 'ntdll.dll') |
        ForEach-Object {
            $path = Join-Path "$env:WINDIR/System32" $_
            if (Test-Path -LiteralPath $path) {
                $file = Get-Item -LiteralPath $path
                [ordered]@{ path = $file.FullName; fileVersion = $file.VersionInfo.FileVersion;
                    productVersion = $file.VersionInfo.ProductVersion }
            } else { [ordered]@{ path = $path; missing = $true } }
        } | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $destination 'system-runtime-files.json') -Encoding utf8

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $dumpbin = $null
    if (Test-Path -LiteralPath $vswhere) {
        $dumpbin = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe' |
            Select-Object -First 1
    }
    if (!$dumpbin) { 'Installed Visual Studio dumpbin was not found.' | Set-Content (Join-Path $destination 'dumpbin-unavailable.txt') }
    else {
        [ordered]@{ path = $dumpbin; fileVersion = (Get-Item -LiteralPath $dumpbin).VersionInfo.FileVersion } |
            ConvertTo-Json | Set-Content (Join-Path $destination 'dumpbin.json') -Encoding utf8
    }
    $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $mt = Get-ChildItem -LiteralPath $sdkBin -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName 'x64/mt.exe' } |
        Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (!$mt) { 'Installed Windows SDK mt.exe was not found.' | Set-Content (Join-Path $destination 'mt-unavailable.txt') }
    else {
        [ordered]@{ path = $mt; fileVersion = (Get-Item -LiteralPath $mt).VersionInfo.FileVersion } |
            ConvertTo-Json | Set-Content (Join-Path $destination 'mt.json') -Encoding utf8
    }
    foreach ($executable in $executables) {
        $path = [IO.Path]::GetFullPath($executable)
        $file = Get-Item -LiteralPath $path
        if ($dumpbin) {
            & $dumpbin /headers $path 2>&1 | Out-File (Join-Path $destination "$($file.Name).headers.txt") -Encoding utf8
            $importsPath = Join-Path $destination "$($file.Name).imports.txt"
            & $dumpbin /imports $path 2>&1 | Out-File $importsPath -Encoding utf8
            Select-String -Path $importsPath -Pattern 'comctl32|TaskDialogIndirect|\b345\b|\b159\b' -Context 2,6 |
                Out-File (Join-Path $destination "$($file.Name).common-controls-imports.txt") -Encoding utf8
        }
        if ($mt) {
            $manifest = Join-Path $destination "$($file.Name).manifest"
            & $mt -nologo "-inputresource:$path;#1" "-out:$manifest" 2>&1 |
                Out-File (Join-Path $destination "$($file.Name).manifest-extraction.txt") -Encoding utf8
            $manifestExit = $LASTEXITCODE
            [ordered]@{ resource = 1; exitCode = $manifestExit;
                extracted = ($manifestExit -eq 0 -and (Test-Path -LiteralPath $manifest)) } |
                ConvertTo-Json | Set-Content (Join-Path $destination "$($file.Name).manifest-status.json") -Encoding utf8
        }
    }
    if ($dumpbin) {
        # Compare actual installed exports. This does not assert which DLL the
        # failed process selected: its activation context depends on its manifest.
        $commonControls = @((Join-Path "$env:WINDIR/System32" 'comctl32.dll')) +
            @(Get-ChildItem -Path "$env:WINDIR/WinSxS/amd64_microsoft.windows.common-controls_*/comctl32.dll" -File -ErrorAction SilentlyContinue |
                Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 4 -ExpandProperty FullName)
        $libraries = @()
        $index = 0
        foreach ($path in ($commonControls | Sort-Object -Unique)) {
            $index++
            $exports = Join-Path $destination "comctl32-$index.exports.txt"
            if (!(Test-Path -LiteralPath $path)) {
                $libraries += [ordered]@{ path = $path; missing = $true }
                continue
            }
            $library = Get-Item -LiteralPath $path
            & $dumpbin /exports $path 2>&1 | Out-File $exports -Encoding utf8
            $dumpExit = $LASTEXITCODE
            Select-String -Path $exports -Pattern 'TaskDialogIndirect|^\s*345\s' -Context 1,1 |
                Out-File (Join-Path $destination "comctl32-$index.task-dialog.txt") -Encoding utf8
            $libraries += [ordered]@{ path = $path; fileVersion = $library.VersionInfo.FileVersion;
                exports = [IO.Path]::GetFileName($exports); dumpbinExitCode = $dumpExit }
        }
        $libraries | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $destination 'common-controls-libraries.json') -Encoding utf8
    }
    & node scripts/ci/windows-native-manifest-ab.mjs $destination 2>&1 |
        Out-File (Join-Path $destination 'manifest-ab-driver.log') -Encoding utf8
} catch {
    $_ | Out-String | Set-Content (Join-Path $destination 'diagnostic-error.txt') -Encoding utf8
    Write-Warning "Windows diagnostics were incomplete: $_"
} finally {
    # Even a diagnostic error must preserve the original Cargo status.
    exit $testExit
}
