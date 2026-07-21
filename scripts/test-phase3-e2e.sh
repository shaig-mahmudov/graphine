#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data="$(mktemp -d)"
trap 'rm -rf "$data"' EXIT
cd "$repo"
mvn -q -f analyzer-jdt/pom.xml package
cargo build -q -p graphine-cli
target/debug/graphine --data-dir "$data" register "$repo/fixtures/spring-web" --name spring-web >/dev/null
analysis="$(target/debug/graphine --data-dir "$data" analyze spring-web --mode safe)"
python3 -c 'import json,sys; assert json.load(sys.stdin)["summary"]["capabilities"]["spring_static_semantics"]' <<<"$analysis"
response="$(printf '%s\n' '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"phase3-e2e","version":"1"}}}' '{"jsonrpc":"2.0","method":"notifications/initialized"}' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"spring-web","stable_id":"route:POST:/api/v1/events","detail":"evidence","token_budget":4000}}}' | target/debug/graphine --data-dir "$data" serve | tail -n 1)"
python3 -c 'import json,sys; d=json.load(sys.stdin); expected="method:dev.graphine.fixture.web.EventController#create(dev.graphine.fixture.web.CreateEventRequest)"; assert expected in [f.get("target") for f in d["result"]["structuredContent"]["facts"] if f.get("kind")=="HANDLED_BY"]; assert "RUNTIME_CONFIRMED" not in json.dumps(d)' <<<"$response"
echo "Phase 3 E2E passed: Spring fixture -> semantic pass -> JSONL -> Rust -> SQLite -> MCP"
