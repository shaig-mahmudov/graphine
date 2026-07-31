param([string]$OutputDirectory = "benchmarks/reports/generated")
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$jar = Join-Path $repo "analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar"
$output = Join-Path $repo $OutputDirectory
$temp = Join-Path ([IO.Path]::GetTempPath()) ("graphine-accuracy-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $temp, $output | Out-Null

try {
    Push-Location $repo
    & mvn.cmd -q -f analyzer-jdt/pom.xml package
    if ($LASTEXITCODE -ne 0) { throw "analyzer package failed" }
    & cargo build -q -p benchmark-core
    if ($LASTEXITCODE -ne 0) { throw "benchmark build failed" }
    foreach ($fixture in @("java-core", "java-unresolved")) {
        $project = [IO.Path]::GetFullPath((Join-Path $repo "fixtures/$fixture"))
        $request = @{
            protocol_version = 2; request_id = "accuracy-$fixture"; operation = "analyze_project"
            language = "java"; project_root = $project; mode = "safe"; source_sets = @("main", "test")
            options = @{ include_method_bodies = $true; include_field_access = $true; include_tests = $true; explicit_classpath = @() }
            maven_executable = "mvn.cmd"; cargo_executable = "cargo.exe"; rustc_executable = "rustc.exe"
            cargo = @{}; timeout_ms = 120000
        } | ConvertTo-Json -Depth 5 -Compress
        $jsonl = Join-Path $temp "$fixture.jsonl"
        $jsonLines = @($request | & java -Xmx1024m -jar $jar)
        if ($LASTEXITCODE -ne 0) { throw "$fixture analyzer run failed" }
        [IO.File]::WriteAllLines($jsonl, [string[]]$jsonLines, [Text.UTF8Encoding]::new($false))
        & (Join-Path $repo "target/debug/benchmark-core.exe") evaluate-java --input $jsonl `
            --truth (Join-Path $repo "benchmarks/graph-ground-truth/$fixture.json") `
            --output (Join-Path $output "$fixture-accuracy.json") --min-precision 0.97 --min-recall 0.90
        if ($LASTEXITCODE -ne 0) { throw "$fixture accuracy threshold failed" }
    }
} finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temp) -and $temp.StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
