# Benchmark runner adapters

`benchmark-core` owns corpus loading, validation, statistics, and deterministic reports. Future adapters belong here and will translate either a baseline agent session or a Graphine MCP-assisted session into `run-result.schema.json`. Phase 0 performs no model or network calls.

