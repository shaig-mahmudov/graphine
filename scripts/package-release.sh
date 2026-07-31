#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
output="${1:-$repo/dist}"
stage="$output/graphine"
if [[ "$stage" != "$output/graphine" || "$stage" == "/graphine" ]]; then
  printf '%s\n' "invalid package staging path" >&2
  exit 1
fi
cd "$repo"
cargo build --release -p graphine-cli -p graphine-rust-analyzer
mvn -q -f analyzer-jdt/pom.xml package
rm -rf -- "$stage"
mkdir -p "$stage/lib"
cp target/release/graphine "$stage/graphine"
cp target/release/graphine-rust-analyzer "$stage/graphine-rust-analyzer"
cp analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar "$stage/lib/graphine-analyzer.jar"
archive="$output/graphine-linux-x86_64.tar.gz"
tar -C "$stage" -czf "$archive" .
(cd "$output" && sha256sum "$(basename "$archive")" > SHA256SUMS)
printf '%s\n' "$archive"
