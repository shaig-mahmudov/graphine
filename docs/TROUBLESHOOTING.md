# Analyzer troubleshooting

Run `graphine analyzer doctor --language java|rust` first. For Java, build `analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar` with `mvn -f analyzer-jdt/pom.xml package`, or configure `java_analyzer_jar` and `java_executable`. For Rust, build `graphine-rust-analyzer` or configure `rust_analyzer_worker`, `cargo_executable`, and `rustc_executable`.

If safe analysis reports unresolved dependencies, inspect `graphine diagnostics <project>`. Build dependencies separately or use explicit trusted mode for a reviewed repository. Trusted failures commonly mean Maven is absent, `maven_executable` is wrong (`mvn.cmd` may be needed on Windows), repository access failed, or the timeout expired.

`analyzer_timeout` means the worker was killed and the previous active generation remains queryable. `analyzer_protocol_error` means stdout was malformed, oversized, incomplete, inconsistent, or from an unsupported worker. Worker stderr is bounded and logged only as a short safe diagnostic; source contents are not logged.

A `partial` result has recoverable parse failures. It does not activate by default. Re-run without broken sources or explicitly opt in after reviewing diagnostics. A missing `pom.xml`, missing registered root, or catastrophic classpath failure is fatal.

For unexpected facts, query `get_symbol_context` and inspect confidence, provenance, dispatch metadata, external markers, source set, and evidence. Virtual declared targets and lambda/method-reference edges are static facts, not proof of runtime execution.
