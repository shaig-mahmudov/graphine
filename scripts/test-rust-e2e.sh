#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
data="$(mktemp -d)"
trap 'rm -rf "$data"' EXIT
cd "$repo"
cargo build -q -p graphine-cli -p graphine-rust-analyzer
target/debug/graphine --data-dir "$data" register fixtures/rust-core --name rust-core
target/debug/graphine --data-dir "$data" analyze rust-core --mode safe --features fancy --allow-partial
target/debug/graphine --data-dir "$data" status rust-core | grep -q '"language": "rust"'
target/debug/graphine --data-dir "$data" analyzer doctor --language rust | grep -q '"status": "ok"'
cat >"$data/mcp-input.jsonl" <<'EOF'
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"rust-e2e","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_project_map","arguments":{"project":"rust-core","token_budget":1200}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project":"rust-core","stable_id":"function:crate:app/lib/app::crate::dynamic_process()","detail":"detailed","token_budget":2000}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_evidence","arguments":{"project":"rust-core","references":[{"file":"app/src/lib.rs","start_line":49,"end_line":55}],"token_budget":1200}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_evidence","arguments":{"project":"rust-core","references":[{"file":"../Cargo.toml","start_line":1,"end_line":1}],"token_budget":800}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_endpoint_context","arguments":{"project":"rust-core","method":"GET","path":"/","token_budget":800}}}
EOF
target/debug/graphine --data-dir "$data" serve <"$data/mcp-input.jsonl" >"$data/mcp-output.jsonl"
python3 - "$data/mcp-output.jsonl" <<'PY'
import json, sys
responses = {value.get("id"): value for value in map(json.loads, open(sys.argv[1], encoding="utf-8"))}
project_map = responses[1]["result"]["structuredContent"]["result"]
assert project_map["language"] == "rust" and project_map["summary"]["traits"] >= 1
context = responses[2]["result"]["structuredContent"]["result"]
targets = {value["stable_id"] for value in context["callees"] if value["relationship"] == "CALLS"}
assert "method:crate:app/lib/app::crate::Store#save()" in targets
assert responses[3]["result"]["structuredContent"]["result"]["snippets"][0]["file"] == "app/src/lib.rs"
assert responses[4]["error"]["data"]["code"] == "invalid_path"
assert responses[5]["error"]["data"]["code"] == "capability_not_supported"
PY
target/debug/graphine --data-dir "$data" analyze rust-core --mode trusted --all-features --allow-partial
