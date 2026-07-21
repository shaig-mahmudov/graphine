package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.Map;

public record EdgeOccurrence(
        @JsonProperty("file_path") String filePath,
        @JsonProperty("start_line") int startLine,
        @JsonProperty("end_line") int endLine,
        Map<String, Object> metadata) {
    public EdgeOccurrence {
        if (filePath == null || filePath.isBlank() || startLine <= 0 || endLine < startLine) {
            throw new IllegalArgumentException("invalid edge occurrence");
        }
        metadata = metadata == null ? Map.of() : Map.copyOf(metadata);
    }
}
