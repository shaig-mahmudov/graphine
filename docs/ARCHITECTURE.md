# Two-language architecture

Phase 0 remains an independent evaluation pipeline:

```text
fixtures -> questions + ground truth -> benchmark-core -> deterministic report
```

The host uses an explicit two-backend dispatcher. Language-specific analysis remains behind independent worker processes and both workers produce one normalized graph:

```text
graphine-cli -> graphine-analyzer-client -> Java worker -> Maven + Eclipse JDT + Spring pass
      |                    |
      |                    +-------------> Rust worker -> Cargo + rust-analyzer
      |
      +-> graphine-mcp -> graphine-query -> graphine-index -> SQLite
                               \              /
                                graphine-protocol
```

- `graphine-protocol` owns stable IDs, confidence, versioned analyzer events, request/response types, configuration, and safe errors.
- `graphine-analyzer-client` is the sealed `java`/`rust` dispatcher and owns bounded process startup, cancellation, timeout, JSONL streaming, protocol validation, and conversion into one atomic ingestion batch.
- `analyzer-jdt` is a separate Maven reactor containing language-neutral protocol records, Maven resolution, JDT graph extraction, and the shaded worker CLI.
- `graphine-rust-analyzer` is a separately runnable Rust worker. It pins its `ra_ap_*` dependencies exactly and keeps their unstable API behind analyzer protocol v2.
- `graphine-index` is the deliberately small SQLite data-access and generation-lifecycle layer.
- `graphine-query` owns application-oriented ranking, grouped symbol/endpoint packs, bounded traversal, evidence IDs and secure snippet retrieval, pagination, and token-budget compaction.
- `graphine-mcp` is a thin newline-delimited JSON-RPC/STDIO adapter. Its only source-reading capability is the bounded, repository-contained `get_evidence` contract.
- `graphine-cli` owns configuration, local administration, explicit analyzer invocation, synthetic loading, and server startup.
- `benchmark-core` remains independent of SQLite and MCP dependencies while validating paired baseline/Graphine session captures with deterministic claims and evidence.

SQLite work is synchronous and bounded. The current STDIO server processes requests serially, so it needs no async wrapper or global mutable state. Queries use indexed SQL and bounded neighbor reads rather than loading the graph into memory. Remote and concurrent transports are not implemented; any future concurrent transport must put synchronous SQLite calls behind an explicit blocking boundary.

Indexing uses immutable, language-tagged generations. The client validates the complete stream in memory before opening an ingestion generation. Nodes, edges, diagnostics, analyzer name/version/language, `READY`, and activation are then written in one transaction. A rollback, worker crash, timeout, protocol error, wrong-language worker, or explicit `FAILED` record cannot replace the previous active graph. Partial activation is disabled by default.

The Java worker batch-parses compilation units with bindings and recovery enabled. A dedicated bounded pass derives conservative Spring facts from the already parsed units. The Rust worker loads Cargo targets through rust-analyzer, then normalizes semantic resolution and syntax fallback into the same nodes, edges, evidence, and diagnostics. Dynamic trait-object calls target the trait declaration; closures are folded into their nearest named function. The detailed boundaries are in `ANALYZER_ARCHITECTURE.md` and `RUST_SUPPORT.md`.

Phase 0 loading remains deterministic: sorted YAML, B-tree aggregation, strict schemas, relative evidence paths, and fail-fast cross-document validation.
