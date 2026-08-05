package dev.graphine.analyzer.java;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import dev.graphine.analyzer.protocol.AnalysisRequest;
import dev.graphine.analyzer.protocol.AnalyzerMode;
import dev.graphine.analyzer.protocol.AnalyzerOptions;
import dev.graphine.analyzer.protocol.ProtocolWriter;
import dev.graphine.analyzer.resolver.ProjectModel;
import dev.graphine.analyzer.resolver.SourceRoot;
import java.io.StringWriter;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Set;
import java.util.stream.Collectors;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

final class JavaProjectAnalyzerTest {
    @TempDir Path root;

    @Test
    void extracts_overloads_calls_overrides_annotations_and_field_access() throws Exception {
        Path sourceRoot = Files.createDirectories(root.resolve("src/main/java"));
        Path source = sourceRoot.resolve("sample/Subject.java");
        Files.createDirectories(source.getParent());
        Files.writeString(source, """
                package sample;
                import java.util.function.Function;
                @interface Tag { String value(); }
                interface Parent { default String format(String value) { return value; } }
                @Tag("fixture")
                class Subject implements Parent {
                    int count;
                    Subject() {}
                    void over(String value) { count = value.length(); }
                    void over(int value) { count += value; count++; int copy = count; }
                    @Override public String format(String value) { return Parent.super.format(value); }
                    Function<String, String> reference() { return this::format; }
                    Runnable lambda() { return () -> over(1); }
                    record Nested<T>(T value) {}
                }
                """);
        ProjectModel model = new ProjectModel(root, List.of(root),
                List.of(new SourceRoot(sourceRoot, "main", "fixture", root)), List.of(), "17", "test", 0);
        AnalysisRequest request = new AnalysisRequest(2, "test", "analyze_project", root, AnalyzerMode.safe,
                List.of("main"), new AnalyzerOptions(true, true, false, List.of()), Path.of("mvn"), 10_000);
        ObjectMapper mapper = new ObjectMapper();
        StringWriter output = new StringWriter();

        AnalysisResult result = new JavaProjectAnalyzer().analyze(model, request, new ProtocolWriter(mapper, output));
        List<JsonNode> events = output.toString().lines().map(line -> {
            try { return mapper.readTree(line); }
            catch (Exception error) { throw new IllegalStateException(error); }
        }).toList();

        assertEquals("complete", result.status());
        assertEquals(1, result.summary().filesParsed());
        assertTrue(hasNode(events, "constructor:sample.Subject#<init>()"));
        assertTrue(hasNode(events, "method:sample.Subject#over(java.lang.String)"));
        assertTrue(hasNode(events, "method:sample.Subject#over(int)"));
        assertTrue(hasNode(events, "type:sample.Subject.Nested"));
        assertTrue(hasEdge(events, "method:sample.Subject#lambda()", "method:sample.Subject#over(int)", "CALLS"));
        assertTrue(hasEdge(events, "method:sample.Subject#format(java.lang.String)",
                "method:sample.Parent#format(java.lang.String)", "OVERRIDES"));
        assertTrue(hasKind(events, "READS_FIELD"));
        assertTrue(hasKind(events, "WRITES_FIELD"));
        assertTrue(events.stream().anyMatch(event -> "ANNOTATED_BY".equals(event.path("kind").asText())
                && event.path("metadata").path("explicit_values").path("value").asText().contains("fixture")
                && "not_analyzed".equals(event.path("metadata").path("spring_semantics").asText())));
        assertTrue(events.stream().anyMatch(event -> "CALLS".equals(event.path("kind").asText())
                && event.path("metadata").path("method_reference").asBoolean()));
    }

    @Test
    void syntax_failure_is_visible_while_other_units_are_recovered() throws Exception {
        Path sourceRoot = Files.createDirectories(root.resolve("recover/src/main/java/sample"));
        Files.writeString(sourceRoot.resolve("Valid.java"), "package sample; class Valid { void ok() {} }\n");
        Files.writeString(sourceRoot.resolve("Broken.java"), "package sample; class Broken { void broken( }\n");
        Path project = root.resolve("recover");
        ProjectModel model = new ProjectModel(project, List.of(project),
                List.of(new SourceRoot(project.resolve("src/main/java"), "main", "recover", project)),
                List.of(), "17", "test", 0);
        AnalysisRequest request = new AnalysisRequest(2, "recover", "analyze_project", project, AnalyzerMode.safe,
                List.of("main"), new AnalyzerOptions(true, true, false, List.of()), Path.of("mvn"), 10_000);
        StringWriter output = new StringWriter();

        AnalysisResult result = new JavaProjectAnalyzer().analyze(model, request,
                new ProtocolWriter(new ObjectMapper(), output));

        assertEquals("partial", result.status());
        assertEquals(2, result.summary().filesParsed());
        assertEquals(1, result.summary().filesFailed());
        assertTrue(output.toString().contains("JAVA_PARSE_ERROR"));
        assertTrue(output.toString().contains("type:sample.Valid"));
    }

