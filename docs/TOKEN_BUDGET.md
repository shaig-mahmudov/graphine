# Token budgets and detail levels

Phase 1 has no model tokenizer. `estimated_tokens` is explicitly a conservative deterministic estimate: compact JSON character count divided by four and rounded up. It is not an exact provider token count.

Budgets range from 128 to the configured maximum, 4,000 by default. Compaction preserves project/generation identity and unresolved/ambiguity state first, then ranked facts, then evidence. If a base summary cannot fit, it becomes an identity-only summary. Lower-priority facts and evidence are appended only if the whole response remains within budget. JSON is selected structurally and never byte-truncated.

Omission sets `budget.truncated=true` and `pagination.has_more=true`; resumable result sets provide a deterministic cursor. Detail levels are:

- `summary`: identity, essential names, kind, and confidence;
- `standard`: direct graph context and compact locations;
- `detailed`: selected metadata with standard relationships; and
- `evidence`: complete available evidence references within budget, never source contents.

