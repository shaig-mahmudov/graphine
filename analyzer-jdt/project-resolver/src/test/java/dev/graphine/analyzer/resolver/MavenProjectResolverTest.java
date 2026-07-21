package dev.graphine.analyzer.resolver;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.graphine.analyzer.protocol.AnalysisRequest;
import dev.graphine.analyzer.protocol.AnalyzerMode;
import dev.graphine.analyzer.protocol.AnalyzerOptions;
import dev.graphine.analyzer.protocol.Diagnostic;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class MavenProjectResolverTest {
    @TempDir Path root;

    @Test
    void safe_mode_discovers_declared_modules_without_executing_maven() throws Exception {
        Files.writeString(root.resolve("pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><groupId>sample</groupId><artifactId>parent</artifactId>
                <version>1</version><packaging>pom</packaging><modules><module>api</module></modules>
                <properties><maven.compiler.release>21</maven.compiler.release></properties>
                <dependencies><dependency><groupId>missing</groupId><artifactId>library</artifactId>
                <version>999</version></dependency></dependencies></project>
                """);
        Path module = Files.createDirectories(root.resolve("api/src/main/java/sample"));
        Files.writeString(module.resolve("Api.java"), "package sample; interface Api {}\n");
        Files.writeString(root.resolve("api/pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><groupId>sample</groupId><artifactId>api</artifactId>
                <version>1</version></project>
                """);
        AnalysisRequest request = new AnalysisRequest(1, "test", "analyze_project", root, AnalyzerMode.safe,
                List.of("main"), new AnalyzerOptions(true, true, false, List.of()),
                root.resolve("must-not-run-maven"), 10_000);
        List<Diagnostic> diagnostics = new ArrayList<>();

        ProjectModel model = new MavenProjectResolver().resolve(request, diagnostics::add);

        assertEquals(2, model.moduleRoots().size());
        assertEquals(1, model.sourceRoots().size());
        assertEquals("main", model.sourceRoots().get(0).sourceSet());
        assertEquals("21", model.javaRelease());
        assertFalse(model.fingerprint().isBlank());
        assertTrue(diagnostics.stream().anyMatch(value -> "UNRESOLVED_MAVEN_DEPENDENCY".equals(value.kind())));
    }
}
