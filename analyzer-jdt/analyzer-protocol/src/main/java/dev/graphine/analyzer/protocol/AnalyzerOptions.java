package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

public record AnalyzerOptions(
        @JsonProperty("include_method_bodies") boolean includeMethodBodies,
        @JsonProperty("include_field_access") boolean includeFieldAccess,
        @JsonProperty("include_tests") boolean includeTests,
        @JsonProperty("explicit_classpath") List<String> explicitClasspath) {
    public AnalyzerOptions {
        explicitClasspath = explicitClasspath == null ? List.of() : List.copyOf(explicitClasspath);
    }
}
