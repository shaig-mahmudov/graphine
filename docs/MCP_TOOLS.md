# MCP tool contract

Run `graphine serve` to use newline-delimited JSON-RPC 2.0 over STDIO. Initialization returns tool capability metadata. The five tools are:

- `get_project_map`: node/edge counts by kind, modules, packages, generation, stale state, and unresolved count;
- `search_symbol`: bounded lexical search with kind filters, detail, limit, cursor, and token budget;
- `get_symbol_context`: identity, location, unresolved markers, evidence, and direct inbound/outbound edges;
- `trace_flow`: bounded breadth-first inbound or outbound traversal with edge-kind filters; and
- `index_status`: lifecycle, active generation, stale/partial flags, analyzer protocol and summary, diagnostic count, graph counts, and safe failure summary.

All successful calls return the stable envelope documented in `TOKEN_BUDGET.md` through MCP text content and `structuredContent`. Operational failures use JSON-RPC errors with a safe `data.code`, such as `project_not_found`, `index_not_ready`, `invalid_stable_id`, `invalid_path`, `invalid_token_budget`, `invalid_cursor`, `generation_conflict`, or `database_error`. Absolute repository paths are not included in MCP results or errors.

Real Java nodes expose source-set and external-symbol metadata. Call facts expose `dispatch` such as `exactly_resolved` or `virtual_declared_target`; method-reference edges explicitly state that runtime execution is not proven. Unresolved analyzer bindings are persisted as diagnostics and summarized by project/status responses.

MCP is read-only with respect to analysis. No tool starts the JVM worker or Maven. The server accepts cancellation notifications. Bounded synchronous requests are processed serially, so a cancellation notification has no concurrent task to interrupt and produces no response. Shutdown is clean at EOF or the `shutdown` request.
