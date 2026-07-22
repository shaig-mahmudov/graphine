# Repository boundary security

## Repository and MCP boundary

Registration canonicalizes the requested path and rejects files. When `allowed_repository_roots` is configured, the canonical project root must be below one of those canonical roots.

Synthetic evidence paths must be non-empty repository-relative paths. Graphine rejects Unix absolute paths, Windows drive or UNC paths, NUL or replacement characters, empty segments, `.` and `..`, missing files, and non-normal path components. The final target is canonicalized and must remain under the canonical project root, preventing symlink escape. Stored and returned evidence paths use forward-slash repository-relative form.

## Analyzer boundary

Safe analysis discovers only standard Maven source roots, existing `target/classes` directories, explicit classpath entries, and literal dependency artifacts already present in the local Maven cache. It does not start Maven or execute repository-controlled plugins or scripts.

Trusted analysis must be explicitly requested from the CLI. It starts Maven in the canonical registered project root with a fixed argument vector for `dependency:build-classpath`; Graphine does not use `sh -c`, `cmd /c`, string interpolation, or user-supplied Maven goals. The process has a filtered environment, bounded output, and a timeout. Maven and plugins are still code execution, so trusted mode is appropriate only for repositories the operator trusts. It is never reached from an MCP read.

The Rust supervisor limits request size, stdout, and captured stderr; validates every JSONL event, version, request ID, stable ID, path, duplicate, edge endpoint, and summary count; and kills the worker on timeout, cancellation, output overflow, or protocol failure. Source text and Maven output are not logged. Spring analysis never starts an application context, evaluates SpEL, collects environment values, or performs network calls. Configuration values are not persisted; secret-like keys are flagged using key names only. Operating-system sandboxing of the worker is outside Graphine's current scope.

MCP exposes graph facts plus bounded source bodies only through `get_evidence`. That tool accepts generation-bound indexed IDs or explicit repository-relative ranges, canonicalizes root and target, rejects symlink escape, binary/non-UTF-8 files, `.env`, private-key/credential/secret filenames, configured exclusions, oversized ranges, and byte overflow, and preserves relative paths and line numbers. It is not arbitrary filesystem access and never executes a command.

These controls define repository and process boundaries; they do not establish application security correctness. In particular, static annotations and routes do not prove effective Spring Security configuration, authorization behavior, proxy interception, or runtime condition outcomes.

MCP has no shell, build, network, or upload tool. Errors contain safe codes and do not reveal canonical roots. Structured logs contain tool names, project labels, generation, timing, counts, estimates, truncation, and errors—but never source code.
