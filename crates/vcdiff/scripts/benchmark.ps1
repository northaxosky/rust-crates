[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Source,
    [Parameter(Mandatory = $true)]
    [string]$Delta,
    [Parameter(Mandatory = $true)]
    [string]$Expected,
    [Parameter(Mandatory = $true)]
    [string]$Output,
    [Parameter(Mandatory = $true)]
    [string]$Report,
    [Parameter(Mandatory = $true)]
    [string]$Producer,
    [Parameter(Mandatory = $true)]
    [string]$ProducerVersion,
    [Parameter(Mandatory = $true)]
    [string]$Compression,
    [Parameter(Mandatory = $true)]
    [string]$PowerContext,
    [Parameter(Mandatory = $true)]
    [string]$DefenderContext,
    [Parameter(Mandatory = $true)]
    [string]$FilesystemContext,
    [string]$SourceSha256,
    [string]$DeltaSha256,
    [string]$ExpectedSha256,
    [switch]$SkipByteComparison
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$sourcePath = [IO.Path]::GetFullPath($Source)
$deltaPath = [IO.Path]::GetFullPath($Delta)
$expectedPath = [IO.Path]::GetFullPath($Expected)
$outputPath = [IO.Path]::GetFullPath($Output)
$reportPath = [IO.Path]::GetFullPath($Report)
if (Test-Path -LiteralPath $outputPath) {
    throw "Output already exists: $outputPath"
}
if (Test-Path -LiteralPath $reportPath) {
    throw "Report already exists: $reportPath"
}

function Get-Identity {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path
    $hash = Get-FileHash -LiteralPath $Path -Algorithm SHA256
    [ordered]@{
        bytes  = [int64]$item.Length
        sha256 = $hash.Hash.ToLowerInvariant()
    }
}

function Confirm-ExpectedHash {
    param(
        [string]$Label,
        [System.Collections.IDictionary]$Identity,
        [string]$Supplied
    )
    if ($Supplied -and $Identity.sha256 -ne $Supplied.ToLowerInvariant()) {
        throw "$Label SHA-256 does not match the supplied identity"
    }
}

function Confirm-PathFreeText {
    param(
        [string]$Label,
        [string]$Value
    )
    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "$Label must not be empty"
    }
    if ($Value.Contains('\') -or $Value.Contains('/') -or $Value -match '(?i)\b[a-z]:') {
        throw "$Label must not contain a filesystem path"
    }
    $Value
}

function Get-RepositoryIdentity {
    $commit = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not determine the repository commit'
    }
    $status = @(& git status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not determine the repository status'
    }
    $paths = @(& git ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not enumerate the repository files'
    }
    $entries = [Collections.Generic.List[string]]::new()
    foreach ($relativePath in ($paths | Sort-Object -Unique)) {
        $fullPath = Join-Path $repo $relativePath
        if (Test-Path -LiteralPath $fullPath -PathType Leaf) {
            $hash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
            $entries.Add("$relativePath`t$hash") | Out-Null
        }
        else {
            $entries.Add("$relativePath`t<deleted>") | Out-Null
        }
    }
    $treeBytes = [Text.Encoding]::UTF8.GetBytes(($entries -join "`n"))
    $hasher = [Security.Cryptography.SHA256]::Create()
    try {
        $treeHashBytes = $hasher.ComputeHash($treeBytes)
    }
    finally {
        $hasher.Dispose()
    }
    $treeHash = ($treeHashBytes | ForEach-Object { $_.ToString('x2') }) -join ''
    [ordered]@{
        commit       = $commit
        dirty        = $status.Count -ne 0
        tree_sha256  = $treeHash
    }
}

function Get-ProcessPeakSample {
    param([Diagnostics.Process]$Process)
    try {
        $Process.Refresh()
        $sample = [Math]::Max($Process.WorkingSet64, $Process.PeakWorkingSet64)
        if ($sample -gt 0) {
            return $sample
        }
    }
    catch [InvalidOperationException] {
        return $null
    }
    catch [ComponentModel.Win32Exception] {
        return $null
    }
    $null
}

