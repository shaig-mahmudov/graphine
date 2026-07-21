package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.Map;

public record AnalysisSummary(
        @JsonProperty("files_discovered") long filesDiscovered,
        @JsonProperty("files_parsed") long filesParsed,
        @JsonProperty("files_failed") long filesFailed,
        @JsonProperty("bindings_resolved") long bindingsResolved,
        @JsonProperty("bindings_unresolved") long bindingsUnresolved,
        @JsonProperty("nodes_emitted") long nodesEmitted,
        @JsonProperty("edges_emitted") long edgesEmitted,
        @JsonProperty("duration_ms") long durationMs,
        @JsonProperty("classpath_resolution_ms") long classpathResolutionMs,
        @JsonProperty("parsing_ms") long parsingMs,
        @JsonProperty("spring_semantic_ms") long springSemanticMs,
        @JsonProperty("serialization_ms") long serializationMs,
        @JsonProperty("peak_java_memory_bytes") long peakJavaMemoryBytes,
        Map<String, Boolean> capabilities,
        String status) {}
