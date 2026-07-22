# Maven support matrix

| Capability | Safe | Trusted | Notes |
| --- | --- | --- | --- |
| Single-module project | yes | yes | root must contain `pom.xml` |
| Parent and nested reactor modules | bounded | bounded | direct-child XML only; depth is capped |
| `src/main/java` | yes | yes | when present |
| `src/test/java` | yes | yes | enabled by analyzer options |
| Standard annotation-processor output | existing only | existing only | no processor execution by Graphine |
| Existing `target/classes` / `target/test-classes` | yes | yes | used only when present |
| Local reactor dependencies | yes | yes | inherited group/version and reactor dependency management supported |
| Local-cache dependencies | bounded | yes | cached parent/dependency POMs and transitive dependencies are depth/count capped |
| Maven properties | bounded | bounded | project properties, `${project.version}`, `${pom.version}`, and `${revision}`; cycles fail explicitly |
| Full dependency classpath | no | yes | fixed Maven `dependency:build-classpath` command |
| Java release/source property | basic | basic | release, `java.version`, source, or compiler-plugin release |
| Custom source plugins/profiles | no | not claimed | only standard/explicitly recognized roots |
| Gradle | no | no | unsupported |

Safe mode treats absent artifacts as diagnostics and lets JDT recover unresolved bindings. Trusted mode may download according to Maven settings and execute Maven/plugin code; see `SAFE_TRUSTED.md`.

The safe resolver is intentionally not an effective-Maven-model implementation. It reads only direct XML children for project identity, parent, modules, properties, dependencies, and dependency management; it does not activate profiles, execute plugins/extensions, read remote repositories, or interpolate environment/system properties. Resolved classpath entries record `local_reactor`, `local_cache`, or `explicit_classpath` provenance. Content fingerprints hash repository-relative Java/POM paths and bytes plus analyzer/protocol/mode/source-set configuration and classpath content; timestamps and absolute repository paths are excluded.
