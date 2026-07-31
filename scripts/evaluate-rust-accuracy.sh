#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
output="${1:-$repo/benchmarks/reports/generated}"
temp="$(mktemp -d)"
trap 'rm -rf "$temp"' EXIT
cd "$repo"
cargo build -q -p graphine-rust-analyzer -p benchmark-core
mkdir -p "$output"
python3 -c 'import json,sys; print(json.dumps({"protocol_version":2,"request_id":"accuracy-rust-core","operation":"analyze_project","language":"rust","project_root":sys.argv[1],"mode":"safe","source_sets":["main","test"],"options":{"include_method_bodies":True,"include_field_access":True,"include_tests":True,"explicit_classpath":[]},"maven_executable":"mvn","cargo_executable":"cargo","rustc_executable":"rustc","cargo":{"features":["fancy"],"all_features":False,"no_default_features":False,"target":None},"timeout_ms":120000}))' "$repo/fixtures/rust-core" \
  | target/debug/graphine-rust-analyzer > "$temp/rust-core.jsonl"
target/debug/benchmark-core evaluate-graph \
  --input "$temp/rust-core.jsonl" \
  --truth benchmarks/graph-ground-truth/rust-core.json \
  --output "$output/rust-core-accuracy.json" \
  --min-precision 0.97 --min-recall 0.95
