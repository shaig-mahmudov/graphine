package dev.graphine.analyzer.cli;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.ObjectMapper;
import dev.graphine.analyzer.java.AnalysisResult;
import dev.graphine.analyzer.java.JavaProjectAnalyzer;
import dev.graphine.analyzer.protocol.AnalyzerCapabilities;
import dev.graphine.analyzer.protocol.AnalysisRequest;
import dev.graphine.analyzer.protocol.AnalysisSummary;
import dev.graphine.analyzer.protocol.Diagnostic;
import dev.graphine.analyzer.protocol.ProtocolWriter;
import dev.graphine.analyzer.resolver.MavenProjectResolver;
import dev.graphine.analyzer.resolver.ProjectModel;
import dev.graphine.analyzer.resolver.SourceRoot;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class AnalyzerMain {
    public static final String VERSION = "0.1.0";
    private static final int PROTOCOL_VERSION = 1;
    private static final int REQUEST_LIMIT = 1_048_576;

    private AnalyzerMain() {}

    public static void main(String[] args) throws Exception {
        ObjectMapper mapper = new ObjectMapper().findAndRegisterModules()
                .enable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES);
        if (args.length == 1 && "--version".equals(args[0])) {
            System.out.println("graphine-analyzer " + VERSION + " protocol " + PROTOCOL_VERSION + " eclipse-jdt");
            return;
        }
        if (args.length == 1 && "--metadata".equals(args[0])) {
            mapper.writeValue(System.out, metadata());
            return;
        }
        ProtocolWriter writer = new ProtocolWriter(mapper,
                new OutputStreamWriter(System.out, StandardCharsets.UTF_8));
        try {
            String line = readBoundedLine();
            AnalysisRequest request = mapper.readValue(line, AnalysisRequest.class);
            request.validate();
            writer.emit("analysis_started", Map.of("protocol_version", PROTOCOL_VERSION,
                    "request_id", request.requestId(), "analyzer_version", VERSION));
            List<Diagnostic> resolverDiagnostics = new ArrayList<>();
            ProjectModel model = new MavenProjectResolver().resolve(request, resolverDiagnostics::add);
            writer.emit("project_metadata", Map.of(
                    "fingerprint", model.fingerprint(),
                    "java_release", model.javaRelease(),
                    "classpath_resolution_ms", model.classpathResolutionMs(),
                    "capabilities", packagedCapabilities(),
                    "modules", model.moduleRoots().stream().map(path -> path.getFileName().toString()).toList()));
            for (var entry : model.sourceRoots().stream().collect(java.util.stream.Collectors.groupingBy(SourceRoot::moduleName)).entrySet()) {
                SourceRoot first = entry.getValue().get(0);
                String moduleRoot = model.root().relativize(first.moduleRoot()).toString().replace('\\', '/');
                writer.emit("module_discovered", Map.of(
                        "name", entry.getKey(),
                        "root", moduleRoot.isEmpty() ? "." : moduleRoot,
                        "source_roots", entry.getValue().stream().map(root -> model.root().relativize(root.path()).toString().replace('\\', '/')).toList()));
            }
            AnalysisResult result = new JavaProjectAnalyzer().analyze(model, request, writer);
            for (Diagnostic diagnostic : resolverDiagnostics) writer.emit("diagnostic", "diagnostic", diagnostic);
            AnalysisSummary original = result.summary();
            AnalysisSummary summary = new AnalysisSummary(original.filesDiscovered(), original.filesParsed(),
                    original.filesFailed(), original.bindingsResolved(),
                    original.bindingsUnresolved() + resolverDiagnostics.size(), original.nodesEmitted(),
                    original.edgesEmitted(), original.durationMs(), original.classpathResolutionMs(),
                    original.parsingMs(), original.springSemanticMs(), original.serializationMs(),
                    original.peakJavaMemoryBytes(), original.capabilities(), result.status());
            writer.emit("analysis_summary", "summary", summary);
            writer.emit("analysis_completed", Map.of("status", result.status()));
        } catch (Exception error) {
            error.printStackTrace(System.err);
            String message = error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage();
            writer.emit("analysis_failed", Map.of("code", "analysis_failed",
                    "message", message.substring(0, Math.min(500, message.length()))));
            System.exit(2);
        }
    }

    static Map<String, Object> metadata() {
        List<String> capabilities = packagedCapabilities().entrySet().stream()
                .filter(Map.Entry::getValue)
                .map(Map.Entry::getKey)
                .toList();
        return Map.of(
                "protocol_version", PROTOCOL_VERSION,
                "analyzer_version", VERSION,
                "capabilities", capabilities);
    }

    private static Map<String, Boolean> packagedCapabilities() {
        Map<String, Boolean> capabilities = new LinkedHashMap<>();
        capabilities.put(AnalyzerCapabilities.JAVA_SEMANTICS,
                classAvailable("dev.graphine.analyzer.java.JavaProjectAnalyzer"));
        capabilities.put(AnalyzerCapabilities.SPRING_STATIC_SEMANTICS,
                classAvailable("dev.graphine.analyzer.spring.SpringSemanticAnalyzer"));
        capabilities.put(AnalyzerCapabilities.SPRING_RUNTIME_SEMANTICS, false);
        capabilities.put(AnalyzerCapabilities.MAVEN_TRUSTED_MODE,
                classAvailable("dev.graphine.analyzer.resolver.MavenProjectResolver"));
        return capabilities;
    }

    private static boolean classAvailable(String name) {
        try {
            Class.forName(name, false, AnalyzerMain.class.getClassLoader());
            return true;
        } catch (ClassNotFoundException | LinkageError error) {
            return false;
        }
    }

    private static String readBoundedLine() throws Exception {
        BufferedReader reader = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        StringBuilder value = new StringBuilder();
        for (int character; (character = reader.read()) >= 0 && character != '\n';) {
            if (value.length() >= REQUEST_LIMIT) throw new IllegalArgumentException("request exceeds limit");
            value.append((char) character);
        }
        if (value.isEmpty()) throw new IllegalArgumentException("missing request");
        return value.toString();
    }
}
