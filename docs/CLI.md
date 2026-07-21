# CLI usage

The binary is `graphine`. Global `--config` reads local JSON; `--data-dir` overrides its data directory. `GRAPHINE_CONFIG` and `GRAPHINE_DATA_DIR` are equivalent environment options.

```text
graphine register <path> [--name <display-name>]
graphine unregister <project-id-or-name>
graphine projects
graphine status <project-id-or-name>
graphine doctor
graphine config
graphine db migrate
graphine synthetic-index <project> <fixture.json>
graphine serve
```

Example configuration:

```json
{
  "data_dir": ".graphine",
  "log_level": "info",
  "default_token_budget": 800,
  "maximum_token_budget": 4000,
  "maximum_query_depth": 5,
  "maximum_result_count": 100,
  "allowed_repository_roots": [],
  "sqlite_timeout_ms": 5000,
  "mcp_transport": "stdio"
}
```

Defaults are local-only. An empty allowed-root list permits explicit local registration anywhere the user can access; installations can restrict it. CLI administration may show canonical paths because the user explicitly requested local inspection. MCP responses do not.

