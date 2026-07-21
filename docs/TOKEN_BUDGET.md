# Token budgets and detail levels

Phase 1 has no model tokenizer. `estimated_tokens` is explicitly a conservative deterministic estimate: compact JSON character count divided by four and rounded up. It is not an exact provider token count.

Budgets range from 128 to the configured maximum, 4,000 by default. Compaction uses this exact priority:

1. project and active-generation identity;
2. stale and partial index state;
3. requested answer identity;
4. aggregate uncertainty and then individual ambiguity/unresolved details;
5. requested facts in deterministic confidence/query order;
6. evidence references;
7. secondary relationships; and
8. optional summary and analyzer metadata.

`uncertainty` always retains unresolved and ambiguity counts. `truncation` distinguishes omitted facts, evidence, unresolved details, ambiguity details, and summary metadata. Any material omission sets `budget.truncated=true` and `complete=false`; uncertainty truncation can therefore never turn a partial answer into an apparently complete one. If a base summary cannot fit, it becomes an identity-only summary. JSON is selected structurally and never byte-truncated.

Fact pagination sets `pagination.has_more=true`; other truncation is reported by its explicit truncation flag. Resumable result sets provide a deterministic cursor. Detail levels are:

- `summary`: identity, essential names, kind, and confidence;
- `standard`: direct graph context and compact locations;
- `detailed`: selected metadata with standard relationships; and
- `evidence`: complete available evidence references within budget, never source contents.
