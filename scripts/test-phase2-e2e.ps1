$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$dataDir = Join-Path ([IO.Path]::GetTempPath()) ("graphine-phase2-e2e-" + [guid]::NewGuid().ToString("N"))
$binary = Join-Path $repo "target/debug/graphine.exe"

function Invoke-GraphineMcp([string]$toolRequest) {
    $initialize = '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"phase2-e2e","version":"1"}}}'
    $initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    $lines = (($initialize, $initialized, $toolRequest) -join "`n") | & $binary --data-dir $dataDir serve
    return @($lines | ForEach-Object { $_ | ConvertFrom-Json })[-1]
}

try {
    Push-Location (Join-Path $repo "analyzer-jdt")
    & mvn.cmd -q package
    if ($LASTEXITCODE -ne 0) { throw "analyzer build failed" }
    Pop-Location

    Push-Location $repo
    & cargo build -q -p graphine-cli
    if ($LASTEXITCODE -ne 0) { throw "Rust build failed" }
    & $binary --data-dir $dataDir register (Join-Path $repo "fixtures/java-core") --name java-core | Out-Null
    & $binary --data-dir $dataDir analyze java-core --mode safe | Out-Null
    $request = '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"java-core","stable_id":"method:dev.graphine.fixture.core.CoreScenario#run()","detail":"detailed","token_budget":4000}}}'
    $response = Invoke-GraphineMcp $request
    $expected = "method:dev.graphine.fixture.core.Operations#combine(java.lang.String,java.lang.String)"
    $wrong = "method:dev.graphine.fixture.core.Operations#combine(int,int)"
    $targets = @($response.result.structuredContent.result.callees | Where-Object relationship -eq "CALLS" | ForEach-Object stable_id)
    if ($targets -notcontains $expected) { throw "expected overloaded call target was absent" }
    if ($targets -contains $wrong) { throw "incorrect overloaded call target was present" }

    $occurrenceRequest = '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"java-core","stable_id":"method:dev.graphine.fixture.core.Operations#repeatTrim(java.lang.String)","detail":"detailed","token_budget":4000}}}'
    $occurrenceResponse = Invoke-GraphineMcp $occurrenceRequest
    $trim = @($occurrenceResponse.result.structuredContent.result.callees | Where-Object { $_.relationship -eq "CALLS" -and $_.stable_id -eq "method:java.lang.String#trim()" })
    if ($trim.Count -ne 1) { throw "graph traversal did not retain exactly one logical trim edge" }
    if ($trim[0].occurrence_count -ne 3) { throw "repeated trim call occurrences were not preserved" }
    if (@($trim[0].evidence_refs).Count -ne 3) { throw "trim occurrence evidence was not queryable" }
    Write-Output "Phase 2 E2E passed: fixture -> JDT -> JSONL -> Rust -> SQLite -> MCP"
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $resolved = [IO.Path]::GetFullPath($dataDir)
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolved)) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
