# Maven support matrix

| Capability | Safe | Trusted | Notes |
| --- | --- | --- | --- |
| Single-module project | yes | yes | root must contain `pom.xml` |
| Basic parent with direct `<modules>` | yes | yes | nested/custom reactor logic is not claimed |
| `src/main/java` | yes | yes | when present |
| `src/test/java` | yes | yes | enabled by analyzer options |
| Standard annotation-processor output | existing only | existing only | no processor execution by Graphine |
| Existing `target/classes` / `target/test-classes` | yes | yes | used only when present |
| Literal local-cache dependencies | yes | yes | `${...}` versions are not expanded in safe mode |
| Full dependency classpath | no | yes | fixed Maven `dependency:build-classpath` command |
| Java release/source property | basic | basic | release, `java.version`, source, or compiler-plugin release |
| Custom source plugins/profiles | no | not claimed | only standard/explicitly recognized roots |
| Gradle | no | no | outside Phase 2 |

Safe mode treats absent artifacts as diagnostics and lets JDT recover unresolved bindings. Trusted mode may download according to Maven settings and execute Maven/plugin code; see `SAFE_TRUSTED.md`.
