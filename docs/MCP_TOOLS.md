# MCP tool contract

Run `graphine serve` to use newline-delimited JSON-RPC 2.0 over STDIO. Graphine supports stable MCP revisions `2025-11-25`, `2025-06-18`, and `2025-03-26`, preferring `2025-11-25` when the requested revision is unknown. The client must send a valid `initialize` request followed by `notifications/initialized`; tool calls before that transition fail. The five tools are:

- `get_project_map`: node/edge counts by kind, modules, packages, generation, stale state, and unresolved count;
- `search_symbol`: bounded lexical search with kind filters, detail, limit, cursor, and token budget;
- `get_symbol_context`: identity, location, unresolved markers, evidence, and direct inbound/outbound edges;
- `trace_flow`: bounded breadth-first inbound or outbound traversal with edge-kind filters; and
- `index_status`: lifecycle, active generation, stale/partial flags, analyzer protocol and summary, diagnostic count, graph counts, and safe failure summary.

All successful calls return the stable envelope documented in `TOKEN_BUDGET.md` through MCP text content and `structuredContent`. Operational failures use JSON-RPC errors with a safe `data.code`, such as `project_not_found`, `index_not_ready`, `invalid_stable_id`, `invalid_path`, `invalid_token_budget`, `invalid_cursor`, `generation_conflict`, or `database_error`. Absolute repository paths are not included in MCP results or errors.

`search_symbol`, `get_symbol_context`, and `trace_flow` accept and return opaque `v1` cursors. A cursor is bound to the project ID, active generation, operation, symbol, filters/direction, limits, and detail level applicable to that operation. Reusing a cursor after reindexing or with a different query or symbol fails explicitly with `invalid_cursor`; clients must not edit cursors. The next request resumes at the first fact not returned by the previous page.

Real Java nodes expose source-set and external-symbol metadata. Call facts expose `dispatch` such as `exactly_resolved` or `virtual_declared_target`; method-reference edges explicitly state that runtime execution is not proven. Unresolved analyzer bindings are persisted as diagnostics and summarized by project/status responses.

MCP is read-only with respect to analysis. No tool starts the JVM worker or Maven. Graphine does not advertise cancellation: bounded synchronous requests are processed serially, so late/unknown cancellation notifications are safely ignored and cannot interrupt an active request. Arrays are rejected as invalid requests because the supported 2025-06-18-and-later protocol line removed JSON-RPC batching. Standard STDIO shutdown is EOF/process termination; the legacy `shutdown` request plus `exit` notification is also accepted as a compatibility extension.

Compatibility smoke checks are maintained as follows:

- Codex CLI: configure the release-layout `graphine serve` command, start a task, and verify initialize, `tools/list`, and `index_status` against a registered fixture;
- Claude Code (or another mainstream client): add the same local STDIO command and verify the five schemas plus one `get_symbol_context` call; and
- MCP Inspector: launch the STDIO server, negotiate `2025-11-25`, list tools, exercise a structured result, then close stdin.

The automated Rust compatibility suite covers version negotiation, initialized lifecycle, malformed JSON-RPC versions and IDs, unsupported batches, structured content, schema exposure, stale/cross-symbol cursors, cancellation honesty, legacy shutdown/exit, and EOF.
