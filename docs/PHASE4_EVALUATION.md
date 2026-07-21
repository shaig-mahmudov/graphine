# Phase 4 evaluation status

Phase 4 includes the complete paired capture schema and deterministic comparison command for baseline versus Graphine agent sessions. A publishable comparison requires externally supplied runs from the same model under identical controls. This repository does not bundle a model provider or credentials, so no token/tool/file-read savings are claimed until those paired captures are supplied.

The checked-in `benchmarks/reports/phase4-agent-evaluation.json` records this as `not_run` with null reductions rather than inventing measurements. Use `benchmark-core compare-agents` to replace it with an aggregate report after capturing both modes.

Local structural verification covers the small deterministic Java/Spring fixtures and generated medium Java/Spring corpora. No suitable multi-module Spring Boot or private Loopin checkout is registered in this repository, so Phase 4 does not claim agent-efficiency coverage for those two corpus classes yet.

The optional `resolve_target` convenience tool is intentionally not exposed: no paired benchmark evidence yet demonstrates a material tool-call reduction beyond `search_symbol` plus its deterministic disambiguation fields.

Local query latency and compact-response size are independently reproducible with `scripts/benchmark-phase4-mcp.ps1`. Those measurements test MCP/query engineering targets only; Graphine's character-based response estimate is not a provider input-token measurement and must not be presented as the agent token-reduction result.

The checked-in July 21, 2026 local Spring fixture run measured 100 calls in one warm server process: `get_endpoint_context` p95 was 8 ms with a 740-token median estimated response, and small `get_evidence` p95 was 2 ms with a 288-token median estimated response. The generated 100-controller Spring corpus separately measured 31 ms endpoint-context p95 including fresh STDIO process startup. These are environment-specific engineering measurements, not universal latency claims.

Known cases where Graphine can perform worse than grep/read are:

- repositories whose active index is stale or partial, because verification requires both a Graphine call and source reads;
- reflection, runtime bean conditions, dynamic routes, generated runtime proxies, or custom framework conventions outside the static analyzer;
- very small one-file questions where one direct read is cheaper than orientation plus a context pack;
- questions dominated by prose, comments, SQL resources, or configuration values not represented in the graph; and
- ambiguous high-branching graphs where a compact pack intentionally requires evidence pagination.
