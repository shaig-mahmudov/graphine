#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
output="${1:-$repo/benchmarks/reports/generated}"
temp="$(mktemp -d)"
trap 'rm -rf "$temp"' EXIT
cd "$repo"
mvn -q -f analyzer-jdt/pom.xml package
cargo build -q -p benchmark-core
mkdir -p "$output"
for fixture in java-core java-unresolved; do
  project="$repo/fixtures/$fixture"
  python3 -c 'import json,sys; print(json.dumps({"protocol_version":1,"request_id":"accuracy-"+sys.argv[2],"operation":"analyze_project","project_root":sys.argv[1],"mode":"safe","source_sets":["main","test"],"options":{"include_method_bodies":True,"include_field_access":True,"include_tests":True,"explicit_classpath":[]},"maven_executable":"mvn","timeout_ms":120000}))' "$project" "$fixture" \
    | java -Xmx1024m -jar analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar > "$temp/$fixture.jsonl"
  target/debug/benchmark-core evaluate-java --input "$temp/$fixture.jsonl" \
    --truth "benchmarks/graph-ground-truth/$fixture.json" \
    --output "$output/$fixture-accuracy.json" --min-precision 0.97 --min-recall 0.90
done
