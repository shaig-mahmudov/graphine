param([string]$OutputDirectory = "benchmarks/reports/generated")
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$output = Join-Path $repo $OutputDirectory
$temp = Join-Path ([IO.Path]::GetTempPath()) ("graphine-rust-accuracy-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $temp, $output | Out-Null

try {
    Push-Location $repo
    & cargo build -q -p graphine-rust-analyzer -p benchmark-core
    if ($LASTEXITCODE -ne 0) { throw "Rust analyzer build failed" }
    $request = @{
        protocol_version = 2; request_id = "accuracy-rust-core"; operation = "analyze_project"
        language = "rust"; project_root = [IO.Path]::GetFullPath((Join-Path $repo "fixtures/rust-core"))
        mode = "safe"; source_sets = @("main", "test")
        options = @{include_method_bodies=$true;include_field_access=$true;include_tests=$true;explicit_classpath=@()}
        maven_executable = "mvn.cmd"; cargo_executable = "cargo.exe"; rustc_executable = "rustc.exe"
        cargo = @{features=@("fancy");all_features=$false;no_default_features=$false;target=$null}
        timeout_ms = 120000
    } | ConvertTo-Json -Depth 6 -Compress
    $jsonl = Join-Path $temp "rust-core.jsonl"
    $jsonLines = @($request | & (Join-Path $repo "target/debug/graphine-rust-analyzer.exe"))
    if ($LASTEXITCODE -ne 0) { throw "Rust analyzer run failed" }
    [IO.File]::WriteAllLines($jsonl, [string[]]$jsonLines, [Text.UTF8Encoding]::new($false))
    & (Join-Path $repo "target/debug/benchmark-core.exe") evaluate-graph `
        --input $jsonl `
        --truth (Join-Path $repo "benchmarks/graph-ground-truth/rust-core.json") `
        --output (Join-Path $output "rust-core-accuracy.json") `
        --min-precision 0.97 --min-recall 0.95
    if ($LASTEXITCODE -ne 0) { throw "Rust accuracy threshold failed" }
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ((Test-Path -LiteralPath $temp) -and $temp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
