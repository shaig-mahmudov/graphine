package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.Map;

public record AnalysisSummary(
        String language,
        @JsonProperty("files_discovered") long filesDiscovered,
        @JsonProperty("files_parsed") long filesParsed,
        @JsonProperty("files_failed") long filesFailed,
        @JsonProperty("bindings_resolved") long bindingsResolved,
        @JsonProperty("bindings_unresolved") long bindingsUnresolved,
        @JsonProperty("nodes_emitted") long nodesEmitted,
        @JsonProperty("edges_emitted") long edgesEmitted,
        @JsonProperty("duration_ms") long durationMs,
        Map<String, Boolean> capabilities,
        @JsonProperty("timings_ms") Map<String, Long> timingsMs,
        Map<String, Long> resources,
        Map<String, Object> configuration,
        String status) {}
