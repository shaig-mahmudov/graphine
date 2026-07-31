# Graphine

Graphine is a local-first code-intelligence foundation and MCP server for Java/Spring Boot and Rust/Cargo repositories. It gives AI coding agents compact structural facts, evidence locations, confidence, and unresolved ambiguity without uploading source code.

## Current status: Java and Rust beta hardening

Graphine is an advanced technical alpha moving toward a public beta. The repository contains the Phase 0 evaluation foundation through the Phase 4 agent query and evaluation layer:

- product, accuracy, architecture, non-goal, and reproducible benchmark contracts;
- versioned schemas, matching ground truth, Java 17 fixtures, and a Cargo workspace fixture;
- a benchmark validator, statistics CLI, deterministic graph reports, and paired baseline-versus-Graphine agent capture/evaluation;
- a `graphine` CLI for language-aware registration, explicit safe/trusted analysis, diagnostics, synthetic indexing, health checks, and MCP serving;
- SQLite project, generation, logical-edge, normalized occurrence-evidence, and diagnostic storage with atomic activation;
- deterministic application-oriented ranking, grouped symbol packs, endpoint flow, bounded path traversal, generation-bound evidence, stable pagination, and uncertainty-preserving structural compaction;
- a seven-tool MCP server over STDIO with project orientation, ranked symbol search, grouped symbol/endpoint context, secure evidence snippets, status, and tracing to stderr;
- independently runnable Java 17/JDT and Rust/rust-analyzer workers behind a sealed two-backend dispatcher;
- a dedicated bounded Spring pass for stereotypes, beans/injection, routes, repositories/entities, configuration keys, and static events; and
- bounded safe Maven and Cargo loading, content-based fingerprints, executable-relative analyzer packaging, derived doctor capabilities, automated graph accuracy evaluation, cross-platform CI/E2E, and deterministic Java, Spring, and Rust medium corpora.

Graphine supports exactly two registered languages: `java` and `rust`. A registration has one language; register mixed-monorepo subdirectories separately. Spring semantics remain Java-only, while Rust v1 covers Cargo crates, modules, declarations, calls, traits/implementations, types, imports, fields, macros, and tests. Rust web-framework semantics and cross-language calls are not inferred. See [Rust support](docs/RUST_SUPPORT.md), [compatibility](docs/COMPATIBILITY.md), and [MCP tools](docs/MCP_TOOLS.md).

Graphine does **not** start Spring or make runtime-confirmed, effective-security, transaction-proxy, or Rust dynamic-dispatch implementation claims. There is no Gradle support, non-Cargo Rust support, incremental indexing, embedding model, graph database, runtime probe, remote transport, operating-system sandbox, or built-in model provider. Checked-in accuracy reports cover only the named deterministic fixtures and do not establish arbitrary-project accuracy.

## Build and run

Rust 1.95+, Java 17+, and Maven 3.9+ are required to build every component. Cargo and rustc are required when analyzing Rust projects.

```bash
cargo build --workspace
mvn -q -f analyzer-jdt/pom.xml package
cargo run -p graphine-cli -- --data-dir .graphine db migrate
cargo run -p graphine-cli -- --data-dir .graphine register fixtures/java-core --name java-core
cargo run -p graphine-cli -- --data-dir .graphine analyze java-core --mode safe
cargo run -p graphine-cli -- --data-dir .graphine register fixtures/rust-core --name rust-core
cargo run -p graphine-cli -- --data-dir .graphine analyze rust-core --mode safe --features fancy
cargo run -p graphine-cli -- --data-dir .graphine status java-core
cargo run -p graphine-cli -- --data-dir .graphine doctor
cargo run -p graphine-cli -- --data-dir .graphine serve
```

Safe mode never executes the registered repository. For Rust it forces offline Cargo loading, disables build scripts and procedural macros, and falls back to a local manifest/path-dependency model when cached locked metadata is unavailable. Trusted mode is explicit and may run Maven resolution or rust-analyzer Cargo build data, build scripts, and procedural macros with the user's OS permissions. MCP reads never trigger analysis or a build tool.

MCP transport is newline-delimited JSON-RPC over STDIO. Logs go to stderr; protocol responses go to stdout. See [CLI usage](docs/CLI.md), [MCP tools](docs/MCP_TOOLS.md), and [synthetic graph format](docs/SYNTHETIC_GRAPH.md).

## Corpus tools and checks

```bash
cargo run -p benchmark-core -- validate
cargo run -p benchmark-core -- stats
cargo run -p benchmark-core -- report --output benchmarks/reports/corpus.json
cargo run -p benchmark-core -- compare-agents --baseline baseline.json --graphine graphine.json --output benchmarks/reports/phase4-agent-comparison.json
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/evaluate-java-accuracy.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/evaluate-spring-accuracy.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/evaluate-rust-accuracy.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-phase2-e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-phase3-e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-rust-e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/benchmark-medium.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/benchmark-spring-medium.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/benchmark-rust-medium.ps1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Spring fixture Maven builds require their declared dependencies. Corpus validation, safe analysis, graph queries, and MCP serving perform no network calls and never execute repository build scripts. Trusted analysis may use configured Maven or Cargo sources and may execute trusted Rust build-time code.

For packaged binary layout, analyzer discovery, and removal instructions, see [installation](docs/INSTALLATION.md). The repository contains packaging scripts, but a reproducible signed release workflow remains a beta exit criterion.

## Add benchmark material

1. Add or extend a self-contained project under `fixtures/<fixture>` and document it.
2. Add a unique YAML question under `benchmarks/questions`.
3. Add matching ground truth under `benchmarks/ground-truth` with tight fixture-relative evidence.
4. Represent unavailable certainty as `ambiguous`, `unresolved`, or `unsupported`.
5. Run validation, tests, and regenerate `benchmarks/reports/corpus.json`.

Schemas can grow beyond 100 questions without format changes. See the [benchmark methodology](docs/BENCHMARK_METHODOLOGY.md) and [product contract](docs/PRODUCT_CONTRACT.md).
