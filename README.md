# Graphine

Graphine is a local-first code-intelligence foundation and MCP server for Java and Spring Boot repositories. It gives AI coding agents compact structural facts, evidence locations, confidence, and unresolved ambiguity without uploading source code.

## Current status: Phase 1

The repository contains the Phase 0 evaluation foundation and the Phase 1 local graph service:

- product, accuracy, architecture, non-goal, and reproducible benchmark contracts;
- versioned schemas, 44 questions, matching ground truth, and four Java 17 fixtures;
- a benchmark validator, statistics CLI, and deterministic run reports;
- a `graphine` CLI for registration, migrations, inspection, synthetic indexing, health checks, and MCP serving;
- SQLite project, generation, node, edge, and evidence storage with atomic activation;
- deterministic lexical queries, bounded traversal, pagination, detail levels, and token-budget compaction; and
- a five-tool MCP server over STDIO with safe JSON-RPC errors and structured tracing to stderr.

Graphine still does **not** parse Java or infer Spring semantics. Phase 1 graph facts come only from explicit synthetic JSON. There is no Eclipse JDT integration, embedding model, graph database, runtime probe, remote service, or LLM adapter. Benchmark targets remain engineering goals, not achieved product claims.

## Build and run

Stable Rust 1.85 or newer is required.

```bash
cargo build --workspace
cargo run -p graphine-cli -- --data-dir .graphine db migrate
cargo run -p graphine-cli -- --data-dir .graphine register fixtures/spring-web --name spring-web
cargo run -p graphine-cli -- --data-dir .graphine synthetic-index spring-web fixtures/spring-web/graphine.synthetic.json
cargo run -p graphine-cli -- --data-dir .graphine status spring-web
cargo run -p graphine-cli -- --data-dir .graphine serve
```

MCP transport is newline-delimited JSON-RPC over STDIO. Logs go to stderr; protocol responses go to stdout. See [CLI usage](docs/CLI.md), [MCP tools](docs/MCP_TOOLS.md), and [synthetic graph format](docs/SYNTHETIC_GRAPH.md).

## Corpus tools and checks

```bash
cargo run -p benchmark-core -- validate
cargo run -p benchmark-core -- stats
cargo run -p benchmark-core -- report --output benchmarks/reports/corpus.json
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Spring fixture Maven builds require their declared dependencies. Corpus validation, graph queries, and MCP serving perform no network calls and never execute repository build scripts.

## Add benchmark material

1. Add or extend a self-contained project under `fixtures/<fixture>` and document it.
2. Add a unique YAML question under `benchmarks/questions`.
3. Add matching ground truth under `benchmarks/ground-truth` with tight fixture-relative evidence.
4. Represent unavailable certainty as `ambiguous`, `unresolved`, or `unsupported`.
5. Run validation, tests, and regenerate `benchmarks/reports/corpus.json`.

Schemas can grow beyond 100 questions without format changes. See the [benchmark methodology](docs/BENCHMARK_METHODOLOGY.md) and [product contract](docs/PRODUCT_CONTRACT.md).

