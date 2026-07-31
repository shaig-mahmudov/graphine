# Analyzer performance and memory

The worker uses JDT batch parsing but visits one compilation unit at a time and does not retain the complete AST set. Output is streamed as JSONL, then held as normalized graph records by the Rust validator until the complete stream is known-good. SQLite insertion is one generation transaction, not one transaction per fact.

The summary separates Maven classpath resolution, JDT parsing/extraction, the bounded Spring semantic pass, JSON serialization, total worker time, and peak observed Java heap use. End-to-end measurements additionally include JVM startup, Rust validation, and SQLite ingestion. Peak Rust memory is not yet instrumented and is reported as unavailable rather than estimated.

Rust summaries use generic `timings_ms` and `resources` maps for Cargo loading, trusted Cargo check, parsing/extraction, total time, source bytes, and diagnostics. `scripts/benchmark-rust-medium.ps1` generates a dependency-free 250-module Cargo corpus by default and records worker/ingestion/query measurements in `benchmarks/reports/rust-medium-performance.json`. These measurements describe the recorded environment, not arbitrary repositories.

Historical analyzer measurements are checked into `benchmarks/reports/phase2-performance.json`, `phase2.5-medium-performance.json`, and `phase3-spring-medium-performance.json`. The Phase 3 report uses a bounded two-hop route-neighborhood query; that value is a historical graph-query proxy and must not be presented as Phase 4 endpoint-context latency.

Direct Phase 4 MCP measurements are in `benchmarks/reports/generated/phase4-mcp-performance.json` and are produced by `scripts/benchmark-phase4-mcp.ps1`. The script indexes the `spring-web` fixture, keeps one initialized STDIO server warm, and measures actual `get_endpoint_context` and `get_evidence` calls. Its response-token field is Graphine's character-count estimate, not a provider-reported model token count. All checked-in results are engineering samples from the recorded environment and fixtures, not universal latency, memory, accuracy, or agent-efficiency claims.

The default worker heap limit is 1 GiB, stdout is capped at 64 MiB, stderr capture at 64 KiB, and timeout at 120 seconds. Tune these with configuration only after measuring the target repository.