    @Test
    void ambiguous_overload_is_a_diagnostic_not_a_deterministic_edge() throws Exception {
        Path sourceRoot = Files.createDirectories(root.resolve("ambiguous/src/main/java/sample"));
        Files.writeString(sourceRoot.resolve("Ambiguous.java"), """
                package sample;
                class Ambiguous {
                    void choose(String value) {}
                    void choose(Integer value) {}
                    void run() { choose(null); }
                }
                """);
        Path project = root.resolve("ambiguous");
        ProjectModel model = new ProjectModel(project, List.of(project),
                List.of(new SourceRoot(project.resolve("src/main/java"), "main", "ambiguous", project)),
                List.of(), "17", "test", 0);
        AnalysisRequest request = new AnalysisRequest(2, "ambiguous", "analyze_project", project, AnalyzerMode.safe,
                List.of("main"), new AnalyzerOptions(true, true, false, List.of()), Path.of("mvn"), 10_000);
        StringWriter output = new StringWriter();

        new JavaProjectAnalyzer().analyze(model, request, new ProtocolWriter(new ObjectMapper(), output));

        assertTrue(output.toString().contains("AMBIGUOUS_BINDING"));
        assertFalse(output.toString().lines().anyMatch(line -> line.contains("\"kind\":\"CALLS\"")
                && line.contains("#choose(")));
    }

    @Test
    void spring_pass_composes_routes_and_resolves_unique_constructor_candidates() throws Exception {
        Path sourceRoot = Files.createDirectories(root.resolve("spring/src/main/java"));
        write(sourceRoot, "org/springframework/stereotype/Service.java",
                "package org.springframework.stereotype; public @interface Service { String value() default \"\"; }");
        write(sourceRoot, "org/springframework/web/bind/annotation/RestController.java",
                "package org.springframework.web.bind.annotation; public @interface RestController { String value() default \"\"; }");
        write(sourceRoot, "org/springframework/web/bind/annotation/RequestMapping.java",
                "package org.springframework.web.bind.annotation; public @interface RequestMapping { String[] value() default {}; }");
        write(sourceRoot, "org/springframework/web/bind/annotation/GetMapping.java",
                "package org.springframework.web.bind.annotation; public @interface GetMapping { String[] value() default {}; }");
        write(sourceRoot, "sample/App.java", """
                package sample;
                import org.springframework.stereotype.Service;
                import org.springframework.web.bind.annotation.*;
                @Service class GreetingService { String get() { return "hi"; } }
                @RestController @RequestMapping("/api")
                class GreetingController {
                    private final GreetingService service;
                    GreetingController(GreetingService service) { this.service = service; }
                    @GetMapping("/greeting") String get() { return service.get(); }
                }
                """);
        Path project = root.resolve("spring");
        ProjectModel model = new ProjectModel(project, List.of(project),
                List.of(new SourceRoot(sourceRoot, "main", "spring", project)), List.of(), "17", "test", 0);
        AnalysisRequest request = new AnalysisRequest(2, "spring", "analyze_project", project, AnalyzerMode.safe,
                List.of("main"), new AnalyzerOptions(true, true, false, List.of()), Path.of("mvn"), 10_000);
        StringWriter output = new StringWriter();

        AnalysisResult result = new JavaProjectAnalyzer().analyze(model, request,
                new ProtocolWriter(new ObjectMapper(), output));

        assertEquals(Boolean.TRUE, result.summary().capabilities().get("spring_static_semantics"));
        assertTrue(output.toString().contains("route:GET:/api/greeting"));
        assertTrue(output.toString().contains("bean:greetingService"));
        assertTrue(output.toString().contains("\"kind\":\"SELECTED_BEAN\""));
        assertFalse(output.toString().contains("RUNTIME_CONFIRMED"));
    }

