# Graphine

Graphine is intended to become a local-first code-intelligence engine and MCP server for Java and Spring Boot repositories. Its goal is to give AI coding agents compact structural facts, evidence locations, confidence, and unresolved ambiguity so they can answer repository questions with fewer file reads and fewer tokens.

## Current status: Phase 0

This repository currently contains only the evaluation foundation:

- a product contract, accuracy model, architecture boundary, non-goals, and reproducible benchmark methodology;
- versioned JSON Schemas for questions, manually verified ground truth, and run reports;
- 44 deterministic questions across pure Java, Spring Web, bean resolution, and Spring Data fixtures;
- small Java 17 fixture projects with intentional edge cases; and
- a Rust CLI that validates the corpus, rejects inconsistent data, computes statistics, and writes deterministic reports.

Graphine does **not** yet contain an MCP server, Java compiler integration, Spring analyzer, embeddings, graph database, runtime probe, or LLM adapter. The benchmark targets are engineering goals, not measured achievements.

## Use the corpus tools

Stable Rust 1.85 or newer is required.

```bash
cargo run -p benchmark-core -- validate
cargo run -p benchmark-core -- stats
cargo run -p benchmark-core -- report --output benchmarks/reports/corpus.json
```

Quality checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Spring fixture Maven builds require their declared dependencies to be available. Corpus validation itself performs no network calls and never builds fixtures.

## Add benchmark material

1. Add or extend a self-contained project under `fixtures/<fixture>` and document its intended behavior in that fixture's README.
2. Add a YAML document under `benchmarks/questions`. Choose a globally unique ID, declared fixture, category, capabilities, and honest token budget.
3. Add a matching YAML document under `benchmarks/ground-truth`. Evidence paths must be relative to the fixture root; line numbers must exist and must tightly support the expected claim.
4. Use `ambiguous`, `unresolved`, or `unsupported` when deterministic ground truth is unavailable. Add allowed ambiguities and forbidden claims.
5. Run validation, tests, and regenerate `benchmarks/reports/corpus.json`.

Schemas are designed so the corpus can grow beyond 100 questions without changing the format. See [Benchmark methodology](docs/BENCHMARK_METHODOLOGY.md) for evaluation rules and [Product contract](docs/PRODUCT_CONTRACT.md) for the intended stable product.

