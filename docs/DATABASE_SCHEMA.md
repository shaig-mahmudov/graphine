# SQLite schema and lifecycle

Graphine stores local state in `graphine.sqlite3`. Migration version 1 creates:

- `projects`: stable UUIDv5-derived project ID, canonical root, unique display name, registration time, source-fingerprint placeholder, analyzer placeholder, and schema version;
- `project_generations`: immutable numbered generations and `CREATED`, `BUILDING`, `READY`, `FAILED`, or `STALE` lifecycle records;
- `project_state`: active generation, stale flag, last successful activation, and safe failure summary;
- `nodes`: stable IDs, kinds, names, module/package, repository-relative location, confidence, provenance, JSON metadata, and unresolved markers;
- `edges`: generation-scoped source/target stable IDs, kind, confidence, provenance, and JSON metadata; and
- `evidence`: one-based inclusive repository-relative ranges.

Composite foreign keys prevent edges from crossing projects or generations. Unique constraints reject duplicate stable IDs and duplicate `(source, target, kind)` edges. JSON is checked by SQLite. Indexes cover project/generation, node stable ID, kind, qualified/simple names, edge source/target, and edge kind.

Activation is atomic: graph rows, invariant checks, `READY`, and `project_state.active_generation` commit together. A failure is recorded separately after rollback. The last ready generation is retained. Phase 1 does not garbage-collect old generations.

