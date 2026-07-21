# Non-goals

Phase 2 implements compiler-level Java analysis for standard Maven projects. It specifically excludes Spring semantic resolution, Gradle, embeddings, a graph database, and runtime probes.

The initial product is not intended to:

- support many programming languages;
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

Phase 2 also does not claim custom Maven plugin source roots, annotation-processor execution in safe mode, complete candidate dispatch sets, bytecode bridge methods, full Java module-system inference, or runtime lambda/virtual dispatch. Generic Java annotations remain annotation facts only. Spring beans, composed routes, security, JPA, transactions, and framework conditions begin in a later phase.
