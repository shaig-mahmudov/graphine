#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-$repo/benchmarks/reports/generated}"
temp="$(mktemp -d)"
trap 'rm -rf "$temp"' EXIT
mkdir -p "$output"
cd "$repo"
mvn -q -f analyzer-jdt/pom.xml package
cargo build -q -p benchmark-core
for fixture in spring-web spring-beans spring-data; do
  python3 -c 'import json,sys; print(json.dumps({"protocol_version":2,"request_id":"accuracy-"+sys.argv[2],"operation":"analyze_project","language":"java","project_root":sys.argv[1],"mode":"safe","source_sets":["main"],"options":{"include_method_bodies":True,"include_field_access":True,"include_tests":False,"explicit_classpath":[]},"maven_executable":"mvn","cargo_executable":"cargo","rustc_executable":"rustc","cargo":{},"timeout_ms":120000}))' "$repo/fixtures/$fixture" "$fixture" | java -Xmx1024m -jar analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar > "$temp/$fixture.jsonl"
  if grep -Eq 'RUNTIME_CONFIRMED|fixture-secret-must-never-be-stored|fixture-token-must-never-be-stored' "$temp/$fixture.jsonl"; then
    echo "$fixture emitted a forbidden runtime claim or configuration value" >&2
    exit 1
  fi
  target/debug/benchmark-core evaluate-java --input "$temp/$fixture.jsonl" --truth "benchmarks/graph-ground-truth/$fixture.json" --output "$output/$fixture-accuracy.json" --min-precision 0.97 --min-recall 0.95
done
