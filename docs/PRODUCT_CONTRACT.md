# Graphine product contract

## Problem and users

AI coding agents often discover Java and Spring structure by repeatedly searching and opening source files. That workflow spends tokens on syntax rather than verified relationships and can turn framework inference into unjustified certainty. Graphine is intended for coding agents and the developers operating them on non-trivial Java and Spring Boot repositories.

Graphine's job is to answer structural questions: route composition; controller-service-repository-entity flow; call and impact relationships; bean candidates and selection; event publication and consumption; transaction evidence; and the minimum source evidence needed to verify a claim.

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

Indexing, parsing, storage, and query processing run on the user's machine by default. Graphine will not upload repository content, derived symbols, embeddings, or query results to a Graphine-operated service. A future explicitly configured adapter may invoke a third-party service, but it must be opt-in, separately documented, and outside the local-first core. Phase 0 benchmark tools and the Phase 1 graph/MCP service perform no network calls.

## Expected MCP behavior

The stable MCP surface provides deterministic, bounded structural queries; accepts repository scope and budgets; distinguishes empty results from incomplete analysis; identifies stale indexes; returns repository-relative evidence; exposes confidence; and preserves ambiguity. Errors are explicit and machine-readable. Phase 1 implements this contract over synthetic graph data; real Java and Spring analysis remains future work.

## Evidence, ambiguity, and accuracy

Evidence is a list of minimal file ranges or stronger compiler, bytecode, framework, or runtime observations. Ambiguity is first-class data: candidates and the missing condition needed to distinguish them must be returned. `unresolved` records missing dependencies, incomplete indexing, unsupported language features, or absent runtime evidence.

> The system must prefer an honest unresolved or ambiguous answer over an unsupported deterministic answer.

“Accurate” means every asserted structural fact is supported by evidence at or above the required confidence, expected facts are not omitted within declared scope, unsupported claims are absent, and uncertainty is represented honestly. Static evidence alone must never be described as runtime proof.

“Token efficient” means the complete response, plus optional verification reads, uses fewer model input tokens than the controlled baseline while preserving answer quality. Concision alone is not success; token reductions are measured only under the methodology and must not conceal missing facts.

## Claims Graphine will not make

Graphine will not claim perfect program understanding, runtime certainty from static analysis, security correctness, complete dead-code proof, or benchmark improvements that were not measured. It will not market planned analyzers as available. The broader non-goal boundary is in `NON_GOALS.md`.

## First stable release scope

The first stable release is expected to index a local Java 17+ / Spring Boot repository and serve evidence-first queries for symbols, resolved direct calls, type hierarchies, composed HTTP mappings, common constructor bean injection, Spring Data repositories/entities, application events, and transaction annotations. It should support incremental refresh, deterministic machine-readable output, and documented limits. Runtime probes, semantic embeddings, multi-language analysis, and exhaustive Spring condition evaluation are outside that initial scope unless separately promoted after evaluation.
