param(
    [int]$Modules = 250,
    [string]$Output = "benchmarks/reports/rust-medium-performance.json"
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$graphine = Join-Path $repo "target/debug/graphine.exe"
$benchmark = Join-Path $repo "target/debug/benchmark-core.exe"
$temp = Join-Path ([IO.Path]::GetTempPath()) ("graphine-rust-medium-" + [guid]::NewGuid().ToString("N"))
$corpus = Join-Path $temp "corpus"
$data = Join-Path $temp "data"

function Percentile([long[]]$values, [double]$percentile) {
    $ordered = @($values | Sort-Object)
    $index = [Math]::Min($ordered.Count - 1, [Math]::Ceiling($ordered.Count * $percentile) - 1)
    return $ordered[[Math]::Max(0, $index)]
}

function Invoke-McpQuery([string]$request) {
    $initialize = '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"rust-medium-benchmark","version":"1"}}}'
    $initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $result = (($initialize, $initialized, $request) -join "`n") | & $graphine --data-dir $data serve | Select-Object -Last 1
    $watch.Stop()
    if ($LASTEXITCODE -ne 0 -or -not $result) { throw "Rust MCP benchmark query failed" }
    return $watch.ElapsedMilliseconds
}

try {
    Push-Location $repo
    & cargo build -q -p graphine-cli -p graphine-rust-analyzer -p benchmark-core
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    & $benchmark generate-rust-medium --output $corpus --modules $Modules | Out-Null
    & $graphine --data-dir $data register $corpus --name rust-medium --language rust | Out-Null

    $watch = [Diagnostics.Stopwatch]::StartNew()
    $analysis = (& $graphine --data-dir $data analyze rust-medium --mode safe | ConvertFrom-Json)
    $watch.Stop()
    $status = (& $graphine --data-dir $data status rust-medium | ConvertFrom-Json)
    $files = @(Get-ChildItem -LiteralPath $corpus -Recurse -Filter *.rs -File)
    $sourceLines = ($files | ForEach-Object { (Get-Content -LiteralPath $_.FullName).Count } | Measure-Object -Sum).Sum
    $request = '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_symbol","arguments":{"project":"rust-medium","query":"S0125","detail":"summary","token_budget":800}}}'
    $latencies = @(1..20 | ForEach-Object { Invoke-McpQuery $request })
    $database = Join-Path $data "graphine.sqlite3"
    $report = [ordered]@{
        report_version = 1
        generated_at = (Get-Date).ToUniversalTime().ToString("o")
        environment = [ordered]@{
            os = [Environment]::OSVersion.VersionString
            architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
            rust = (& rustc --version)
            cargo = (& cargo --version)
        }
        corpus = [ordered]@{
            generator = "benchmark-core generate-rust-medium"
            module_count = $Modules
            file_count = $files.Count
            source_lines = $sourceLines
        }
        graph = [ordered]@{
            node_count = $status.node_count
            edge_count = $status.edge_count
            occurrence_count = $status.occurrence_count
            database_bytes = (Get-Item -LiteralPath $database).Length
        }
        timing_ms = [ordered]@{
            end_to_end_analysis = $watch.ElapsedMilliseconds
            worker_total = $analysis.summary.timings_ms.total
            ingestion = $analysis.ingestion_ms
            query_p50 = (Percentile $latencies 0.50)
            query_p95 = (Percentile $latencies 0.95)
        }
        resources = $analysis.summary.resources
        capabilities = $analysis.summary.capabilities
        assumptions = @(
            "Generated Cargo corpus has no external dependencies or build script",
            "Safe mode is offline and does not execute project code",
            "Query latency includes fresh STDIO process startup",
            "Results describe this environment only and are not universal performance claims"
        )
    }
    $destination = [IO.Path]::GetFullPath((Join-Path $repo $Output))
    New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($destination)) | Out-Null
    [IO.File]::WriteAllText($destination, ($report | ConvertTo-Json -Depth 7) + "`n", [Text.UTF8Encoding]::new($false))
    Write-Output $destination
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temp) -and $temp.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
