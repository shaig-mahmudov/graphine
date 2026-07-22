# Installation and removal

When Graphine is distributed as a release archive, it contains the `graphine` executable and `lib/graphine-analyzer.jar`. Keep that relative layout intact, place the extracted directory on `PATH`, and verify the archive against `SHA256SUMS`. No component is downloaded automatically. The repository currently provides packaging scripts; automated reproducible release publication and signing remain beta-exit work.

Analyzer discovery order is:

1. JSON `analyzer_jar` configuration;
2. `GRAPHINE_ANALYZER_JAR`;
3. `lib/graphine-analyzer.jar` relative to the executable;
4. `graphine-analyzer.jar` beside the executable; and
5. the runtime-discovered development workspace target, only in debug builds.

Run `graphine analyzer doctor` to see the selected source, every searched path, worker-reported protocol, and packaged capability list. Run `graphine doctor` to check SQLite, Java, analyzer compatibility, Maven trusted-mode availability, explicit Gradle non-support, and derived Java/Spring capabilities. Neither command analyzes a repository or starts Maven.

To uninstall, remove the extracted distribution directory. Local indexes are deliberately separate; remove the configured `data_dir` only if its registered-project history and SQLite graph are no longer needed. Graphine never modifies a Maven repository during safe analysis.

Maintainers can build checksum-bearing archives with `scripts/package-release.ps1` on Windows or `scripts/package-release.sh` on Linux/macOS. The shell archive name should be adjusted by the release workflow for the actual operating system and architecture.
