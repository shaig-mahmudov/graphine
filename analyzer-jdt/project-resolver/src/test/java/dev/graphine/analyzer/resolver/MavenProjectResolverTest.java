package dev.graphine.analyzer.resolver;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

import dev.graphine.analyzer.protocol.AnalysisRequest;
import dev.graphine.analyzer.protocol.AnalyzerMode;
import dev.graphine.analyzer.protocol.AnalyzerOptions;
import dev.graphine.analyzer.protocol.Diagnostic;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
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
        AnalysisRequest request = new AnalysisRequest(2, "test", "analyze_project", root, AnalyzerMode.safe,
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

    @Test
    void direct_project_artifact_id_wins_over_parent_and_nested_modules_are_recursive() throws Exception {
        Files.writeString(root.resolve("pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><groupId>sample</groupId><artifactId>root</artifactId>
                <version>1</version><packaging>pom</packaging><modules><module>platform</module></modules></project>
                """);
        Path platform = Files.createDirectories(root.resolve("platform"));
        Files.writeString(platform.resolve("pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><parent><groupId>sample</groupId><artifactId>root</artifactId>
                <version>1</version></parent><artifactId>platform</artifactId><packaging>pom</packaging>
                <modules><module>leaf</module></modules></project>
                """);
        Path leafSource = Files.createDirectories(platform.resolve("leaf/src/main/java/sample"));
        Files.writeString(leafSource.resolve("Leaf.java"), "package sample; class Leaf {}\n");
        Files.writeString(platform.resolve("leaf/pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><parent><groupId>sample</groupId><artifactId>platform</artifactId>
                <version>1</version></parent><artifactId>leaf</artifactId></project>
                """);

        ProjectModel model = new MavenProjectResolver().resolve(request(root, List.of()), ignored -> {});

        assertEquals(3, model.moduleRoots().size());
        assertEquals("leaf", model.sourceRoots().get(0).moduleName());
    }

    @Test
    void safe_mode_resolves_inherited_coordinates_properties_management_and_reactor_modules() throws Exception {
        Files.writeString(root.resolve("pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><groupId>sample</groupId><artifactId>root</artifactId>
                <version>${revision}</version><packaging>pom</packaging><properties><revision>2</revision></properties>
                <modules><module>api</module><module>service</module></modules>
                <dependencyManagement><dependencies><dependency><groupId>sample</groupId><artifactId>api</artifactId>
                <version>${project.version}</version></dependency></dependencies></dependencyManagement></project>
                """);
        Path api = Files.createDirectories(root.resolve("api/target/classes"));
        Files.writeString(root.resolve("api/pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><parent><groupId>sample</groupId><artifactId>root</artifactId>
                <version>2</version></parent><artifactId>api</artifactId></project>
                """);
        Files.createDirectories(root.resolve("service/src/main/java/sample"));
        Files.writeString(root.resolve("service/src/main/java/sample/Service.java"), "package sample; class Service {}\n");
        Files.writeString(root.resolve("service/pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><parent><groupId>sample</groupId><artifactId>root</artifactId>
                <version>2</version></parent><artifactId>service</artifactId><dependencies><dependency>
                <groupId>sample</groupId><artifactId>api</artifactId></dependency></dependencies></project>
                """);

        ProjectModel model = new MavenProjectResolver().resolve(request(root, List.of()), ignored -> {});

        assertEquals("local_reactor", model.classpathProvenance().get(api.toRealPath()));
    }

    @Test
    void dependencies_with_repeated_artifact_ids_keep_distinct_group_coordinates() throws Exception {
        Files.writeString(root.resolve("pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><groupId>sample</groupId><artifactId>leaf</artifactId><version>1</version>
                <dependencies><dependency><groupId>one</groupId><artifactId>shared</artifactId><version>missing</version></dependency>
                <dependency><groupId>two</groupId><artifactId>shared</artifactId><version>missing</version></dependency></dependencies></project>
                """);
        List<Diagnostic> diagnostics = new ArrayList<>();

        new MavenProjectResolver().resolve(request(root, List.of()), diagnostics::add);

        assertTrue(diagnostics.stream().anyMatch(value -> "one:shared:missing".equals(value.symbolText())));
        assertTrue(diagnostics.stream().anyMatch(value -> "two:shared:missing".equals(value.symbolText())));
    }

    @Test
    void fingerprints_use_content_paths_configuration_and_classpath_not_timestamps() throws Exception {
        Files.writeString(root.resolve("pom.xml"), """
                <project><modelVersion>4.0.0</modelVersion><groupId>sample</groupId><artifactId>leaf</artifactId><version>1</version></project>
                """);
        Path source = Files.createDirectories(root.resolve("src/main/java/sample")).resolve("Same.java");
        Files.writeString(source, "package sample; class A {}\n");
        Path classpath = root.resolve("dependency.jar");
        Files.writeString(classpath, "classpath-one");
        MavenProjectResolver resolver = new MavenProjectResolver();
        String initial = resolver.resolve(request(root, List.of(classpath.toString())), ignored -> {}).fingerprint();
        FileTime timestamp = Files.getLastModifiedTime(source);
        Files.writeString(source, "package sample; class B {}\n");
        Files.setLastModifiedTime(source, timestamp);
        String edited = resolver.resolve(request(root, List.of(classpath.toString())), ignored -> {}).fingerprint();
        assertNotEquals(initial, edited);

        Path renamed = source.resolveSibling("Moved.java");
        Files.move(source, renamed);
        String moved = resolver.resolve(request(root, List.of(classpath.toString())), ignored -> {}).fingerprint();
        assertNotEquals(edited, moved);

        Files.writeString(root.resolve("pom.xml"), Files.readString(root.resolve("pom.xml")).replace("</project>", "<properties><x>1</x></properties></project>"));
        String pomChanged = resolver.resolve(request(root, List.of(classpath.toString())), ignored -> {}).fingerprint();
        assertNotEquals(moved, pomChanged);

        Files.writeString(classpath, "classpath-two");
        String classpathChanged = resolver.resolve(request(root, List.of(classpath.toString())), ignored -> {}).fingerprint();
        assertNotEquals(pomChanged, classpathChanged);
        assertEquals("explicit_classpath", resolver.resolve(request(root, List.of(classpath.toString())), ignored -> {})
                .classpathProvenance().get(classpath.toRealPath()));
    }

    private static AnalysisRequest request(Path project, List<String> classpath) {
        return new AnalysisRequest(2, "test", "analyze_project", project, AnalyzerMode.safe,
                List.of("main"), new AnalyzerOptions(true, true, false, classpath),
                project.resolve("must-not-run-maven"), 10_000);
    }
}
