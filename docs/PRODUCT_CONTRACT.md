# Graphine product contract

## Problem and users

AI coding agents often discover Java, Spring, and Rust structure by repeatedly searching and opening source files. That workflow spends tokens on syntax rather than verified relationships and can turn framework or dispatch inference into unjustified certainty. Graphine is intended for coding agents and developers working on non-trivial Maven/Spring Boot and Cargo repositories.

Graphine's job is to answer structural questions: call and impact relationships, types, imports, fields, traits and implementations, tests, Spring route/application flow, and the minimum source evidence needed to verify a claim.

## Agent workflow

An agent asks a narrow structural question through MCP. Graphine returns compact typed facts, confidence, evidence ranges, ambiguities, and unresolved items. The agent may then open only the cited evidence or request a broader traversal. Answers must remain useful without dumping whole source files.

An expected response shape is:

```json
{
  "answer": {"handler": "example.EventController#create"},
  "confidence": "FRAMEWORK_RESOLVED",
  "evidence": [{"file": "src/main/java/example/EventController.java", "start_line": 20, "end_line": 24}],
  "unresolved": [],
  "ambiguities": []
}
```

Paths are repository-relative. Each material claim must be traceable to source, compiler, bytecode, framework, or runtime evidence at the declared confidence level.

## Local-first privacy guarantee

Indexing, parsing, storage, and query processing run on the user's machine by default. Graphine will not upload repository content, derived symbols, embeddings, or query results to a Graphine-operated service. A future explicitly configured adapter may invoke a third-party service, but it must be opt-in, separately documented, and outside the local-first core. Safe analysis, benchmark tools, and graph/MCP queries perform no network calls; explicit trusted analysis follows the operator's Maven or Cargo configuration and may execute Rust build-time code.

## Expected MCP behavior

The current MCP surface provides seven deterministic, bounded structural queries; accepts repository scope and budgets; distinguishes empty results from incomplete analysis; identifies stale or partial indexes; returns repository-relative evidence; exposes confidence and dispatch metadata; and preserves ambiguity. Errors are explicit and machine-readable. The contract is implemented over compiler-resolved Java graphs, bounded Spring static semantics, and rust-analyzer-backed Cargo graphs. Endpoint context is Spring-only; general structural tools apply to both languages. These are static structural results, not runtime traces.

## Evidence, ambiguity, and accuracy

Evidence is a list of minimal file ranges or stronger compiler, bytecode, framework, or runtime observations. Ambiguity is first-class data: candidates and the missing condition needed to distinguish them must be returned. `unresolved` records missing dependencies, incomplete indexing, unsupported language features, or absent runtime evidence.

> The system must prefer an honest unresolved or ambiguous answer over an unsupported deterministic answer.

“Accurate” means every asserted structural fact is supported by evidence at or above the required confidence, expected facts are not omitted within declared scope, unsupported claims are absent, and uncertainty is represented honestly. Static evidence alone must never be described as runtime proof.

“Token efficient” means the complete response, plus optional verification reads, uses fewer model input tokens than the controlled baseline while preserving answer quality. Concision alone is not success; token reductions are measured only under the methodology and must not conceal missing facts.

## Claims Graphine will not make

Graphine will not claim perfect program understanding, runtime certainty from static analysis, security correctness, complete dead-code proof, or benchmark improvements that were not measured. It will not market planned analyzers as available. The broader non-goal boundary is in `NON_GOALS.md`.

## First stable release scope

The first stable release indexes exactly two registered project languages: Java 17+ Maven/Spring Boot and stable Rust Cargo projects supported by the pinned analyzer. Each registration has one language. It serves evidence-first queries for normalized symbols, calls, types, implementations, imports, and fields, plus Java-only Spring facts. The current implementation performs full generation-based reindexing; incremental indexing is not implemented. Runtime probes, semantic embeddings, cross-language call inference, Rust web-framework semantics, a third language, and exhaustive Spring condition evaluation remain outside scope.
