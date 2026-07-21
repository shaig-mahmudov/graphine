# Confidence, provenance, and accuracy

Confidence describes the strongest evidence supporting a claim, not an arbitrary probability. Results may contain claims at different levels; each claim should carry its own provenance when a response combines them. Ordering below is evidentiary, not a guarantee that stronger evidence is available.

| Level | Required evidence | Permitted claims | Prohibited claims | Deterministic? | Inspect source? |
| --- | --- | --- | --- | --- | --- |
| `RUNTIME_CONFIRMED` | A reproducible probe or trace from the specified repository state and scenario | The observed call, target, transaction, or bean occurred in that run | That all inputs or deployments behave identically | Deterministic for the captured observation; not universally | Recommended when explaining why |
| `BYTECODE_CONFIRMED` | Successfully loaded project bytecode with resolved members/instructions | Bytecode-level owners, signatures, annotations, and call instructions | Actual virtual target or runtime framework behavior without more evidence | Yes for the artifact hash | Usually not required, but useful for source mapping |
| `COMPILER_RESOLVED` | Successful compiler-grade symbol and type resolution for the source set | Overloads, types, overrides, direct static targets, generic substitutions | Runtime dispatch or framework selection not fixed by Java semantics | Yes for inputs and compiler configuration | Optional when evidence ranges suffice |
| `FRAMEWORK_RESOLVED` | Compiler-resolved code plus applicable deterministic Spring metadata and conditions | Composed routes, eligible beans, qualifier/primary selection, repository semantics, declared event listeners | Runtime activation when profiles/properties/environment are unknown | Yes only when all relevant conditions are known; otherwise return ambiguity | Recommended for condition-sensitive claims |
| `STATIC_INFERRED` | Traceable syntax or conservative local analysis without full resolution | Candidate relationships and explicitly qualified inferences | “Definitely,” unique targets, runtime behavior, or complete impact | No; it is a bounded inference | Yes |
| `AMBIGUOUS` | Evidence for two or more viable outcomes and the unresolved discriminator | Candidate set, known constraints, and what would resolve it | Selecting one candidate without evidence | Deterministic as a statement of known ambiguity | Yes, at candidate/condition ranges |
| `UNRESOLVED` | A recorded failure, missing dependency, unsupported construct, stale state, or absent evidence | What could not be resolved, why, and suggested next evidence | Any positive structural conclusion that depends on the missing fact | Deterministic as a failure report | Yes when source inspection could unblock it |

Every benchmark answer supports this common envelope:

```json
{
  "confidence": "COMPILER_RESOLVED",
  "evidence": [{"file": "src/main/java/example/EventService.java", "start_line": 20, "end_line": 31}],
  "unresolved": [],
  "ambiguities": []
}
```

Absolute paths are forbidden in corpus data. Evidence ranges are one-based, inclusive, fixture-relative, non-empty, and automatically checked against existing files. `answerable`, `ambiguous`, `unresolved`, and `unsupported` distinguish the nature of ground truth; confidence explains its provenance.

Accuracy scoring will compare normalized atomic claims. Precision is supported expected claims divided by all deterministic claims made. Recall is recovered required claims divided by required claims. Any deterministic claim forbidden by ground truth counts as an unsupported claim. An answer receives no benefit for hiding required facts behind generic uncertainty, and no penalty for returning an allowed ambiguity where the fixture genuinely lacks a discriminator.

