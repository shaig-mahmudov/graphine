# Non-goals

The implementation provides compiler-level Java analysis, bounded Spring static semantics, rust-analyzer-backed Rust analysis, and evidence-first MCP queries for Maven and Cargo projects. It supports exactly two registered languages and specifically excludes Gradle, non-Cargo Rust, embeddings, a graph database, remote services, and runtime probes.

The initial product is not intended to:

- support languages beyond Java and Rust or expose a general analyzer marketplace/plugin ABI;
- build a custom Java compiler;
- generate Cypher from natural language;
- provide 3D graph visualization;
- upload source code remotely or provide cloud indexing;
- include built-in LLM inference;
- fully prove runtime behavior through static analysis;
- declare security correctness;
- declare code dead without sufficient runtime or coverage evidence;
- add embedding search before lexical and structural search are evaluated;
- replace Java, Spring, database, or security testing; or
- claim accuracy, token savings, or latency targets before controlled measurement.

These boundaries keep early work centered on verifiable structural facts and a benchmark capable of falsifying future product claims.

The current implementation also does not claim custom Maven plugin source roots, annotation-processor execution, complete candidate dispatch sets, bytecode bridge methods, full Java module-system inference, or runtime lambda/virtual dispatch. It derives bounded static facts for common Spring beans, composed routes, JPA, transactions, events, and conditions, but does not prove runtime activation, effective security, or proxy behavior.

Rust non-goals are Axum/Actix/Rocket or other web-framework semantics, non-Cargo layouts, cross-language call inference, runtime trait-object implementation selection, nightly-only completeness, build/test execution in safe mode, incremental indexing, and standalone nodes for impl blocks, closures, or local variables.
