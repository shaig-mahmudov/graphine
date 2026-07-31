# Beta readiness

## Current maturity

Graphine is an advanced technical alpha moving toward public beta. Its architecture, local generation store, Java/Spring analyzer, evidence-first query layer, benchmark schemas, and cross-platform validation are substantial enough for external evaluation. The checked-in fixtures and generated corpora are controlled engineering evidence, not proof of compatibility or accuracy across arbitrary repositories.

## Implemented capabilities

- Local registration of repositories and immutable SQLite index generations with atomic activation, stale/partial state, diagnostics, and source fingerprints.
- Explicit safe or trusted analysis of supported Java 17+ Maven projects by an independently packaged Eclipse JDT worker.
- Compiler-resolved Java types, methods, fields, direct call declarations, type hierarchy, annotations, field access, and repository-relative evidence within the documented limits.
- Bounded Spring static semantics for common stereotypes, beans and injection, MVC routes, Spring Data/JPA, configuration keys, profiles/selected conditions, events, and transaction annotations.
- Seven read-only STDIO MCP tools: project map, symbol search, symbol context, endpoint context, bounded flow tracing, evidence snippets, and index status.
- Deterministic response budgets, pagination, freshness/completeness disclosure, confidence, ambiguity, unresolved diagnostics, and generation-bound evidence identifiers.
- Repository containment and sensitive-file rejection for evidence reads; bounded analyzer requests/output/time and explicit Maven trust separation.
- Analyzer metadata inspection that reports packaged Java, Spring static-semantic, Maven trusted-mode, and protocol capabilities without analyzing a repository or starting Maven.
- Checked-in fixture ground truth, graph-accuracy evaluation, Phase 2/3 E2E scripts, Phase 4 query tests and local MCP latency measurement, plus Linux/Windows CI definitions.

## Unsupported or partial areas

| Area | Current status |
| --- | --- |
| Gradle | Unsupported. Doctor reports this explicitly. |
| Lombok | Partial at best when already generated/compiled artifacts make symbols visible; Graphine does not run Lombok or claim source transformation fidelity. |
| Annotation processors | Not executed by Graphine. Existing generated outputs may contribute only through supported discovered roots or classpath artifacts. |
| Generated sources | Standard existing Maven output is bounded/partial; arbitrary plugin-generated source roots are not discovered or claimed. |
| Custom Maven source roots | Unsupported unless supplied through an already supported explicit mechanism; arbitrary build-helper/plugin behavior is not evaluated. |
| Complex Maven profiles | Safe mode does not activate profiles or compute the full effective Maven model. Trusted classpath resolution delegates only the fixed documented operation to Maven. |
| Multi-module edge cases | Direct bounded reactors, parents, and dependency management are supported; unusual nesting, extensions, cycles, custom packaging, and complex inheritance require real-world validation. |
| Runtime Spring behavior | Not observed. Static paths are possible structure, not execution traces. |
| Effective Spring Security | Not inferred or proven. Security annotations and routes do not establish the effective filter chain or authorization result. |
| Transaction proxy behavior | Transaction annotations are recorded, but effective proxy creation, self-invocation behavior, propagation, rollback, and interception are not proven. |
| Runtime-dependent conditional beans | Profiles and selected conditions are retained as unevaluated constraints when the environment is unknown; exhaustive condition evaluation is unsupported. |
| SpEL | Not evaluated. |
| Kotlin and other JVM languages | Unsupported as source languages. Directly referenced compiled symbols may appear only as external classpath facts. |
| Incremental indexing | Unsupported. Each analysis creates a complete candidate generation before atomic activation. |
| Remote transports | Unsupported. MCP transport is local STDIO only. |
| Operating-system sandboxing | Not provided. Safe mode avoids Maven/plugin execution, but the analyzer still runs with the invoking user's OS permissions. |

The analyzer also has documented limits around Lombok-like transformations, local/anonymous identities, generic substitutions, virtual dispatch candidate sets, bytecode bridges, module descriptors, dynamic Spring registration, JPQL/SQL semantics, runtime event order, and database migration behavior.

## Beta exit criteria

The public beta should not be declared until all of these measurable criteria are met:

- Every required CI job passes on Linux and Windows at the release commit: formatting, Clippy with warnings denied, Rust workspace tests, benchmark validation, Java 17/21 tests/package, Phase 2/3 E2E, and Java/Spring fixture accuracy.
- README, product, architecture, analyzer, Spring, MCP, security, CI, performance, installation, compatibility, and non-goal documents are reviewed against the release binaries and expose no known contradiction.
- At least three public, non-fixture Spring repositories of different sizes and at least one non-trivial multi-module repository are indexed from clean checkouts; versions, modes, timings, diagnostics, unsupported constructs, and manually audited samples are published.
- Evaluation emits zero forbidden deterministic claims on the supported checked-in corpus, and reports fixture-scoped precision/recall without extrapolating to arbitrary projects.
- Binary installation, analyzer discovery, upgrade, removal, and checksum verification instructions are executable as written on supported platforms.
- Linux and Windows release archives are produced from a tagged commit by an automated workflow with recorded toolchains, checksums, provenance, and a reproducibility procedure.
- A paired baseline-versus-Graphine agent evaluation is completed with the same model, prompts, repository state, limits, and provider-reported token counts; correctness, unsupported claims, file reads, tool calls, tokens, and timing are published even if targets are missed.
- A tested compatibility matrix is published for operating system, architecture, Java, Maven, Spring Boot, Maven project shapes, Lombok/processor/generated-source behavior, and known failure modes.
- A clean-install smoke test verifies that `graphine doctor` reports Java and Spring only when the compatible packaged analyzer is present, and reports missing/mismatched analyzers without starting Maven.

## Known risks

- **Correctness:** Recovery bindings, static dispatch, ambiguous bean selection, dynamic routes, conditions, generics, reflection, and generated code can produce missing or conservative results. Confidence and diagnostics reduce but do not eliminate misuse by callers.
- **Compatibility:** Fixture and generated-corpus coverage is much narrower than the Maven/Spring ecosystem. Parent POMs, profiles, plugins, modules, toolchains, Spring versions, and platform path behavior can expose untested cases.
- **Security boundary:** Safe mode avoids Maven/Cargo project-code execution and evidence reads are contained, but there is no OS sandbox. Trusted mode can execute Maven extensions/plugins, Cargo build scripts, procedural macros, or downloaded code with user permissions. Static results do not prove application security.
- **Performance:** Current reports are single-environment engineering samples. Large graphs are validated in memory before ingestion, indexing is full-generation, SQLite access is synchronous, and repository-scale memory/latency ceilings are not established.
- **Packaging:** Packaging scripts exist, but automated signed/tagged release publication, artifact provenance, upgrade testing, and broad architecture coverage are not yet demonstrated.

## Current assessment

The codebase is suitable for controlled external alpha use and real-repository validation. Public beta readiness depends primarily on compatibility evidence, reproducible distribution, and paired agent evaluation rather than additional query features.
