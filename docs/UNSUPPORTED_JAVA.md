# Known Java analyzer limitations

Phase 2 does not yet fully model record components as dedicated component/field nodes, invocation-site generic substitutions, local/anonymous class stable identities, candidate virtual-dispatch sets, bytecode bridge methods, module descriptors, pattern-variable data flow, annotation default/constant folding beyond explicit source values, or custom generated/source roots contributed by arbitrary Maven plugins.

The analyzer records a resolved virtual declaration target but does not enumerate every possible runtime implementation. It records lambda functional-interface type and method-reference targets without proving execution. Syntax recovery is best effort; only recoverable units can contribute a partial graph. External symbols are deliberately limited to directly referenced types and methods.

Safe mode cannot resolve property-driven dependency versions or artifacts absent from the local cache without executing Maven. Basic direct Maven modules are supported; unusual profiles, nested reactor behavior, extensions, and custom plugins are not claimed. Gradle is unsupported.

No Phase 2 fact performs Spring bean selection, injection, composed route resolution, Spring Security, JPA, transaction, event, or runtime semantics. Those begin in a later phase.
