# Analyzer JSONL protocol

Protocol version 1 is one request on stdin followed by independently valid JSON objects on stdout. Human diagnostics go to bounded stderr; stdout is protocol-only. The request has `protocol_version`, `request_id`, `operation: analyze_project`, canonical `project_root`, `mode`, source sets, analyzer options, Maven executable, and timeout.

Successful event order is:

```text
analysis_started
project_metadata
module_discovered *
node *
edge *
diagnostic *
analysis_summary
analysis_completed
```

`analysis_started` echoes the request ID and declares the worker version. Project metadata includes the source fingerprint, release, classpath timing, module list, and additive analyzer capabilities. Nodes use the public stable-ID grammar and repository-relative source ranges. Phase 3 adds `config:` IDs while reusing `type:`, `method:`, `field:`, `bean:`, and `route:` identities. Edges carry source/target IDs, kind, confidence, provenance, object-valued aggregate metadata, and a backward-compatible `occurrences` array. Diagnostics are structured unresolved or ambiguity facts rather than invented deterministic edges.

`analysis_summary` reports discovered/parsed/failed files, resolved/unresolved binding counts, node/edge counts, classpath/parsing/Spring-semantic/serialization/total time, peak Java heap observation, capability flags, and complete/partial status. Capability fields are additive and default empty when reading older Phase 2 summaries. `analysis_completed` is required. `analysis_failed` terminates a request without graph activation.

Rust rejects unknown/malformed lines, unsupported versions, wrong request IDs, events before start or after completion, duplicate IDs/edges, unknown edge endpoints, absolute/traversing paths, mismatched summary counts, output overflow, incomplete streams, and nonzero worker exits. Protocol types contain no JDT objects or serialization keys.

Version 1 is defined in `graphine-protocol` and mirrored by `analyzer-protocol`. Phase 3 uses additive metadata and graph kinds, so it remains wire-compatible; any incompatible change still requires a new protocol version and explicit client support.
