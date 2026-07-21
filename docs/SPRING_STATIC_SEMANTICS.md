# Spring static semantics

Phase 3 converts compiler facts into conservative Spring framework facts without starting Spring. The `spring-static-analyzer` receives the JDT compilation units already parsed by the Java analyzer, stores a bounded normalized intermediate model, and emits into the same language-neutral graph sink.

## Supported facts

- Direct and provable meta-annotated `@Component`, `@Service`, `@Repository`, `@Controller`, `@RestController`, and `@Configuration` stereotypes.
- Default and explicit bean names, `@Bean` aliases and return types, simple implementation returns, `@Primary`, `@Qualifier`, constructor/factory injection, optional/provider/collection shapes, candidates, selections, and explicit ambiguity.
- Default Spring Boot scan roots and explicit `@ComponentScan` packages/classes. Unknown roots retain static candidates and emit a diagnostic.
- Profiles and `ConditionalOnProperty`, `ConditionalOnBean`, and `ConditionalOnMissingBean` as unevaluated conditions. They never prove runtime eligibility.
- Class/method MVC mapping composition, aliases, verbs, media constraints, params, headers, binding kinds, external names, required flags, validation markers, response status metadata, and duplicate-route diagnostics.
- Spring Data repository domain/ID types, common inherited CRUD method facts, bounded derived-query parsing, `@Query`/`@Modifying` metadata, and conservative entity read/write edges from repository calls.
- JPA entities, IDs, persistent fields, explicit columns, and direct relationship cardinality/ownership metadata.
- `@Value`, `@ConfigurationProperties`, properties/YAML keys, and profile-specific files. Values are never stored; secret-like key names are marked only as metadata.
- Static `ApplicationEventPublisher.publishEvent` and `@EventListener` links, plus transaction annotation metadata with runtime proxying explicitly unconfirmed.

Every derived node or edge has `spring-static-v1` provenance and repository-relative evidence. Existing Java type, method, and field identities are enriched rather than duplicated.

## Confidence boundary

`FRAMEWORK_RESOLVED` means a deterministic static Spring rule succeeded. `STATIC_INFERRED` covers unknown scan roots, profiles, conditions, conditional control flow, and runtime proxy/context assumptions. `AMBIGUOUS` means multiple valid candidates or handlers remain. Missing constants, symbols, qualifiers, domains, relationships, or property keys produce structured diagnostics. Phase 3 never emits `RUNTIME_CONFIRMED`.

## Deliberate limits

Graphine does not reproduce the Spring condition evaluator, run component scanning, evaluate SpEL, start an application, inspect environment values, infer effective security, prove transaction interception, execute repository queries, compare database migrations, or implement the Phase 4 endpoint MCP tool. Complex generic factories, custom runtime bean registrars, dynamic route registration, full JPQL/SQL parsing, and runtime event ordering remain unresolved.

## Verification

Run `scripts/evaluate-spring-accuracy.ps1` (or `.sh`) for checked-in graph ground truth, `scripts/test-phase3-e2e.ps1` (or `.sh`) for analyzer-to-MCP ingestion, and `scripts/benchmark-spring-medium.ps1` for the deterministic medium corpus report.
