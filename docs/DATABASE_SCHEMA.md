# SQLite schema and lifecycle

Graphine stores local state in `graphine.sqlite3`. Migration version 1 creates:

- `projects`: stable UUIDv5-derived project ID, canonical root, unique display name, registration time, source-fingerprint placeholder, analyzer placeholder, and schema version;
- `project_generations`: immutable numbered generations and `CREATED`, `BUILDING`, `READY`, `FAILED`, or `STALE` lifecycle records;
- `project_state`: active generation, stale flag, last successful activation, and safe failure summary;
- `nodes`: stable IDs, kinds, names, module/package, repository-relative location, confidence, provenance, JSON metadata, and unresolved markers;
- `edges`: generation-scoped source/target stable IDs, kind, confidence, provenance, and JSON metadata; and
- `evidence`: one-based inclusive repository-relative ranges.

Composite foreign keys prevent edges from crossing projects or generations. Unique constraints reject duplicate stable IDs and duplicate `(source, target, kind)` edges. JSON is checked by SQLite. Indexes cover project/generation, node stable ID, kind, qualified/simple names, edge source/target, and edge kind.

Migration version 2 adds analyzer protocol/version, source fingerprint, partial flag, and summary JSON to each generation, plus generation-scoped `analyzer_diagnostics`. Diagnostics contain a kind, optional repository-relative range and symbol text, a safe reason, and severity. The active generation's analyzer counts and summary are exposed by CLI and MCP status.

Migration version 3 adds normalized `edge_occurrences`. `edges` remains the one-row logical relationship used by traversal, while each occurrence stores a stable ordinal, repository-relative file, validated one-based inclusive line range, and bounded JSON metadata. Its composite foreign key ties evidence to exactly one project, generation, source, target, and kind. Compact queries expose `occurrence_count`; detailed/evidence queries page occurrence ranges without duplicating traversal edges.

Activation is atomic: graph rows, diagnostics, analyzer metadata, invariant checks, `READY`, and `project_state.active_generation` commit together. A failure is recorded separately after rollback. The last ready generation is retained. Graphine does not yet garbage-collect old generations.
