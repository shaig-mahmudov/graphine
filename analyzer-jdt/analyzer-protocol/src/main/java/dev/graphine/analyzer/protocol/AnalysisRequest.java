package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.nio.file.Path;
import java.util.List;

public record AnalysisRequest(
        @JsonProperty("protocol_version") int protocolVersion,
        @JsonProperty("request_id") String requestId,
        String operation,
        @JsonProperty("project_root") Path projectRoot,
        AnalyzerMode mode,
        @JsonProperty("source_sets") List<String> sourceSets,
        AnalyzerOptions options,
        @JsonProperty("maven_executable") Path mavenExecutable,
        @JsonProperty("timeout_ms") long timeoutMs) {
    public void validate() {
        if (protocolVersion != 1 || !"analyze_project".equals(operation) || requestId == null
                || requestId.isBlank() || projectRoot == null || mode == null || options == null
                || timeoutMs <= 0) {
            throw new IllegalArgumentException("invalid analyzer request");
        }
    }
}
