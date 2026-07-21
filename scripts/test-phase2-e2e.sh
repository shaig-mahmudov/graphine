#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
data="$(mktemp -d)"
trap 'rm -rf "$data"' EXIT
cd "$repo"
mvn -q -f analyzer-jdt/pom.xml package
cargo build -q -p graphine-cli
binary="$repo/target/debug/graphine"
"$binary" --data-dir "$data" register "$repo/fixtures/java-core" --name java-core >/dev/null
"$binary" --data-dir "$data" analyze java-core --mode safe >/dev/null
python3 - "$binary" "$data" <<'PY'
import json, subprocess, sys
binary, data = sys.argv[1:]

def call(request):
    messages = [
        {"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"linux-e2e","version":"1"}}},
        {"jsonrpc":"2.0","method":"notifications/initialized"},
        request,
    ]
    payload = "\n".join(json.dumps(item) for item in messages) + "\n"
    completed = subprocess.run([binary,"--data-dir",data,"serve"],input=payload,text=True,capture_output=True,check=True)
    return json.loads(completed.stdout.strip().splitlines()[-1])["result"]["structuredContent"]

context = call({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"java-core","stable_id":"method:dev.graphine.fixture.core.CoreScenario#run()","detail":"detailed","token_budget":4000}}})
targets = {fact.get("target") for fact in context["facts"] if fact.get("kind") == "CALLS"}
expected = "method:dev.graphine.fixture.core.Operations#combine(java.lang.String,java.lang.String)"
wrong = "method:dev.graphine.fixture.core.Operations#combine(int,int)"
assert expected in targets and wrong not in targets

occurrences = call({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"java-core","stable_id":"method:dev.graphine.fixture.core.Operations#repeatTrim(java.lang.String)","detail":"detailed","token_budget":4000}}})
trim = [fact for fact in occurrences["facts"] if fact.get("kind") == "CALLS" and fact.get("target") == "method:java.lang.String#trim()"]
assert len(trim) == 1 and trim[0]["occurrence_count"] == 3 and len(trim[0]["evidence_refs"]) == 3
PY
printf '%s\n' 'Phase 2 E2E passed: fixture -> JDT -> JSONL -> Rust -> SQLite -> MCP'
