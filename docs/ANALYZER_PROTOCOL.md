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

`analysis_started` echoes the request ID and declares the worker version. Project metadata includes the source fingerprint, release, classpath timing, and module list. Nodes use the public stable-ID grammar and repository-relative source ranges. Edges carry source/target IDs, kind, confidence, provenance, and an object-valued metadata field. Diagnostics are structured unresolved or parse facts rather than synthetic deterministic edges.

`analysis_summary` reports discovered/parsed/failed files, resolved/unresolved binding counts, node/edge counts, classpath/parsing/serialization/total time, peak Java heap observation, and complete/partial status. `analysis_completed` is required. `analysis_failed` terminates a request without graph activation.

Rust rejects unknown/malformed lines, unsupported versions, wrong request IDs, events before start or after completion, duplicate IDs/edges, unknown edge endpoints, absolute/traversing paths, mismatched summary counts, output overflow, incomplete streams, and nonzero worker exits. Protocol types contain no JDT objects or serialization keys.

Version 1 is defined in `graphine-protocol` and mirrored by `analyzer-protocol`. Any incompatible change requires a new protocol version and explicit client support.
