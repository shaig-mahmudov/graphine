param(
    [int]$Types = 250,
    [string]$Output = "benchmarks/reports/phase2.5-medium-performance.json"
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$binary = Join-Path $repo "target/debug/graphine.exe"
$benchmark = Join-Path $repo "target/debug/benchmark-core.exe"
$temp = Join-Path ([IO.Path]::GetTempPath()) ("graphine-medium-" + [guid]::NewGuid().ToString("N"))
$corpus = Join-Path $temp "corpus"
$data = Join-Path $temp "data"

function Percentile([long[]]$values, [double]$percentile) {
    $ordered = @($values | Sort-Object)
    $index = [Math]::Min($ordered.Count - 1, [Math]::Ceiling($ordered.Count * $percentile) - 1)
    return $ordered[[Math]::Max(0, $index)]
}

function Invoke-McpQuery([string]$request) {
    $initialize = '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"medium-benchmark","version":"1"}}}'
    $initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $result = (($initialize, $initialized, $request) -join "`n") | & $binary --data-dir $data serve | Select-Object -Last 1
    $watch.Stop()
    if ($LASTEXITCODE -ne 0 -or -not $result) { throw "MCP benchmark query failed" }
    return $watch.ElapsedMilliseconds
}

try {
    Push-Location $repo
    & mvn.cmd -q -f analyzer-jdt/pom.xml package
    if ($LASTEXITCODE -ne 0) { throw "analyzer package failed" }
    & cargo build -q -p graphine-cli -p benchmark-core
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    & $benchmark generate-medium --output $corpus --types $Types | Out-Null
    & $binary --data-dir $data register $corpus --name medium-corpus | Out-Null

    $stdout = Join-Path $temp "analyze.json"
    $stderr = Join-Path $temp "analyze.stderr"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $quotedData = '"' + $data + '"'
    $process = Start-Process -FilePath $binary -ArgumentList @("--data-dir", $quotedData, "analyze", "medium-corpus", "--mode", "safe") `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr -NoNewWindow -PassThru
    [long]$peakRustWorkingSet = 0
    while (-not $process.HasExited) {
        $process.Refresh()
        $peakRustWorkingSet = [Math]::Max($peakRustWorkingSet, $process.WorkingSet64)
        Start-Sleep -Milliseconds 20
    }
    $process.WaitForExit()
    $process.Refresh()
    $watch.Stop()
    if (($null -ne $process.ExitCode -and $process.ExitCode -ne 0) -or -not (Test-Path -LiteralPath $stdout)) { throw "medium analyzer failed: $([IO.File]::ReadAllText($stderr))" }
    $analysis = Get-Content -LiteralPath $stdout -Raw | ConvertFrom-Json
    $status = (& $binary --data-dir $data status medium-corpus | ConvertFrom-Json)
    $files = @(Get-ChildItem -LiteralPath $corpus -Recurse -Filter *.java -File)
    $sourceLines = ($files | ForEach-Object { (Get-Content -LiteralPath $_.FullName).Count } | Measure-Object -Sum).Sum
    $latencies = @()
    $request = '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_symbol","arguments":{"project":"medium-corpus","query":"C0125","detail":"summary","token_budget":800}}}'
    1..20 | ForEach-Object { $latencies += Invoke-McpQuery $request }
    $database = Join-Path $data "graphine.sqlite3"
    $javaVersion = (& cmd.exe /d /c "java -version 2>&1" | Select-Object -First 1).ToString()
    $report = [ordered]@{
        report_version = 1
        generated_at = (Get-Date).ToUniversalTime().ToString("o")
        environment = [ordered]@{ os = [Environment]::OSVersion.VersionString; architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString(); java = $javaVersion; rust = (& rustc --version) }
        corpus = [ordered]@{ generator = "benchmark-core generate-medium"; type_count = $Types; file_count = $files.Count; source_lines = $sourceLines }
        graph = [ordered]@{ node_count = $status.node_count; edge_count = $status.edge_count; occurrence_count = $status.occurrence_count; database_bytes = (Get-Item -LiteralPath $database).Length }
        timing_ms = [ordered]@{ end_to_end_analysis = $watch.ElapsedMilliseconds; analyzer = $analysis.summary.duration_ms; ingestion = $analysis.ingestion_ms; query_p50 = (Percentile $latencies 0.50); query_p95 = (Percentile $latencies 0.95) }
        memory_bytes = [ordered]@{ peak_java_heap_observed = $analysis.summary.resources.peak_memory_bytes; peak_rust_working_set = $peakRustWorkingSet }
        assumptions = @("Generated corpus has no external dependencies", "Query latency includes fresh STDIO process startup", "Peak Java heap is the analyzer's observation", "Results describe this environment only and are not universal performance claims")
    }
    $destination = [IO.Path]::GetFullPath((Join-Path $repo $Output))
    New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($destination)) | Out-Null
    [IO.File]::WriteAllText($destination, ($report | ConvertTo-Json -Depth 6) + "`n", [Text.UTF8Encoding]::new($false))
    Write-Output $destination
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temp) -and $temp.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
