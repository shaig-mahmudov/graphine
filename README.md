# Graphine

Graphine is a local-first code-intelligence foundation and MCP server for Java and Spring Boot repositories. It gives AI coding agents compact structural facts, evidence locations, confidence, and unresolved ambiguity without uploading source code.

## Current status: Phase 3 Spring static semantics

The repository contains the Phase 0 evaluation foundation, Phase 1 local graph service, Phase 2 Java analyzer, Phase 2.5 hardening, and Phase 3 Spring semantic pass:

- product, accuracy, architecture, non-goal, and reproducible benchmark contracts;
- versioned schemas, 44 questions, matching ground truth, and four Java 17 fixtures;
- a benchmark validator, statistics CLI, and deterministic run reports;
- a `graphine` CLI for registration, explicit safe/trusted Java analysis, diagnostics, synthetic indexing, health checks, and MCP serving;
- SQLite project, generation, logical-edge, normalized occurrence-evidence, and diagnostic storage with atomic activation;
- deterministic lexical queries, bounded traversal, generation-bound opaque pagination, detail levels, and uncertainty-preserving token-budget compaction;
- a five-tool MCP server over STDIO with validated initialization lifecycle, negotiated stable protocol revisions, safe JSON-RPC errors, structured content, and tracing to stderr;
- an independently runnable Java 17 worker using Eclipse JDT batch parsing and compiler bindings;
- a dedicated bounded Spring pass for stereotypes, beans/injection, routes, repositories/entities, configuration keys, and static events; and
- bounded safe Maven parent/property/reactor/cache resolution, content-based fingerprints, executable-relative analyzer packaging, derived doctor capabilities, automated graph accuracy evaluation, cross-platform CI/E2E, and a deterministic medium corpus.

Graphine does **not** start Spring or make runtime-confirmed, effective-security, or transaction-proxy claims. There is no Gradle support, embedding model, graph database, runtime probe, remote service, or LLM adapter. See the checked-in Phase 2/2.5/3 accuracy and performance reports and [Spring static-semantics boundary](docs/SPRING_STATIC_SEMANTICS.md).

## Build and run

Stable Rust 1.86+, Java 17+, and Maven 3.9+ are required to build every component.

```bash
cargo build --workspace
mvn -q -f analyzer-jdt/pom.xml package
cargo run -p graphine-cli -- --data-dir .graphine db migrate
cargo run -p graphine-cli -- --data-dir .graphine register fixtures/java-core --name java-core
cargo run -p graphine-cli -- --data-dir .graphine analyze java-core --mode safe
cargo run -p graphine-cli -- --data-dir .graphine status java-core
cargo run -p graphine-cli -- --data-dir .graphine serve
```

Safe mode never executes the registered repository. Trusted mode is an explicit `analyze --mode trusted` operation and runs only Graphine's fixed Maven dependency-classpath command. MCP reads never trigger analysis or Maven.

MCP transport is newline-delimited JSON-RPC over STDIO. Logs go to stderr; protocol responses go to stdout. See [CLI usage](docs/CLI.md), [MCP tools](docs/MCP_TOOLS.md), and [synthetic graph format](docs/SYNTHETIC_GRAPH.md).

## Corpus tools and checks

```bash
cargo run -p benchmark-core -- validate
cargo run -p benchmark-core -- stats
cargo run -p benchmark-core -- report --output benchmarks/reports/corpus.json
powershell -File scripts/evaluate-java-accuracy.ps1
powershell -File scripts/evaluate-spring-accuracy.ps1
powershell -File scripts/test-phase2-e2e.ps1
powershell -File scripts/test-phase3-e2e.ps1
powershell -File scripts/benchmark-medium.ps1
powershell -File scripts/benchmark-spring-medium.ps1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Spring fixture Maven builds require their declared dependencies. Corpus validation, safe analysis, graph queries, and MCP serving perform no network calls and never execute repository build scripts. Trusted analysis may use Maven's configured repositories.

## Add benchmark material

1. Add or extend a self-contained project under `fixtures/<fixture>` and document it.
2. Add a unique YAML question under `benchmarks/questions`.
3. Add matching ground truth under `benchmarks/ground-truth` with tight fixture-relative evidence.
4. Represent unavailable certainty as `ambiguous`, `unresolved`, or `unsupported`.
5. Run validation, tests, and regenerate `benchmarks/reports/corpus.json`.

Schemas can grow beyond 100 questions without format changes. See the [benchmark methodology](docs/BENCHMARK_METHODOLOGY.md) and [product contract](docs/PRODUCT_CONTRACT.md).
