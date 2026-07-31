# Deterministic medium benchmark

`benchmark-core generate-medium --output <empty-directory> --types 250` creates a dependency-free Maven project with stable file names and contents. Each generated type includes repeated call sites, field reads/writes, and a call to the next type, making the corpus useful for JVM startup/parse cost, logical-edge versus occurrence counts, SQLite ingestion, output volume, and query latency.

On Windows, `scripts/benchmark-medium.ps1` builds the analyzer and CLI, generates the corpus in a temporary directory, analyzes and ingests it, runs 20 common MCP searches, and writes a JSON report containing file/source-line, node/edge/occurrence, analyzer/ingestion, peak Java heap, peak Rust working set, database-size, and query p50/p95 measurements. The report records the environment and assumptions. It explicitly does not generalize one machine's numbers into universal performance claims.

`benchmark-core generate-rust-medium --output <path> --modules <50..2000>` creates the equivalent dependency-free Cargo corpus. `scripts/benchmark-rust-medium.ps1` analyzes 250 modules by default and writes `benchmarks/reports/rust-medium-performance.json`, including source size, graph size, generic worker timings/resources, SQLite size, and MCP query p50/p95.
