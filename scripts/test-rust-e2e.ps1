$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$data = Join-Path ([IO.Path]::GetTempPath()) ("graphine-rust-e2e-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $data | Out-Null

function Invoke-GraphineMcp([string]$toolRequest) {
    $initialize = '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"rust-e2e","version":"1"}}}'
    $initialized = '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    $lines = (($initialize, $initialized, $toolRequest) -join "`n") | & target/debug/graphine.exe --data-dir $data serve
    return @($lines | ForEach-Object { $_ | ConvertFrom-Json })[-1]
}

try {
    Push-Location $repo
    & cargo build -q -p graphine-cli -p graphine-rust-analyzer
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
    & target/debug/graphine.exe --data-dir $data register fixtures/rust-core --name rust-core
    if ($LASTEXITCODE -ne 0) { throw "registration failed" }
    & target/debug/graphine.exe --data-dir $data analyze rust-core --mode safe --features fancy --allow-partial
    if ($LASTEXITCODE -ne 0) { throw "safe analysis failed" }
    $status = & target/debug/graphine.exe --data-dir $data status rust-core | ConvertFrom-Json
    if ($status.language -ne "rust") { throw "wrong persisted language" }
    $doctor = & target/debug/graphine.exe --data-dir $data analyzer doctor --language rust | ConvertFrom-Json
    if ($doctor.status -ne "ok") { throw "Rust worker doctor failed" }
    $map = Invoke-GraphineMcp '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_project_map","arguments":{"project":"rust-core","token_budget":1200}}}'
    if ($map.result.structuredContent.result.language -ne "rust") { throw "Rust project map was not language-aware" }
    if ($map.result.structuredContent.result.summary.traits -lt 1) { throw "Rust project map omitted traits" }
    $context = Invoke-GraphineMcp '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"rust-core","stable_id":"function:crate:app/lib/app::crate::dynamic_process()","detail":"detailed","token_budget":2000}}}'
    $targets = @($context.result.structuredContent.result.callees | Where-Object relationship -eq "CALLS" | ForEach-Object stable_id)
    if ($targets -notcontains "method:crate:app/lib/app::crate::Store#save()") { throw "Rust dynamic dispatch did not target the trait declaration" }
    $evidence = Invoke-GraphineMcp '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_evidence","arguments":{"project":"rust-core","references":[{"file":"app/src/lib.rs","start_line":49,"end_line":55}],"token_budget":1200}}}'
    if ($evidence.result.structuredContent.result.snippets[0].file -ne "app/src/lib.rs") { throw "Rust evidence was not repository-relative" }
    $escape = Invoke-GraphineMcp '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_evidence","arguments":{"project":"rust-core","references":[{"file":"../Cargo.toml","start_line":1,"end_line":1}],"token_budget":800}}}'
    if ($escape.error.data.code -ne "invalid_path") { throw "Rust evidence containment did not reject traversal" }
    $endpoint = Invoke-GraphineMcp '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_endpoint_context","arguments":{"project":"rust-core","method":"GET","path":"/","token_budget":800}}}'
    if ($endpoint.error.data.code -ne "capability_not_supported") { throw "Rust endpoint context did not return capability_not_supported" }
    & target/debug/graphine.exe --data-dir $data analyze rust-core --mode trusted --all-features --allow-partial
    if ($LASTEXITCODE -ne 0) { throw "trusted analysis failed" }
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ((Test-Path -LiteralPath $data) -and $data.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $data -Recurse -Force
    }
}
