# Phase 0 architecture

Graphine Phase 0 separates evaluation data from future implementations:

```text
fixtures -> questions + ground truth -> benchmark-core -> deterministic report
                                      future adapters -> baseline / MCP runs
```

`fixtures/` contains small inspectable repositories. `benchmarks/schema/` is the versioned data contract. Questions describe the task and budget; ground truth describes expected structural facts, minimum confidence, evidence, allowed ambiguity, and forbidden claims. `benchmark-core` validates JSON Schema and cross-document invariants, then calculates corpus statistics or emits a report.

Future baseline and MCP-agent adapters will live under `benchmarks/runner` and produce the same run-result shape. They must not alter corpus loading or scoring semantics. Future production crates for compiler, Spring, storage, or MCP behavior are deliberately absent until Phase 0 gives them a stable evaluation target.

The CLI recursively loads sorted YAML files and YAML documents. B-tree collections and pretty JSON serialization make output ordering deterministic. Validation checks IDs, one-to-one question/answer coverage, confidence vocabulary, relative evidence paths, file existence, and one-based inclusive ranges. Schema or semantic errors stop the run; nothing is silently skipped.

