# Synthetic graph format

Phase 1 accepts deterministic JSON so database, query, and MCP behavior can be tested before Java analysis exists. `fixtures/spring-web/graphine.synthetic.json` is the reference example.

Each node requires a stable ID, kind, qualified name, confidence, and provenance. Optional simple name, module/package, evidence range, metadata object, and unresolved strings are supported. Each edge requires existing endpoints in the same document, kind, confidence, provenance, and optional metadata object.

Validation rejects duplicate stable IDs, duplicate `(source, target, kind)` edges, foreign endpoints, invalid stable IDs, non-object metadata, incomplete or invalid line ranges, unsafe paths, missing evidence targets, and empty graphs. Loading begins a generation before validation so failure is visible, but activation occurs only after all rows and invariants commit.

Stable ID forms are:

```text
project:<uuid>
module:<module-name>
package:<qualified-package>
type:<fully-qualified-type>
method:<fully-qualified-type>#method(parameter-types)
field:<fully-qualified-type>#field
route:<HTTP-METHOD>:<normalized-path>
bean:<bean-name-or-qualified-type>
entity:<fully-qualified-type>
```

Method parameter types are part of identity; method names alone are never assumed unique. Synthetic confidence and provenance describe fixture assertions, not analysis performed by Graphine.
