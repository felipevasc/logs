$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$session = $null
try {
    $request = [Console]::In.ReadToEnd() | ConvertFrom-Json
    $options = @{ ConnectionUri = $request.url; SessionOption = (New-PSSessionOption -OpenTimeout 20000 -OperationTimeout 120000) }
    if ($request.username) {
        if (!$request.password) { throw 'Informe a senha WinRM ou deixe o usuário vazio para autenticação integrada.' }
        $secure = ConvertTo-SecureString $request.password -AsPlainText -Force
        $options.Credential = [PSCredential]::new($request.username, $secure)
    }
    $session = New-PSSession @options
    $files = @(Invoke-Command -Session $session -ArgumentList @($request.paths, $request.maxBytes, $request.maxFiles) -ScriptBlock {
        param($paths, $maximum, $count)
        $ErrorActionPreference = 'Stop'
        $seen = @{}; $total = 0; $result = @(); $directories = 0
        $pending = [Collections.Generic.Queue[string]]::new()
        foreach ($path in $paths) {
            if (!(Test-Path -LiteralPath $path)) { continue }
            $pending.Enqueue($path)
        }
        while ($pending.Count) {
            $item = Get-Item -LiteralPath $pending.Dequeue()
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Links não podem ser coletados.' }
            if ($seen.ContainsKey($item.FullName)) { continue }
            $seen[$item.FullName] = $true
            if ($item.PSIsContainer) {
                if (++$directories -gt 8192) { throw 'Muitas subpastas; selecione um caminho mais específico.' }
                Get-ChildItem -LiteralPath $item.FullName -ErrorAction Stop | ForEach-Object {
                    if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Links não podem ser coletados.' }
                    if ($pending.Count -ge 8192) { throw 'Muitos caminhos; selecione uma pasta mais específica.' }
                    $pending.Enqueue($_.FullName)
                }
                continue
            }
            $total += $item.Length
            if ($result.Count -ge $count -or $total -gt $maximum) { throw 'A seleção excede os limites da coleta.' }
            $result += [PSCustomObject]@{ Path = $item.FullName; Length = $item.Length; Name = $item.Name }
        }
        if (!$result.Count) { throw 'Nenhum arquivo encontrado nos caminhos selecionados.' }
        $result
    })
    if ($request.test) {
        Invoke-Command -Session $session -ArgumentList @(,$files) -ScriptBlock {
            param($files)
            foreach ($file in $files) {
                if ($file.Path -like '*\winevt\Logs\*.evtx') {
                    $channel = [IO.Path]::GetFileNameWithoutExtension($file.Name).Replace('%4', '/')
                    try { Get-WinEvent -LogName $channel -MaxEvents 1 -ErrorAction Stop | Out-Null }
                    catch { if ($_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound*') { throw } }
                } else {
                    $stream = [IO.File]::Open($file.Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
                    try { $null = $stream.ReadByte() } finally { $stream.Dispose() }
                }
            }
        } | Out-Null
        @{ files = $files.Count } | ConvertTo-Json -Compress; exit 0
    }
    New-Item -ItemType Directory -Path $request.destination -Force | Out-Null
    $names = @(); $number = 0; $bytes = 0
    foreach ($file in $files) {
        $name = '{0:D4}-{1}' -f $number++, $file.Name
        $target = Join-Path $request.destination $name
        $remoteSnapshot = $null
        try {
            # Active EVTX files can be locked. Export a consistent snapshot of
            # the corresponding event channel before copying, preserving EVTX.
            $source = $file.Path
            if ($file.Path -like '*\winevt\Logs\*.evtx') {
                $remoteSnapshot = Invoke-Command -Session $session -ArgumentList $file.Name -ScriptBlock {
                    param($name)
                    $temporary = Join-Path $env:TEMP ('loginsight-' + [guid]::NewGuid().ToString() + '.evtx')
                    $channel = [IO.Path]::GetFileNameWithoutExtension($name).Replace('%4', '/')
                    & wevtutil.exe epl $channel $temporary /ow:true
                    if ($LASTEXITCODE -ne 0) { throw 'Falha ao exportar o canal EVTX selecionado.' }
                    $temporary
                }
                $source = $remoteSnapshot
            }
            Copy-Item -FromSession $session -LiteralPath $source -Destination $target -ErrorAction Stop
            $bytes += (Get-Item -LiteralPath $target).Length
            if ($bytes -gt $request.maxBytes) { throw 'A coleta excede o limite configurado.' }
            $names += $name
        } finally {
            if ($remoteSnapshot) { Invoke-Command -Session $session -ArgumentList $remoteSnapshot -ScriptBlock { param($path) Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue } | Out-Null }
        }
    }
    @{ files = @($names); warning = '' } | ConvertTo-Json -Compress
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
} finally {
    if ($session) { Remove-PSSession -Session $session -ErrorAction SilentlyContinue }
}
