# Repository boundary security

## Repository and MCP boundary

Registration canonicalizes the requested path and rejects files. When `allowed_repository_roots` is configured, the canonical project root must be below one of those canonical roots.

Synthetic evidence paths must be non-empty repository-relative paths. Graphine rejects Unix absolute paths, Windows drive or UNC paths, NUL or replacement characters, empty segments, `.` and `..`, missing files, and non-normal path components. The final target is canonicalized and must remain under the canonical project root, preventing symlink escape. Stored and returned evidence paths use forward-slash repository-relative form.

## Analyzer boundary

Safe Java analysis discovers only standard Maven source roots, existing `target/classes` directories, explicit classpath entries, and literal dependency artifacts already present in the local Maven cache. It does not start Maven or execute repository-controlled plugins or scripts. Safe Rust analysis is offline, disables Cargo build data, build scripts, and procedural macros, never runs project targets, and falls back to local manifest/path-dependency discovery when locked cached metadata is insufficient.

Trusted analysis must be explicitly requested from the CLI. Java starts Maven in the canonical registered project root with a fixed argument vector for `dependency:build-classpath`; Rust may run rust-analyzer Cargo build data, build scripts, and procedural macros. Graphine does not use shell interpolation. Processes have bounded output and a timeout, but trusted build configuration is still code execution with the user's OS permissions. It is never reached from an MCP read.

The supervisor applies the same request/output bounds, cancellation, timeout, JSONL validation, and atomic ingestion rules to both workers. It validates protocol version, analyzer language, request ID, stable ID, path, duplicate, edge endpoint, and summary count; failure cannot replace the active generation. Source text and build-tool output are not logged. Spring analysis never starts an application context. Operating-system sandboxing of either worker is outside Graphine's current scope.

MCP exposes graph facts plus bounded source bodies only through `get_evidence`. That tool accepts generation-bound indexed IDs or explicit repository-relative ranges, canonicalizes root and target, rejects symlink escape, binary/non-UTF-8 files, `.env`, private-key/credential/secret filenames, configured exclusions, oversized ranges, and byte overflow, and preserves relative paths and line numbers. It is not arbitrary filesystem access and never executes a command.

These controls define repository and process boundaries; they do not establish application security correctness. In particular, static annotations and routes do not prove effective Spring Security configuration, authorization behavior, proxy interception, or runtime condition outcomes.

MCP has no shell, build, network, or upload tool. Errors contain safe codes and do not reveal canonical roots. Structured logs contain tool names, project labels, generation, timing, counts, estimates, truncation, and errors—but never source code.
