# MCP tool contract

Run `graphine serve` to use newline-delimited JSON-RPC 2.0 over STDIO. Graphine supports MCP revisions `2025-11-25`, `2025-06-18`, and `2025-03-26`. Initialize the server before calling tools.

The seven read-only tools have distinct agent intents:

- `get_project_map`: orient once with modules, major application areas, language-specific symbol counts, generation, freshness, and unresolved totals;
- `search_symbol`: deterministically rank stable IDs, qualified/simple names, camel-case tokens, routes, roles, module, and namespace proximity; `namespace_prefix` is preferred and `package_prefix` remains compatible;
- `get_symbol_context`: return generic `language`/`signature` plus grouped callers, callees, types, implementations, imports, field access, tests, framework facts, and evidence for one stable symbol; Java also exposes `java_signature` during the compatibility window;
- `get_endpoint_context`: return possible static controller-to-service-to-data flow, validation, dependencies, events, configuration, ambiguity, suppression totals, and evidence for one HTTP endpoint;
- `trace_flow`: return bounded grouped paths with semantic edge groups including `types` and `imports`, node filters, application-only filtering, shortest/all-path modes, and cycle handling;
- `get_evidence`: return bounded line-numbered snippets for generation-bound evidence IDs or validated repository-relative ranges; and
- `index_status`: return language, analyzer, generation freshness, source fingerprint, capabilities, effective feature/target configuration, diagnostic counts, modules/source sets, exclusions, and language-specific unsupported areas.

Use the progressive sequence `get_project_map` → `search_symbol` → a task-specific context tool → `get_evidence`. Use `trace_flow` only when the grouped task-specific packs are insufficient.

Successful calls use a shared envelope with `project`, `generation`, `stale`, `partial`, `completeness`, task-specific `result`, `unresolved`, `ambiguities`, deterministic `next_actions`, `pagination`, and `budget`. `completeness.status` is one of `complete`, `truncated`, `partial`, `ambiguous`, or `unresolved`; flags separately disclose fact, evidence, and uncertainty truncation. Static endpoint paths explicitly say they are possible structural flow rather than execution traces.

Evidence IDs have the form `evidence:<generation>:<hash>`. They contain no absolute path, resolve only against indexed locations in the registered repository, and fail with `generation_conflict` after a new generation becomes active. Direct ranges remain subject to canonical containment, symlink containment, text/UTF-8 checks, sensitive filename exclusions, configured excluded paths, and line/byte limits.

Opaque `v1` cursors are bound to project ID, active generation, operation, selector, filters, limits, and detail. Reusing one after reindexing or with changed arguments fails with `invalid_cursor`.

No MCP query starts Maven, a JVM worker, indexing, a build, a command, a runtime probe, or a network operation. Operational failures use JSON-RPC errors with safe codes and never disclose absolute repository paths.

`get_endpoint_context` is Spring-only. Rust projects receive `capability_not_supported` with suggestions to use project map, symbol search/context, and flow tracing. The tools do not prove runtime Spring activation, effective Spring Security behavior, transaction proxy interception, Rust trait-object runtime implementations, runtime-dependent conditions, or SpEL results.

Compatibility tests cover version negotiation, lifecycle, seven-tool schema exposure, valid calls for every tool, structured content, cursor binding, token budgets, cancellation honesty, legacy shutdown/exit, and EOF.
