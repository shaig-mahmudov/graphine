package dev.graphine.analyzer.protocol;

import java.util.Map;

public record GraphEdge(String source, String target, String kind, String confidence,
                        String provenance, Map<String, Object> metadata) {
    public GraphEdge {
        metadata = metadata == null ? Map.of() : Map.copyOf(metadata);
    }
}
