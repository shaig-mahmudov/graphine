package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;
import java.util.Map;

public record GraphNode(
        @JsonProperty("stable_id") String stableId,
        String kind,
        @JsonProperty("qualified_name") String qualifiedName,
        @JsonProperty("simple_name") String simpleName,
        @JsonProperty("module_name") String moduleName,
        @JsonProperty("package_name") String packageName,
        @JsonProperty("file_path") String filePath,
        @JsonProperty("start_line") Integer startLine,
        @JsonProperty("end_line") Integer endLine,
        String confidence,
        String provenance,
        Map<String, Object> metadata,
        List<String> unresolved) {
    public GraphNode {
        metadata = metadata == null ? Map.of() : Map.copyOf(metadata);
        unresolved = unresolved == null ? List.of() : List.copyOf(unresolved);
    }
}
