package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;

public record Diagnostic(
        String kind,
        @JsonProperty("file_path") String filePath,
        @JsonProperty("start_line") Integer startLine,
        @JsonProperty("end_line") Integer endLine,
        @JsonProperty("symbol_text") String symbolText,
        String reason,
        String severity) {}
