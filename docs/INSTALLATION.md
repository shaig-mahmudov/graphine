# Installation and removal

Release archives contain the `graphine` executable, `lib/graphine-analyzer.jar`, and a platform-specific `graphine-rust-analyzer` executable beside `graphine`. Keep that layout intact, place the extracted directory on `PATH`, and verify `SHA256SUMS`. No component is downloaded automatically.

Analyzer discovery order is:

1. JSON `java_analyzer_jar` configuration (`analyzer_jar` is a compatibility alias);
2. `GRAPHINE_ANALYZER_JAR`;
3. `lib/graphine-analyzer.jar` relative to the executable;
4. `graphine-analyzer.jar` beside the executable; and
5. the runtime-discovered development workspace target, only in debug builds.

Rust worker discovery uses JSON `rust_analyzer_worker`, `GRAPHINE_RUST_ANALYZER`, an executable beside `graphine`, then the development target in debug builds. Configure Cargo/rustc with `cargo_executable` and `rustc_executable`.

Run `graphine analyzer doctor --language java|rust` to inspect one worker's selected source, searched paths, protocol, language, and capabilities. Run `graphine doctor` for both stacks and registered-project health. Neither command analyzes a repository or starts Maven/Cargo.

To uninstall, remove the extracted distribution directory. Local indexes are deliberately separate; remove the configured `data_dir` only if its registered-project history and SQLite graph are no longer needed. Graphine never modifies a Maven repository during safe analysis.

Maintainers can build checksum-bearing archives with `scripts/package-release.ps1` on Windows or `scripts/package-release.sh` on Linux/macOS. The shell archive name should be adjusted by the release workflow for the actual operating system and architecture.
