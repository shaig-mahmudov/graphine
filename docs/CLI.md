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
graphine analyze <project> --mode safe [--allow-partial]
graphine analyze <project> --mode trusted [--allow-partial]
graphine analyzer doctor
graphine analyzer version
graphine diagnostics <project>
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
  "mcp_transport": "stdio",
  "analyzer_jar": null,
  "java_executable": "java",
  "maven_executable": "mvn",
  "analyzer_timeout_ms": 120000,
  "analyzer_output_limit_bytes": 67108864,
  "allow_partial_activation": false
}
```

Defaults are local-only. An empty allowed-root list permits explicit local registration anywhere the user can access; installations can restrict it. CLI administration may show canonical paths because the user explicitly requested local inspection. MCP responses do not.

`analyze` is always explicit. Safe mode does not execute Maven. Trusted mode runs controlled Maven classpath resolution and therefore requires operator trust. `--allow-partial` is per invocation; the conservative default rejects partial activation. `analyzer doctor` checks that the independently packaged worker starts and speaks the supported protocol. On Windows, configure `maven_executable` as `mvn.cmd` when it is not on `PATH`.
