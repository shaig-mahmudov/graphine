# Analyzer performance and memory

The worker uses JDT batch parsing but visits one compilation unit at a time and does not retain the complete AST set. Output is streamed as JSONL, then held as normalized graph records by the Rust validator until the complete stream is known-good. SQLite insertion is one generation transaction, not one transaction per fact.

The summary separates Maven classpath resolution, JDT parsing/extraction, JSON serialization, total worker time, and peak observed Java heap use. End-to-end measurements additionally include JVM startup, Rust validation, and SQLite ingestion. Peak Rust memory is not yet instrumented and is reported as unavailable rather than estimated.

Current fixture measurements are checked into `benchmarks/reports/phase2-performance.json`. They are engineering samples from one Windows workstation, not universal service-level claims. Medium-project scaling has not yet been measured against a checked-in open-source corpus; this is a documented Phase 2 gap.

The default worker heap limit is 1 GiB, stdout is capped at 64 MiB, stderr capture at 64 KiB, and timeout at 120 seconds. Tune these with configuration only after measuring the target repository.
