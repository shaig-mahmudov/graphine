param(
    [int]$Controllers = 100,
    [string]$Output = "benchmarks/reports/phase3-spring-medium-performance.json"
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$binary = Join-Path $repo "target/debug/graphine.exe"
$benchmark = Join-Path $repo "target/debug/benchmark-core.exe"
$temp = Join-Path ([IO.Path]::GetTempPath()) ("graphine-spring-medium-" + [guid]::NewGuid().ToString("N"))
$corpus = Join-Path $temp "corpus"
$data = Join-Path $temp "data"

function Percentile([long[]]$values, [double]$percentile) {
    $ordered = @($values | Sort-Object)
    $index = [Math]::Min($ordered.Count - 1, [Math]::Ceiling($ordered.Count * $percentile) - 1)
    return $ordered[[Math]::Max(0, $index)]
}

function Invoke-McpQuery([string]$request) {
    $initialize = '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"spring-medium-benchmark","version":"1"}}}'
    $initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $result = (($initialize, $initialized, $request) -join "`n") | & $binary --data-dir $data serve | Select-Object -Last 1
    $watch.Stop()
    if ($LASTEXITCODE -ne 0 -or -not $result) { throw "Spring MCP benchmark query failed" }
    return $watch.ElapsedMilliseconds
}

try {
    Push-Location $repo
    & mvn.cmd -q -f analyzer-jdt/pom.xml package
    if ($LASTEXITCODE -ne 0) { throw "analyzer package failed" }
    & cargo build -q -p graphine-cli -p benchmark-core
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    & $benchmark generate-spring-medium --output $corpus --controllers $Controllers | Out-Null
    & $binary --data-dir $data register $corpus --name spring-medium | Out-Null

    $watch = [Diagnostics.Stopwatch]::StartNew()
    $analysis = (& $binary --data-dir $data analyze spring-medium --mode safe | ConvertFrom-Json)
    $watch.Stop()
    $status = (& $binary --data-dir $data status spring-medium | ConvertFrom-Json)
    $files = @(Get-ChildItem -LiteralPath $corpus -Recurse -Filter *.java -File)
    $sourceLines = ($files | ForEach-Object { (Get-Content -LiteralPath $_.FullName).Count } | Measure-Object -Sum).Sum
    $routeIndex = [Math]::Min($Controllers - 1, [Math]::Floor($Controllers / 2))
    $path = "/api/c$($routeIndex.ToString('000'))/{id}"
    $request = @{jsonrpc="2.0";id=1;method="tools/call";params=@{name="get_endpoint_context";arguments=@{project="spring-medium";method="GET";path=$path;max_depth=4;token_budget=1200}}} | ConvertTo-Json -Compress -Depth 8
    $latencies = @(1..20 | ForEach-Object { Invoke-McpQuery $request })
    $database = Join-Path $data "graphine.sqlite3"
    $report = [ordered]@{
        report_version = 1
        generated_at = (Get-Date).ToUniversalTime().ToString("o")
        environment = [ordered]@{ os = [Environment]::OSVersion.VersionString; architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString(); rust = (& rustc --version) }
        corpus = [ordered]@{ generator = "benchmark-core generate-spring-medium"; controller_count = $Controllers; expected_route_count = $Controllers; file_count = $files.Count; source_lines = $sourceLines }
        graph = [ordered]@{ node_count = $status.node_count; edge_count = $status.edge_count; occurrence_count = $status.occurrence_count; database_bytes = (Get-Item -LiteralPath $database).Length }
        timing_ms = [ordered]@{ end_to_end_analysis = $watch.ElapsedMilliseconds; java_analysis = $analysis.summary.timings_ms.parsing; spring_semantic_pass = $analysis.summary.timings_ms.spring_semantic; protocol_serialization = $analysis.summary.timings_ms.serialization; rust_ingestion = $analysis.ingestion_ms; endpoint_context_preparation_p50 = (Percentile $latencies 0.50); endpoint_context_preparation_p95 = (Percentile $latencies 0.95) }
        memory_bytes = [ordered]@{ peak_java_heap_observed = $analysis.summary.resources.peak_memory_bytes }
        capabilities = $analysis.summary.capabilities
        assumptions = @("Generated Spring-shaped corpus uses canonical source annotation stubs and no external dependencies", "Endpoint-context timing uses the Phase 4 get_endpoint_context tool", "Query latency includes fresh STDIO process startup", "Results describe this environment only")
    }
    $destination = [IO.Path]::GetFullPath((Join-Path $repo $Output))
    New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($destination)) | Out-Null
    [IO.File]::WriteAllText($destination, ($report | ConvertTo-Json -Depth 7) + "`n", [Text.UTF8Encoding]::new($false))
    Write-Output $destination
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temp) -and $temp.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) { Remove-Item -LiteralPath $temp -Recurse -Force }
}
