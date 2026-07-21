package dev.graphine.analyzer.java;

import static org.junit.jupiter.api.Assertions.assertEquals;

import com.fasterxml.jackson.databind.ObjectMapper;
import dev.graphine.analyzer.protocol.GraphNode;
import dev.graphine.analyzer.protocol.GraphEdge;
import dev.graphine.analyzer.protocol.EdgeOccurrence;
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

    @Test
    void repeated_relationships_merge_occurrences_without_duplicate_logical_edges() throws Exception {
        GraphCollector collector = new GraphCollector();
        for (int line : List.of(4, 8, 12)) {
            collector.edge(new GraphEdge("method:sample.A#run()", "method:sample.B#call()", "CALLS",
                    "COMPILER_RESOLVED", "test", Map.of(),
                    List.of(new EdgeOccurrence("src/main/java/sample/A.java", line, line, Map.of()))));
        }
        StringWriter output = new StringWriter();
        collector.emit(new ProtocolWriter(new ObjectMapper(), output));

        var edge = new ObjectMapper().readTree(output.toString().trim());
        assertEquals(1, collector.edgeCount());
        assertEquals(3, edge.path("occurrences").size());
        assertEquals(12, edge.path("occurrences").get(2).path("end_line").asInt());
    }
}
