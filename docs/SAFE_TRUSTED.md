# Safe and trusted analysis

`graphine analyze <project> --mode safe` is the default security posture. It reads registered source/POM files, existing output directories, explicit classpath entries, reactor models, and bounded parent/dependency POMs already in the local Maven cache. It resolves the documented property/dependency-management subset without network access and never starts Maven or plugins. Missing or unsupported versions remain visible as unresolved diagnostics.

For Rust, safe mode forces locked/offline Cargo metadata when available and explicitly disables rust-analyzer build scripts, build-data execution, and procedural macros. If the lockfile or local dependency cache is insufficient, Graphine falls back to a manifest-only graph of local workspace members and path dependencies without changing the repository. It never runs tests, examples, benches, project binaries, or `build.rs`. Generated or unavailable code remains a structured unresolved diagnostic.

`graphine analyze <project> --mode trusted` explicitly authorizes Maven execution for that invocation. Graphine constructs an argument vector for `dependency:build-classpath`, runs it in the canonical registered root, filters the environment, bounds combined Maven output, and applies the request timeout. It never concatenates a shell command and never exposes this path through MCP.

For Rust, trusted mode authorizes configured Cargo source resolution, a fixed `cargo check --all-targets`/rust-analyzer build-data phase, build scripts, and procedural macros. Graphine uses dedicated target subdirectories. These processes and macros inherit the user's OS permissions.

Trusted mode is not a sandbox: Maven extensions/plugins, Cargo build scripts, procedural macros, configuration, and downloaded artifacts can execute code with the worker's OS permissions. Use it only for a repository and toolchain configuration you trust.

Both modes keep repository code local. Neither mode uploads source to Graphine. Partial generation activation remains disabled unless `--allow-partial` or `allow_partial_activation` is explicitly set.
