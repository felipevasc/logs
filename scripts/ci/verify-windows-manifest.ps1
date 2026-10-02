param([Parameter(Mandatory = $true)][ValidateSet('harness', 'app')][string]$Target)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$destination = Join-Path $PWD 'output/windows-manifest-verification'
New-Item -ItemType Directory -Force -Path $destination | Out-Null

function Read-Manifest([string]$Text) {
    $settings = [Xml.XmlReaderSettings]::new()
    $settings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
    $settings.XmlResolver = $null
    $reader = [Xml.XmlReader]::Create([IO.StringReader]::new($Text), $settings)
    try {
        $document = [Xml.XmlDocument]::new()
        $document.XmlResolver = $null
        $document.Load($reader)
        return ,$document
    } finally { $reader.Dispose() }
}

function Get-ManifestShape([Xml.XmlElement]$Element) {
    $attributes = @($Element.Attributes | Where-Object { $_.NamespaceURI -ne 'http://www.w3.org/2000/xmlns/' } |
        Sort-Object NamespaceURI, LocalName | ForEach-Object {
            [ordered]@{ name = $_.LocalName; ns = $_.NamespaceURI; value = $_.Value }
        })
    $children = @($Element.ChildNodes | ForEach-Object {
        if ($_ -is [Xml.XmlElement]) { Get-ManifestShape $_ }
        elseif ($_.NodeType -in @('Text', 'CDATA') -and ![string]::IsNullOrWhiteSpace($_.Value)) {
            [ordered]@{ text = $_.Value }
        }
    })
    return [ordered]@{ name = $Element.LocalName; ns = $Element.NamespaceURI; attributes = $attributes; children = $children }
}

function Get-ManifestSignature([Xml.XmlDocument]$Document) {
    return (Get-ManifestShape $Document.DocumentElement | ConvertTo-Json -Depth 64 -Compress)
}

$report = [ordered]@{ target = $Target; equivalent = $false; sourceCommit = (& git rev-parse HEAD) }
try {
    $expectedPath = Join-Path $PWD 'src-tauri/windows/tauri-default.manifest.xml'
    $expected = Read-Manifest (Get-Content -LiteralPath $expectedPath -Raw)
    $expectedShape = Get-ManifestSignature $expected
    # Exercise the comparison in the same PowerShell/.NET runtime as the gate.
    $variant = Read-Manifest $expected.OuterXml
    $identity = $variant.SelectSingleNode("//*[local-name()='assemblyIdentity']")
    $typeValue = $identity.GetAttribute('type')
    $identity.RemoveAttribute('type'); $identity.SetAttribute('type', $typeValue)
    if ((Get-ManifestSignature $variant) -ne $expectedShape) { throw 'Attribute order must not change manifest equivalence.' }
    foreach ($name in @('trustInfo', 'dpiAware')) {
        $changed = Read-Manifest $expected.OuterXml
        $null = $changed.DocumentElement.AppendChild($changed.CreateElement($name, 'urn:schemas-microsoft-com:asm.v3'))
        if ((Get-ManifestSignature $changed) -eq $expectedShape) { throw "Unexpected $name was not detected." }
    }
    if ($Target -eq 'harness') {
        $log = Get-Content 'output/windows-native-diagnostics/cargo-test.log' -Raw
        $log = [regex]::Replace($log, '\x1b\[[0-?]*[ -/]*[@-~]', '')
        $harnessMatches = [regex]::Matches($log, '(?m)Running unittests src[\\/]lib\.rs \(([^\r\n]+\.exe)\)')
        $paths = @($harnessMatches | ForEach-Object { [IO.Path]::GetFullPath($_.Groups[1].Value) } | Sort-Object -Unique)
        if ($paths.Count -ne 1) { throw 'Cargo log must identify exactly one executed library unit harness.' }
        $executable = $paths[0]
        $deps = [IO.Path]::GetFullPath((Join-Path $PWD 'src-tauri/target/release/deps'))
        if ([IO.Path]::GetDirectoryName($executable) -ine $deps -or [IO.Path]::GetFileName($executable) -notmatch '^loginsight_lib-[\w-]+\.exe$') {
            throw 'Unexpected unit harness path.'
        }
    } else { $executable = [IO.Path]::GetFullPath((Join-Path $PWD 'src-tauri/target/release/loginsight.exe')) }
    $report.executable = $executable
    $report.sha256 = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
    $report.expectedXmlSha256 = (Get-FileHash -LiteralPath $expectedPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Copy-Item -LiteralPath $expectedPath -Destination (Join-Path $destination 'tauri-default.manifest.xml')
    $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $mt = Get-ChildItem -LiteralPath $sdkBin -Directory | Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64/mt.exe' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (!$mt) { throw 'Installed Windows SDK mt.exe is required to verify the embedded manifest.' }
    $report.mt = $mt
    $manifest = Join-Path $destination "$Target.manifest"
    & $mt -nologo "-inputresource:$executable;#1" "-out:$manifest" 2>&1 |
        Out-File (Join-Path $destination "$Target-extraction.log") -Encoding utf8
    $report.mtExitCode = $LASTEXITCODE
    if ($LASTEXITCODE -ne 0) { throw 'Cannot extract the executable manifest resource #1.' }
    $actual = Read-Manifest (Get-Content -LiteralPath $manifest -Raw)
    $report.equivalent = (Get-ManifestSignature $actual) -eq $expectedShape
    $report.executableUnchanged = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant() -eq $report.sha256
    if (!$report.equivalent) { throw 'Embedded manifest differs from the unchanged Tauri default, including dependency, UAC or DPI structure.' }
    if (!$report.executableUnchanged) { throw 'Executable changed during read-only manifest verification.' }
    Write-Host "Verified $Target manifest: exact Tauri XML structure; executable unchanged."
} catch {
    $report.error = $_.ToString()
    throw
} finally {
    $report | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $destination "$Target.json") -Encoding utf8
}
