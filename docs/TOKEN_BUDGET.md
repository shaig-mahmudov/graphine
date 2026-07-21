# Token budgets and completeness

Graphine has no provider tokenizer. `budget.estimated_tokens` is a conservative deterministic estimate: compact JSON character count divided by four and rounded up. It is not reported as a measured model token count.

Phase 4 compacts structures rather than cutting serialized text. Every response retains project/generation freshness, completeness status, the requested identity, and visible uncertainty. Arrays are shortened deterministically and retain `<field>_omitted_count` plus `<field>_truncated`; evidence and uncertainty truncation also appear in `completeness`.

`get_endpoint_context` preserves information in this order:

1. endpoint and handler identity;
2. uncertainty and completeness;
3. main possible application flow;
4. data writes, then reads;
5. validation and events;
6. dependencies and configuration;
7. evidence references;
8. secondary calls and metadata.

The default suppression policy is `agent-default-v1`. It hides external/JDK/framework utility calls, common logging calls, trivial accessors, and generated methods where statically identifiable. Responses disclose `suppressed_fact_count` and the policy name. Set `suppression_policy: "none"` on endpoint queries when those details are required.

Budgets range from 128 to the configured maximum (4,000 by default). If the complete semantic pack cannot fit, Graphine can reduce it to the minimum task identity; it never removes all uncertainty indicators. Pagination resumes only generation-bound deterministic result sets.
