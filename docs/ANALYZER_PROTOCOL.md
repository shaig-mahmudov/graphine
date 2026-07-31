# Analyzer JSONL protocol

Protocol version 2 is one request on stdin followed by independently valid JSON objects on stdout. Human diagnostics go to bounded stderr; stdout is protocol-only. Every request carries `language`, common analysis options, and language-specific Java/Maven or Rust/Cargo settings. Rust settings include selected features, all/default-feature switches, target triple, Cargo executable, and rustc executable.

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

`analysis_started` echoes the request ID and declares analyzer name, worker version, and language. Project metadata includes language, source fingerprint, modules/targets, and an object-valued effective configuration. Nodes use the public stable-ID grammar and repository-relative source ranges. Edges carry source/target IDs, kind, confidence, provenance, object-valued aggregate metadata, and an `occurrences` array. Diagnostics are structured unresolved or ambiguity facts rather than invented deterministic edges.

`analysis_summary` reports language, discovered/parsed/failed files, resolved/unresolved binding counts, node/edge counts, generic timing/resource maps, effective configuration, capability flags, and complete/partial status. `analysis_completed` is required. `analysis_failed` terminates a request without graph activation.

The supervisor rejects unknown/malformed lines, protocol v1, wrong-language workers, wrong request IDs, events before start or after completion, duplicate IDs/edges, unknown edge endpoints, absolute/traversing paths, mismatched summary counts, output overflow, incomplete streams, and nonzero worker exits. Validation completes before a generation transaction starts, so failure cannot replace the active graph. Protocol types contain no JDT or rust-analyzer objects.

Version 2 is defined in `graphine-protocol` and mirrored by `analyzer-protocol`. Java and Rust use the same normalized event stream; any future incompatible change requires a new protocol version and explicit client support.

The non-analysis `--metadata` command returns one JSON object containing `analyzer_name`, `language`, `protocol_version`, `analyzer_version`, and packaged `capabilities`. It does not accept a project, resolve dependencies, or start Maven/Cargo. Doctor compares both language and protocol before advertising capabilities.