    @Test
    void recovered_bean_factory_bindings_share_core_ids_and_leave_no_dangling_edges() throws Exception {
        Path sourceRoot = Files.createDirectories(root.resolve("recovered-spring/src/main/java"));
        write(sourceRoot, "org/springframework/context/annotation/Configuration.java", """
                package org.springframework.context.annotation;
                public @interface Configuration {}
                """);
        write(sourceRoot, "org/springframework/context/annotation/Bean.java", """
                package org.springframework.context.annotation;
                public @interface Bean { String[] value() default {}; String[] name() default {}; }
                """);
        write(sourceRoot, "org/springframework/context/event/EventListener.java", """
                package org.springframework.context.event;
                public @interface EventListener {}
                """);
        write(sourceRoot, "sample/StorageConfiguration.java", """
                package sample;
                import org.springframework.context.annotation.Bean;
                import org.springframework.context.annotation.Configuration;
                import org.springframework.context.event.EventListener;
                import org.springframework.web.socket.messaging.SessionConnectedEvent;
                @Configuration
                class StorageConfiguration {
                    Object callback = new Object() {
                        void accept(MissingCallbackType value) {}
                    };
                    @Bean S3Client s3Client(StorageProperties properties) { return null; }
                    @Bean S3Presigner s3Presigner(S3Client client) { return null; }
                    @EventListener
                    void connected(SessionConnectedEvent event) {}
                }
                """);
        Path project = root.resolve("recovered-spring");
        ProjectModel model = new ProjectModel(project, List.of(project),
                List.of(new SourceRoot(sourceRoot, "main", "recovered-spring", project)),
                List.of(), "17", "test", 0);
        AnalysisRequest request = new AnalysisRequest(2, "recovered-spring", "analyze_project", project,
                AnalyzerMode.safe, List.of("main"), new AnalyzerOptions(true, true, false, List.of()),
                Path.of("mvn"), 10_000);
        ObjectMapper mapper = new ObjectMapper();
        StringWriter output = new StringWriter();

        AnalysisResult result = new JavaProjectAnalyzer().analyze(model, request,
                new ProtocolWriter(mapper, output));
        List<JsonNode> events = output.toString().lines().map(line -> {
            try { return mapper.readTree(line); }
            catch (Exception error) { throw new IllegalStateException(error); }
        }).toList();
        Set<String> nodes = events.stream().map(event -> event.path("stable_id").asText())
                .filter(id -> !id.isBlank()).collect(Collectors.toSet());
        List<JsonNode> edges = events.stream().filter(event -> !event.path("source").asText().isBlank()).toList();
        String clientFactory = "method:sample.StorageConfiguration#s3Client(StorageProperties)";
        String presignerFactory = "method:sample.StorageConfiguration#s3Presigner(S3Client)";
        String listener = "method:sample.StorageConfiguration#connected(SessionConnectedEvent)";
        String externalEvent = "type:org.springframework.web.socket.messaging.SessionConnectedEvent";

        assertEquals("complete", result.status());
        assertTrue(edges.stream().allMatch(edge -> nodes.contains(edge.path("source").asText())),
                "every edge source must be emitted as a node");
        assertTrue(edges.stream().allMatch(edge -> nodes.contains(edge.path("target").asText())),
                "every edge target must be emitted as a node");
        assertFalse(nodes.stream().anyMatch(id -> id.startsWith("method:#") || id.startsWith("constructor:#")));
        assertTrue(hasNode(events, clientFactory));
        assertTrue(hasNode(events, presignerFactory));
        assertTrue(hasEdge(events, clientFactory, "bean:s3Client", "DECLARES_BEAN"));
        assertTrue(hasEdge(events, presignerFactory, "bean:s3Presigner", "DECLARES_BEAN"));
        assertTrue(hasEdge(events, presignerFactory, "bean:s3Client", "BEAN_CANDIDATE"));
        assertTrue(hasEdge(events, presignerFactory, "bean:s3Client", "SELECTED_BEAN"));
        assertTrue(hasEdge(events, "bean:s3Presigner", "bean:s3Client", "INJECTS"));
        assertTrue(hasNode(events, externalEvent), "missing external event node from " + nodes);
        assertTrue(hasEdge(events, listener, externalEvent, "LISTENS_TO_EVENT"),
                "missing listener edge from " + edges);
    }

    private static void write(Path root, String relative, String contents) throws Exception {
        Path file = root.resolve(relative);
        Files.createDirectories(file.getParent());
        Files.writeString(file, contents + "\n");
    }

    private static boolean hasNode(List<JsonNode> events, String id) {
        return events.stream().anyMatch(event -> id.equals(event.path("stable_id").asText()));
    }

    private static boolean hasEdge(List<JsonNode> events, String source, String target, String kind) {
        return events.stream().anyMatch(event -> source.equals(event.path("source").asText())
                && target.equals(event.path("target").asText()) && kind.equals(event.path("kind").asText()));
    }

    private static boolean hasKind(List<JsonNode> events, String kind) {
        return events.stream().anyMatch(event -> kind.equals(event.path("kind").asText()));
    }
}
