package dev.graphine.analyzer.java;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.fasterxml.jackson.databind.ObjectMapper;
import dev.graphine.analyzer.protocol.GraphNode;
import dev.graphine.analyzer.protocol.ProtocolWriter;
import java.io.StringWriter;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

final class GraphCollectorTest {
    @Test
    void source_declaration_replaces_an_earlier_external_placeholder() throws Exception {
        GraphCollector collector = new GraphCollector();
        collector.node(new GraphNode("type:sample.Target", "TYPE", "sample.Target", "Target", null,
                "sample", null, null, null, "COMPILER_RESOLVED", "eclipse-jdt-binding",
                Map.of("external", true), List.of()));
        collector.node(new GraphNode("type:sample.Target", "TYPE", "sample.Target", "Target", "fixture",
                "sample", "src/main/java/sample/Target.java", 3, 7, "COMPILER_RESOLVED", "eclipse-jdt",
                Map.of("source_set", "main"), List.of()));
        StringWriter output = new StringWriter();
        collector.emit(new ProtocolWriter(new ObjectMapper(), output));

        var node = new ObjectMapper().readTree(output.toString().trim());
        assertEquals("src/main/java/sample/Target.java", node.path("file_path").asText());
        assertEquals("main", node.path("metadata").path("source_set").asText());
    }
}
