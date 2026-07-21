$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$data = Join-Path ([IO.Path]::GetTempPath()) ("graphine-phase3-e2e-" + [guid]::NewGuid().ToString("N"))
$binary = Join-Path $repo "target/debug/graphine.exe"
try {
    Push-Location $repo
    & mvn.cmd -q -f analyzer-jdt/pom.xml package
    if ($LASTEXITCODE -ne 0) { throw "analyzer build failed" }
    & cargo build -q -p graphine-cli
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    & $binary --data-dir $data register (Join-Path $repo "fixtures/spring-web") --name spring-web | Out-Null
    $analysis = (& $binary --data-dir $data analyze spring-web --mode safe | ConvertFrom-Json)
    if (-not $analysis.summary.capabilities.spring_static_semantics) { throw "Spring capability was not persisted" }
    $initialize = '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"phase3-e2e","version":"1"}}}'
    $initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    $request = '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"spring-web","stable_id":"route:POST:/api/v1/events","detail":"evidence","token_budget":4000}}}'
    $response = @( (($initialize, $initialized, $request) -join "`n") | & $binary --data-dir $data serve | ForEach-Object { $_ | ConvertFrom-Json } )[-1]
    $handler = "method:dev.graphine.fixture.web.EventController#create(dev.graphine.fixture.web.CreateEventRequest)"
    if (@($response.result.structuredContent.result.routes | Where-Object relationship -eq "HANDLED_BY" | ForEach-Object stable_id) -notcontains $handler) { throw "Spring route handler edge was absent after SQLite ingestion" }
    if (($response | ConvertTo-Json -Depth 20) -match "RUNTIME_CONFIRMED") { throw "Phase 3 emitted a runtime-confirmed claim" }
    Write-Output "Phase 3 E2E passed: Spring fixture -> semantic pass -> JSONL -> Rust -> SQLite -> MCP"
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $data) -and $data.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) { Remove-Item -LiteralPath $data -Recurse -Force }
}
