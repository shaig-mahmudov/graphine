package dev.graphine.analyzer.protocol;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.IOException;
import java.io.Writer;

public final class ProtocolWriter {
    private final ObjectMapper mapper;
    private final Writer output;

    public ProtocolWriter(ObjectMapper mapper, Writer output) {
        this.mapper = mapper;
        this.output = output;
    }

    public synchronized void emit(String type, Object value) throws IOException {
        ObjectNode event = mapper.valueToTree(value);
        event.put("type", type);
        output.write(mapper.writeValueAsString(event));
        output.write('\n');
        output.flush();
    }

    public synchronized void emit(String type, String key, Object value) throws IOException {
        ObjectNode event = mapper.createObjectNode();
        event.put("type", type);
        event.set(key, mapper.valueToTree(value));
        output.write(mapper.writeValueAsString(event));
        output.write('\n');
        output.flush();
    }
}
