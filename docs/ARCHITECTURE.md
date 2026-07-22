# Architecture through Phase 4

Phase 0 remains an independent evaluation pipeline:

```text
fixtures -> questions + ground truth -> benchmark-core -> deterministic report
```

The current implementation keeps source analysis out of the Rust process and layers Spring semantics and agent-oriented queries over the normalized graph:

```text
graphine-cli -> graphine-analyzer-client -> Java worker -> Maven resolver + Eclipse JDT
      |                    |                    |
      +-> graphine-mcp -> graphine-query (symbol + Spring endpoint packs) -> graphine-index -> SQLite
                               \             /
                                graphine-protocol
```

- `graphine-protocol` owns stable IDs, confidence, versioned analyzer events, request/response types, configuration, and safe errors.
- `graphine-analyzer-client` owns bounded process startup, cancellation, timeout, JSONL streaming, protocol validation, and conversion into one atomic ingestion batch.
- `analyzer-jdt` is a separate Maven reactor containing language-neutral protocol records, Maven resolution, JDT graph extraction, and the shaded worker CLI.
- `graphine-index` is the deliberately small SQLite data-access and generation-lifecycle layer.
- `graphine-query` owns application-oriented ranking, grouped symbol/endpoint packs, bounded traversal, evidence IDs and secure snippet retrieval, pagination, and token-budget compaction.
- `graphine-mcp` is a thin newline-delimited JSON-RPC/STDIO adapter. Its only source-reading capability is the bounded, repository-contained `get_evidence` contract.
- `graphine-cli` owns configuration, local administration, explicit analyzer invocation, synthetic loading, and server startup.
- `benchmark-core` remains independent of SQLite and MCP dependencies while validating paired baseline/Graphine session captures with deterministic claims and evidence.

SQLite work is synchronous and bounded. The current STDIO server processes requests serially, so it needs no async wrapper or global mutable state. Queries use indexed SQL and bounded neighbor reads rather than loading the graph into memory. Remote and concurrent transports are not implemented; any future concurrent transport must put synchronous SQLite calls behind an explicit blocking boundary.

Indexing uses immutable generations. The Rust client validates the complete stream in memory before opening an ingestion generation. Nodes, edges, diagnostics, analyzer metadata, `READY`, and activation are then written in one transaction. A rollback, worker crash, timeout, protocol error, or explicit `FAILED` record cannot replace the previous active graph. Partial activation is disabled by default.

The worker batch-parses compilation units with bindings and recovery enabled. A visitor extracts and emits one unit at a time; complete ASTs are not retained. A dedicated bounded pass derives conservative Spring facts from the already parsed units. External nodes are created only for directly referenced types and call targets. The detailed boundary is in `ANALYZER_ARCHITECTURE.md`.

Phase 0 loading remains deterministic: sorted YAML, B-tree aggregation, strict schemas, relative evidence paths, and fail-fast cross-document validation.
