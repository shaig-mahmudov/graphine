# Non-goals

Phase 1 implements the local SQLite graph, query, CLI, and MCP foundation, but not production source analysis. It specifically excludes a Java compiler analyzer, Spring analyzer, embeddings, graph database, and runtime probe.

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
