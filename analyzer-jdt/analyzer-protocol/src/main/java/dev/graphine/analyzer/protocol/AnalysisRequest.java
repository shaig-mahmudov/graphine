package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;

public record AnalysisRequest(
        @JsonProperty("protocol_version") int protocolVersion,
        @JsonProperty("request_id") String requestId,
        String operation,
        String language,
        @JsonProperty("project_root") Path projectRoot,
        AnalyzerMode mode,
        @JsonProperty("source_sets") List<String> sourceSets,
        AnalyzerOptions options,
        @JsonProperty("maven_executable") Path mavenExecutable,
        @JsonProperty("cargo_executable") Path cargoExecutable,
        @JsonProperty("rustc_executable") Path rustcExecutable,
        Map<String, Object> cargo,
        @JsonProperty("timeout_ms") long timeoutMs) {
    public AnalysisRequest(int protocolVersion, String requestId, String operation, Path projectRoot,
            AnalyzerMode mode, List<String> sourceSets, AnalyzerOptions options,
            Path mavenExecutable, long timeoutMs) {
        this(protocolVersion, requestId, operation, "java", projectRoot, mode, sourceSets, options,
                mavenExecutable, Path.of("cargo"), Path.of("rustc"), Map.of(), timeoutMs);
    }

    public void validate() {
        if (protocolVersion != 2 || !"analyze_project".equals(operation) || requestId == null
                || requestId.isBlank() || projectRoot == null || mode == null || options == null
                || !"java".equals(language) || timeoutMs <= 0) {
            throw new IllegalArgumentException("invalid analyzer request");
        }
    }
}