function Compare-FileBytes {
    param(
        [string]$Left,
        [string]$Right
    )
    $leftStream = [IO.File]::OpenRead($Left)
    $rightStream = [IO.File]::OpenRead($Right)
    try {
        if ($leftStream.Length -ne $rightStream.Length) {
            return $false
        }
        $leftBuffer = [byte[]]::new(65536)
        $rightBuffer = [byte[]]::new(65536)
        while ($true) {
            $leftCount = $leftStream.Read($leftBuffer, 0, $leftBuffer.Length)
            $rightCount = $rightStream.Read($rightBuffer, 0, $rightBuffer.Length)
            if ($leftCount -ne $rightCount) {
                return $false
            }
            if ($leftCount -eq 0) {
                return $true
            }
            for ($index = 0; $index -lt $leftCount; $index++) {
                if ($leftBuffer[$index] -ne $rightBuffer[$index]) {
                    return $false
                }
            }
        }
    }
    finally {
        $leftStream.Dispose()
        $rightStream.Dispose()
    }
}

$sourceIdentity = Get-Identity $sourcePath
$deltaIdentity = Get-Identity $deltaPath
$expectedIdentity = Get-Identity $expectedPath
Confirm-ExpectedHash 'Source' $sourceIdentity $SourceSha256
Confirm-ExpectedHash 'Delta' $deltaIdentity $DeltaSha256
Confirm-ExpectedHash 'Expected target' $expectedIdentity $ExpectedSha256
$reportProducer = Confirm-PathFreeText 'Producer' $Producer
$reportProducerVersion = Confirm-PathFreeText 'Producer version' $ProducerVersion
$reportCompression = Confirm-PathFreeText 'Compression' $Compression
$reportPowerContext = Confirm-PathFreeText 'Power context' $PowerContext
$reportDefenderContext = Confirm-PathFreeText 'Defender context' $DefenderContext
$reportFilesystemContext = Confirm-PathFreeText 'Filesystem context' $FilesystemContext

