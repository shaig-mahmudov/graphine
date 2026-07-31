# Compatibility

Graphine registers exactly `java` or `rust`. `graphine register <path> --language auto` selects Java from a root `pom.xml` or Rust from a root `Cargo.toml`; if both exist, the language must be explicit. Mixed monorepos are represented by registering their Java and Rust subdirectories separately.

Schema migration v4 preserves existing registrations and generations as Java without reindexing. Existing Java `package_name` values are copied into the language-neutral `namespace_path`. Analyzer name, analyzer version, and language are immutable generation metadata.

Analyzer protocol v2 is intentionally incompatible with v1. The supervisor rejects a v1, malformed, or wrong-language worker before activation, preserving the prior ready generation. Java/Spring graph outputs and the seven MCP tool names remain compatible.

Configuration now prefers `java_analyzer_jar`; `analyzer_jar` remains a read-time compatibility alias. `package_prefix` remains accepted by symbol search while `namespace_prefix` is preferred. Symbol context exposes `language` and `signature`; Java results retain `java_signature` during the compatibility window. `evaluate-graph` is the language-neutral accuracy command and `evaluate-java` remains an alias.

Supported build layouts are Maven for Java and Cargo for Rust. Gradle, non-Cargo Rust, one registration containing both languages, cross-language edges, a generic analyzer plugin ABI, and a third language are unsupported.
