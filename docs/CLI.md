# CLI usage

The binary is `graphine`. Global `--config` reads local JSON; `--data-dir` overrides its data directory. `GRAPHINE_CONFIG` and `GRAPHINE_DATA_DIR` are equivalent environment options.

```text
graphine register <path> [--name <display-name>] [--language auto|java|rust]
graphine unregister <project-id-or-name>
graphine projects
graphine status <project-id-or-name>
graphine doctor
graphine config
graphine db migrate
graphine synthetic-index <project> <fixture.json>
graphine analyze <project> --mode safe [--allow-partial] [--features <list>] [--all-features] [--no-default-features] [--target <triple>]
graphine analyze <project> --mode trusted [--allow-partial] [Rust Cargo options]
graphine analyzer doctor --language java|rust
graphine analyzer version --language java|rust
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
  "maximum_evidence_lines": 200,
  "maximum_evidence_bytes": 65536,
  "evidence_excluded_paths": [],
  "allowed_repository_roots": [],
  "sqlite_timeout_ms": 5000,
  "mcp_transport": "stdio",
  "java_analyzer_jar": null,
  "rust_analyzer_worker": null,
  "java_executable": "java",
  "maven_executable": "mvn",
  "cargo_executable": "cargo",
  "rustc_executable": "rustc",
  "analyzer_timeout_ms": 120000,
  "analyzer_output_limit_bytes": 67108864,
  "allow_partial_activation": false
}
```

Defaults are local-only. An empty allowed-root list permits explicit local registration anywhere the user can access; installations can restrict it. CLI administration may show canonical paths because the user explicitly requested local inspection. MCP responses do not.

Registration defaults to `auto`: a root `pom.xml` selects Java and a root `Cargo.toml` selects Rust. Both manifests require an explicit language; neither is an error. One registration has one language. `analyze` selects the persisted backend. Rust Cargo options are rejected for Java and conflicting feature selectors are rejected. `analyzer_jar` remains a compatibility alias for `java_analyzer_jar`.

`doctor` reports Java/JDT/Maven and Rust/rust-analyzer/Cargo/rustc stacks independently. Missing tooling for an unused language does not degrade otherwise healthy registered projects. `analyzer doctor --language ...` checks the selected independently packaged worker without analyzing a repository.
