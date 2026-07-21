param(
    [int]$Iterations = 100,
    [string]$Output = "benchmarks/reports/generated/phase4-mcp-performance.json"
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$data = Join-Path ([IO.Path]::GetTempPath()) ("graphine-phase4-benchmark-" + [guid]::NewGuid().ToString("N"))
$binary = Join-Path $repo "target/debug/graphine.exe"
function Invoke-McpBatch([string]$tool, [string]$arguments) {
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"phase4-benchmark","version":"1"}}}')
    $lines.Add('{"jsonrpc":"2.0","method":"notifications/initialized"}')
    for ($index = 1; $index -le $Iterations; $index++) {
        $lines.Add(('{"jsonrpc":"2.0","id":' + $index + ',"method":"tools/call","params":{"name":"' + $tool + '","arguments":' + $arguments + '}}'))
    }
    $stderr = Join-Path $data ($tool + ".stderr.log")
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $capture = @(($lines -join "`n") | & $binary --data-dir $data serve 2> $stderr | ForEach-Object { $_.ToString() })
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference
    if ($exitCode -ne 0) { throw "MCP server failed for $tool" }
    $capture += @(Get-Content -LiteralPath $stderr)
    $durations = @($capture | ForEach-Object { if ($_ -match 'duration_ms=(\d+)') { [int]$matches[1] } }) | Sort-Object
    $responses = @($capture | Where-Object { $_.StartsWith('{') } | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object id -ne 0)
    if ($durations.Count -ne $Iterations -or $responses.Count -ne $Iterations) { throw "incomplete MCP benchmark capture for $tool" }
    $p95Index = [Math]::Max(0, [Math]::Ceiling($durations.Count * 0.95) - 1)
    $tokens = @($responses | ForEach-Object { $_.result.structuredContent.budget.estimated_tokens } | Sort-Object)
    return [ordered]@{
        iterations = $Iterations
        warm_p95_ms = $durations[$p95Index]
        median_estimated_response_tokens = $tokens[[Math]::Floor($tokens.Count / 2)]
        maximum_ms = $durations[-1]
    }
}
try {
    Push-Location $repo
    & cargo build -q -p graphine-cli
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    & $binary --data-dir $data register (Join-Path $repo "fixtures/spring-web") --name spring-web | Out-Null
    & $binary --data-dir $data analyze spring-web --mode safe | Out-Null
    $endpoint = Invoke-McpBatch "get_endpoint_context" '{"project":"spring-web","method":"POST","path":"/api/v1/events","max_depth":4,"token_budget":1400}'
    $evidence = Invoke-McpBatch "get_evidence" '{"project":"spring-web","references":[{"file":"src/main/java/dev/graphine/fixture/web/EventController.java","start_line":28,"end_line":32}],"context_lines":2,"max_total_lines":20,"token_budget":1200}'
    $report = [ordered]@{
        schema_version = "1.0.0"
        measured_at_utc = [DateTime]::UtcNow.ToString("o")
        fixture = "spring-web"
        cache_state = "warm-within-single-server-process"
        endpoint_context = $endpoint
        evidence = $evidence
        notes = @("Estimated response tokens use Graphine's deterministic character estimator, not provider input-token measurements.")
    }
    $destination = if ([IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $repo $Output }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    $report | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 $destination
    Write-Output "wrote $destination"
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $data) -and $data.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $data -Recurse -Force
    }
}
