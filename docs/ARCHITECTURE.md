# Architecture through Phase 1

Phase 0 remains an independent evaluation pipeline:

```text
fixtures -> questions + ground truth -> benchmark-core -> deterministic report
```

Phase 1 adds production boundaries without source analysis:

```text
graphine-cli -> graphine-mcp -> graphine-query -> graphine-index -> SQLite
                        \          /                 /
                         graphine-protocol ----------
```

- `graphine-protocol` owns stable IDs, confidence, request/response types, configuration, safe errors, and Phase 2 analyzer placeholders.
- `graphine-index` is the deliberately small SQLite data-access and generation-lifecycle layer.
- `graphine-query` owns lexical ranking, bounded traversal, pagination, detail shaping, and token-budget compaction.
- `graphine-mcp` is a thin newline-delimited JSON-RPC/STDIO adapter. It exposes no file-reading tool.
- `graphine-cli` owns configuration, local administration, synthetic loading, and server startup.
- `benchmark-core` remains independent of SQLite and MCP dependencies.

SQLite work is synchronous and bounded. The Phase 1 server processes STDIO requests serially, so it needs no async wrapper or global mutable state. Queries use indexed SQL and bounded neighbor reads rather than loading the graph into memory. Future concurrent transports must put synchronous SQLite calls behind an explicit blocking boundary.

Indexing uses immutable generations. Starting a build records `BUILDING` while retaining the active generation. Nodes and edges are written, validated, marked `READY`, and activated in one transaction. A rollback or explicit `FAILED` record cannot replace the previous active graph. `STALE` preserves queryable data while signaling that a future source fingerprint is outdated.

Phase 0 loading remains deterministic: sorted YAML, B-tree aggregation, strict schemas, relative evidence paths, and fail-fast cross-document validation.

