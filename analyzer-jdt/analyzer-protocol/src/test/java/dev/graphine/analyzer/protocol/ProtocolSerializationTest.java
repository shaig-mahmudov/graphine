package dev.graphine.analyzer.protocol;

import static org.junit.jupiter.api.Assertions.*;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.nio.file.Path;
import java.util.List;
import org.junit.jupiter.api.Test;

class ProtocolSerializationTest {
    @Test void requestUsesVersionedSnakeCaseFields() throws Exception {
        ObjectMapper mapper = new ObjectMapper();
        mapper.findAndRegisterModules();
        AnalysisRequest request = new AnalysisRequest(1, "index-1", "analyze_project", Path.of("project"),
                AnalyzerMode.safe, List.of("main"), new AnalyzerOptions(true, true, false, List.of()),
                Path.of("mvn"), 1000);
        String json = mapper.writeValueAsString(request);
        assertTrue(json.contains("\"protocol_version\":1"));
        AnalysisRequest decoded = mapper.readValue(json, AnalysisRequest.class);
        assertEquals(request.protocolVersion(), decoded.protocolVersion());
        assertEquals(request.requestId(), decoded.requestId());
        assertEquals(request.mode(), decoded.mode());
        assertTrue(decoded.projectRoot().toString().endsWith("project"));
    }
}
