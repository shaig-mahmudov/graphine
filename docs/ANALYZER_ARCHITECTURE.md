# Java analyzer architecture

Graphine keeps Java analysis behind an independently runnable JVM process:

```text
explicit CLI analyze
  -> Rust supervisor
  -> versioned JSON request
  -> shaded Java worker
       -> Maven model discovery
       -> Eclipse JDT batch AST + bindings
       -> normalized nodes, edges, diagnostics
  -> bounded JSONL stream validation
  -> one SQLite ingestion transaction
  -> active generation
```

`analyzer-jdt` is a Maven reactor with five modules. `analyzer-protocol` contains language-neutral records, the graph sink, and JSON serialization. `project-resolver` discovers standard Maven roots and classpaths. `java-symbol-analyzer` owns stable-ID normalization and the generic JDT visitor. `spring-static-analyzer` consumes the same already-parsed compilation units, retains only normalized descriptors, and emits framework facts after all units have been seen. `analyzer-cli` reads one bounded request and packages the shaded `graphine-analyzer.jar`.

The worker configures the project Java release, source roots, classpath, binding resolution, binding recovery, and statement recovery, then calls JDT batch `createASTs`. Each compilation unit is visited and released before the next is retained. Source bodies and JDT binding keys are never stored. Directly referenced external types and call targets receive lightweight `external: true` nodes; Graphine does not enumerate JDK implementations.

The Rust client owns process lifetime. It writes one bounded request, reads stdout incrementally, caps stdout and stderr, enforces cancellation/timeout, and kills the worker on any protocol failure. It validates the event order, request/version, stable IDs, duplicates, path shape, metadata objects, edge endpoints, and summary counts before database ingestion. A worker cannot activate a generation itself.

Call edges target the compiler-resolved declaration. Metadata distinguishes `exactly_resolved` from `virtual_declared_target`. The latter is not a claim about the runtime implementation. Method references include `runtime_execution: not_proven`; lambdas store their resolved functional-interface type without claiming execution.

Spring rules never live in the generic symbol visitor. The dedicated bounded pass interprets resolved annotations, safe recovered import names, constants, compiler types, method calls, Maven source metadata, and configuration keys. It emits framework evidence with `FRAMEWORK_RESOLVED`, `STATIC_INFERRED`, `AMBIGUOUS`, or `UNRESOLVED`; Phase 3 never emits `RUNTIME_CONFIRMED`.
