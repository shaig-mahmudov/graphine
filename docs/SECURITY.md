# Repository boundary security

Registration canonicalizes the requested path and rejects files. When `allowed_repository_roots` is configured, the canonical project root must be below one of those canonical roots.

Synthetic evidence paths must be non-empty repository-relative paths. Graphine rejects Unix absolute paths, Windows drive or UNC paths, NUL or replacement characters, empty segments, `.` and `..`, missing files, and non-normal path components. The final target is canonicalized and must remain under the canonical project root, preventing symlink escape. Stored and returned evidence paths use forward-slash repository-relative form.

MCP exposes graph facts and evidence coordinates only. It has no arbitrary filesystem read, source-body, shell, build, network, or upload tool. MCP errors contain safe codes and do not reveal canonical roots. Structured logs contain tool names, project labels, generation, timing, counts, estimates, truncation, and errors—but never source code.

