$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$dataDir = Join-Path ([IO.Path]::GetTempPath()) ("graphine-phase2-e2e-" + [guid]::NewGuid().ToString("N"))
$binary = Join-Path $repo "target/debug/graphine.exe"

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
    $response = ($request | & $binary --data-dir $dataDir serve) | ConvertFrom-Json
    $expected = "method:dev.graphine.fixture.core.Operations#combine(java.lang.String,java.lang.String)"
    $wrong = "method:dev.graphine.fixture.core.Operations#combine(int,int)"
    $targets = @($response.result.structuredContent.facts | Where-Object kind -eq "CALLS" | ForEach-Object target)
    if ($targets -notcontains $expected) { throw "expected overloaded call target was absent" }
    if ($targets -contains $wrong) { throw "incorrect overloaded call target was present" }
    Write-Output "Phase 2 E2E passed: fixture -> JDT -> JSONL -> Rust -> SQLite -> MCP"
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $resolved = [IO.Path]::GetFullPath($dataDir)
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolved)) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
