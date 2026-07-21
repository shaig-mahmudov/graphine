# Installation and removal

Graphine release archives contain the `graphine` executable and `lib/graphine-analyzer.jar`. Keep that relative layout intact, place the extracted directory on `PATH`, and verify the archive against `SHA256SUMS`. No component is downloaded automatically.

Analyzer discovery order is:

1. JSON `analyzer_jar` configuration;
2. `GRAPHINE_ANALYZER_JAR`;
3. `lib/graphine-analyzer.jar` relative to the executable;
4. `graphine-analyzer.jar` beside the executable; and
5. the runtime-discovered development workspace target, only in debug builds.

Run `graphine analyzer doctor` to see the selected source and every searched path. Run `graphine doctor` to check SQLite, Java, the analyzer protocol, Maven availability, and derived capabilities.

To uninstall, remove the extracted distribution directory. Local indexes are deliberately separate; remove the configured `data_dir` only if its registered-project history and SQLite graph are no longer needed. Graphine never modifies a Maven repository during safe analysis.

Maintainers can build checksum-bearing archives with `scripts/package-release.ps1` on Windows or `scripts/package-release.sh` on Linux/macOS. The shell archive name should be adjusted by the release workflow for the actual operating system and architecture.
