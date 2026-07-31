param([string]$OutputDirectory = "dist")
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$output = [IO.Path]::GetFullPath((Join-Path $repo $OutputDirectory))
$stage = Join-Path $output "graphine"
if ([IO.Path]::GetDirectoryName($stage) -ne $output) { throw "Invalid package staging path" }

Push-Location $repo
try {
    & cargo build --release -p graphine-cli -p graphine-rust-analyzer
    if ($LASTEXITCODE -ne 0) { throw "Rust release build failed" }
    & mvn.cmd -q -f analyzer-jdt/pom.xml package
    if ($LASTEXITCODE -ne 0) { throw "Analyzer package failed" }
    if (Test-Path -LiteralPath $stage) {
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
    New-Item -ItemType Directory -Force (Join-Path $stage "lib") | Out-Null
    Copy-Item -Force "target/release/graphine.exe" (Join-Path $stage "graphine.exe")
    Copy-Item -Force "target/release/graphine-rust-analyzer.exe" (Join-Path $stage "graphine-rust-analyzer.exe")
    Copy-Item -Force "analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar" (Join-Path $stage "lib/graphine-analyzer.jar")
    $archive = Join-Path $output "graphine-windows-x86_64.zip"
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive -Force
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $output "SHA256SUMS") -Value "$hash  $([IO.Path]::GetFileName($archive))" -Encoding ascii
    Write-Output $archive
} finally { Pop-Location }
