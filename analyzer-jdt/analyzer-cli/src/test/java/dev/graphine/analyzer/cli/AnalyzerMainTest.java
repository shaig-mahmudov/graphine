package dev.graphine.analyzer.cli;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

class AnalyzerMainTest {
    @Test
    void metadataDescribesThePackagedAnalyzerWithoutStartingAnalysis() {
        Map<String, Object> metadata = AnalyzerMain.metadata();
        assertEquals(2, metadata.get("protocol_version"));
        assertEquals("graphine-java-jdt", metadata.get("analyzer_name"));
        assertEquals("java", metadata.get("language"));
        assertEquals(AnalyzerMain.VERSION, metadata.get("analyzer_version"));

        @SuppressWarnings("unchecked")
        List<String> capabilities = (List<String>) metadata.get("capabilities");
        assertTrue(capabilities.contains("java_semantics"));
        assertTrue(capabilities.contains("spring_static_semantics"));
        assertTrue(capabilities.contains("maven_trusted_mode"));
        assertFalse(capabilities.contains("spring_runtime_semantics"));
    }
}
