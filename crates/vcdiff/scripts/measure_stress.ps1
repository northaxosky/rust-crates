[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ReportPath
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$report = [IO.Path]::GetFullPath($ReportPath)
if (Test-Path -LiteralPath $report) {
    throw "Report already exists: $report"
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

Push-Location $repo
try {
    $buildOutput = & cargo test -p vcdiff-rs --release --test stress_over_2gib --no-run --message-format=json 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Release stress-test build failed`n$($buildOutput -join "`n")"
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
            $message.target.name -eq 'stress_over_2gib' -and
            $message.profile.test -and
            $message.executable
        ) {
            $executable = $message.executable
        }
    }
    if (-not $executable) {
        throw 'Cargo did not report the stress-test executable'
    }

    function Invoke-StressProcess {
        param(
            [string]$Executable,
            [int]$Windows,
            [string]$Label
        )

        $startInfo = [Diagnostics.ProcessStartInfo]::new()
        $startInfo.FileName = $Executable
        $startInfo.WorkingDirectory = $repo
        $startInfo.UseShellExecute = $false
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $startInfo.Environment['VCDIFF_STRESS_WINDOWS'] = $Windows.ToString()
        foreach ($argument in @('stress_over_2gib', '--ignored', '--exact', '--nocapture')) {
            $startInfo.ArgumentList.Add($argument)
        }

        $process = [Diagnostics.Process]::new()
        $process.StartInfo = $startInfo
        $stopwatch = [Diagnostics.Stopwatch]::StartNew()
        if (-not $process.Start()) {
            throw "Could not start $Label stress process"
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
            throw "$Label stress process failed with exit code $exitCode`n$stdout`n$stderr"
        }
        if ($metricSamples -eq 0) {
            throw "$Label stress process produced no working-set measurements"
        }
        [ordered]@{
            label                  = $Label
            windows                = $Windows
            target_bytes           = [int64]$Windows * 524288
            duration_ms            = [Math]::Round($stopwatch.Elapsed.TotalMilliseconds, 3)
            peak_working_set_bytes = $peak
            test_output            = $stdout
        }
    }

    $control = Invoke-StressProcess -Executable $executable -Windows 512 -Label 'control-256mib'
    $large = Invoke-StressProcess -Executable $executable -Windows 4097 -Label 'large-2gib'
    $relativeThreshold = $control.peak_working_set_bytes + 64MB
    $absoluteThreshold = 256MB
    $relativePassed = $large.peak_working_set_bytes -le $relativeThreshold
    $absolutePassed = $large.peak_working_set_bytes -le $absoluteThreshold

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
        control        = $control
        large          = $large
        thresholds     = [ordered]@{
            relative_limit_bytes = $relativeThreshold
            absolute_limit_bytes = $absoluteThreshold
            relative_passed      = $relativePassed
            absolute_passed      = $absolutePassed
        }
    }
    $json = $result | ConvertTo-Json -Depth 8
    $reportDirectory = Split-Path -Parent $report
    New-Item -ItemType Directory -Force -Path $reportDirectory | Out-Null
    [IO.File]::WriteAllText($report, $json, [Text.UTF8Encoding]::new($false))
    Write-Output $json
    if (-not $relativePassed -or -not $absolutePassed) {
        throw 'Stress-test working-set threshold failed'
    }
}
finally {
    Pop-Location
}