Push-Location $repo
try {
    $buildOutput = & cargo build -p vcdiff-rs --example decode_file --release --locked --message-format=json 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Release decode_file build failed`n$($buildOutput -join "`n")"
    }
    $executable = $null
    foreach ($line in $buildOutput) {
        try {
            $message = $line.ToString() | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            continue
        }
        if (
            $message.reason -eq 'compiler-artifact' -and
            $message.target.name -eq 'decode_file' -and
            $message.target.kind -contains 'example' -and
            $message.executable
        ) {
            $executable = $message.executable
        }
    }
    if (-not $executable -or -not (Test-Path -LiteralPath $executable)) {
        throw 'Cargo did not report the release decode_file executable'
    }

    function Invoke-DecodeProcess {
        param([string]$Label)

        if (Test-Path -LiteralPath $outputPath) {
            throw 'Owned benchmark output unexpectedly exists'
        }
        try {
            $startInfo = [Diagnostics.ProcessStartInfo]::new()
            $startInfo.FileName = $executable
            $startInfo.WorkingDirectory = $repo
            $startInfo.UseShellExecute = $false
            $startInfo.RedirectStandardOutput = $true
            $startInfo.RedirectStandardError = $true
            foreach ($argument in @($sourcePath, $deltaPath, $outputPath)) {
                $startInfo.ArgumentList.Add($argument)
            }

            $process = [Diagnostics.Process]::new()
            $process.StartInfo = $startInfo
            $stopwatch = [Diagnostics.Stopwatch]::StartNew()
            if (-not $process.Start()) {
                throw "Could not start benchmark sample $Label"
            }
            $peak = 0L
            $metricSamples = 0
            $sample = Get-ProcessPeakSample $process
            if ($null -ne $sample) {
                $peak = [Math]::Max($peak, $sample)
                $metricSamples++
            }
            while (-not $process.WaitForExit(5)) {
                $sample = Get-ProcessPeakSample $process
                if ($null -ne $sample) {
                    $peak = [Math]::Max($peak, $sample)
                    $metricSamples++
                }
            }
            $process.WaitForExit()
            $stopwatch.Stop()
            $sample = Get-ProcessPeakSample $process
            if ($null -ne $sample) {
                $peak = [Math]::Max($peak, $sample)
                $metricSamples++
            }
            $stdout = $process.StandardOutput.ReadToEnd().Trim()
            $stderr = $process.StandardError.ReadToEnd().Trim()
            $exitCode = $process.ExitCode
            $process.Dispose()
            if ($exitCode -ne 0) {
                throw "Benchmark sample $Label failed with exit code $exitCode`n$stdout`n$stderr"
            }
            if ($metricSamples -eq 0) {
                throw "Benchmark sample $Label produced no working-set measurements"
            }

            $actual = Get-Identity $outputPath
            if (
                $actual.bytes -ne $expectedIdentity.bytes -or
                $actual.sha256 -ne $expectedIdentity.sha256
            ) {
                throw "Benchmark sample $Label produced the wrong target identity"
            }
            if (
                -not $SkipByteComparison -and
                -not (Compare-FileBytes $outputPath $expectedPath)
            ) {
                throw "Benchmark sample $Label failed byte comparison"
            }
            [ordered]@{
                label                  = $Label
                duration_ms            = [Math]::Round($stopwatch.Elapsed.TotalMilliseconds, 3)
                peak_working_set_bytes = $peak
            }
        }
        finally {
            if (Test-Path -LiteralPath $outputPath) {
                Remove-Item -LiteralPath $outputPath -Force
            }
        }
    }

    $warmup = Invoke-DecodeProcess 'warmup'
    $samples = @()
    for ($index = 1; $index -le 5; $index++) {
        $samples += Invoke-DecodeProcess "sample-$index"
    }
    $durations = @($samples | ForEach-Object { $_.duration_ms } | Sort-Object)
    $peaks = @($samples | ForEach-Object { $_.peak_working_set_bytes } | Sort-Object)
    $os = Get-CimInstance Win32_OperatingSystem
    $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
    $computer = Get-CimInstance Win32_ComputerSystem
    $repository = Get-RepositoryIdentity
    $result = [ordered]@{
        timestamp_utc = [DateTime]::UtcNow.ToString('o')
        rustc         = (& rustc -Vv) -join "`n"
        os             = [ordered]@{
            caption = $os.Caption
            version = $os.Version
            build   = $os.BuildNumber
        }
        cpu            = $cpu.Name
        ram_bytes      = [int64]$computer.TotalPhysicalMemory
        repo_commit    = $repository.commit
        repo_dirty     = $repository.dirty
        repo_tree_sha256 = $repository.tree_sha256
        environment    = [ordered]@{
            power_context      = $reportPowerContext
            defender_context   = $reportDefenderContext
            filesystem_context = $reportFilesystemContext
        }
        producer       = [ordered]@{
            name        = $reportProducer
            version     = $reportProducerVersion
            compression = $reportCompression
        }
        source         = $sourceIdentity
        delta          = $deltaIdentity
        target         = $expectedIdentity
        byte_comparison = -not $SkipByteComparison
        warmup         = $warmup
        samples        = $samples
        summary        = [ordered]@{
            median_duration_ms             = $durations[2]
            median_peak_working_set_bytes  = $peaks[2]
            maximum_peak_working_set_bytes = $peaks[-1]
        }
    }
    $json = $result | ConvertTo-Json -Depth 8
    $reportDirectory = Split-Path -Parent $reportPath
    New-Item -ItemType Directory -Force -Path $reportDirectory | Out-Null
    [IO.File]::WriteAllText($reportPath, $json, [Text.UTF8Encoding]::new($false))
    Write-Output $json
}
finally {
    if (Test-Path -LiteralPath $outputPath) {
        Remove-Item -LiteralPath $outputPath -Force
    }
    Pop-Location
}
