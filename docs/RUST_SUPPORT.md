# Rust support

Rust v1 supports Cargo packages and workspaces. Graphine discovers libraries, binaries, integration tests, examples, benches, procedural-macro targets, and build-script sources. It indexes crates, modules, types, traits, implementations, functions, methods, calls, imports, type relationships, fields, variants, constants, statics, macros, associated types, tests, evidence, and structured uncertainty.

The worker is `graphine-rust-analyzer`. It pins `ra_ap_hir`, `ra_ap_ide_db`, `ra_ap_load-cargo`, `ra_ap_project_model`, `ra_ap_syntax`, and `ra_ap_vfs` to `0.0.342`; those APIs never cross analyzer protocol v2. Compiler-resolved facts use `COMPILER_RESOLVED`. Syntax-only recovery uses `STATIC_INFERRED`. Missing dependencies, inactive `cfg` branches, unavailable generated code, and ambiguous targets produce diagnostics rather than invented edges.

`--features`, `--all-features`, `--no-default-features`, and `--target` are part of the analysis request and source fingerprint. Conflicting feature selectors are rejected. Fingerprints also include language, mode, worker/toolchain versions, manifests, lock/config/toolchain files, Rust sources, and trusted generated inputs.

Safe mode is offline and does not execute `build.rs`, procedural macros, tests, examples, benches, or project binaries. It requests locked offline Cargo loading when possible and falls back to local manifests/workspace members/path dependencies without modifying the repository. Built-in and declarative macro information available from rust-analyzer is accepted; unavailable procedural expansion remains explicit.

Trusted mode requires `--mode trusted`. It runs a fixed Cargo check/build-data phase in a dedicated target subdirectory, may use configured Cargo sources, and enables build scripts and procedural macros for rust-analyzer. That code inherits the user's OS permissions; trusted mode is not an operating-system sandbox.

Dynamic trait-object calls target the declared trait method with `dispatch: dynamic_trait` and never claim a concrete runtime implementation. Statically typed receivers may target a compiler-resolved implementation. Calls lexically inside closures are attributed to the nearest named function and marked as closure-originated. Impl blocks, closures, and local variables are not standalone v1 nodes.

Non-Cargo layouts, cross-language call inference, Axum/Actix/Rocket semantics, nightly-only completeness, runtime tracing, and incremental indexing are not supported. Nightly syntax is best effort and must surface uncertainty.
