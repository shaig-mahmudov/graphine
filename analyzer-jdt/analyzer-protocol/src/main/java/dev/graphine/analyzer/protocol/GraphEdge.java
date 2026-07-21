package dev.graphine.analyzer.protocol;

import java.util.Map;
import java.util.List;

public record GraphEdge(String source, String target, String kind, String confidence,
                        String provenance, Map<String, Object> metadata, List<EdgeOccurrence> occurrences) {
    public GraphEdge {
        metadata = metadata == null ? Map.of() : Map.copyOf(metadata);
        occurrences = occurrences == null ? List.of() : List.copyOf(occurrences);
    }

    public GraphEdge(String source, String target, String kind, String confidence,
                     String provenance, Map<String, Object> metadata) {
        this(source, target, kind, confidence, provenance, metadata, List.of());
    }
}
