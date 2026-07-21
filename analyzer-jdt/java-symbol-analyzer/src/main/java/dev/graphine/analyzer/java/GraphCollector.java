package dev.graphine.analyzer.java;

import dev.graphine.analyzer.protocol.Diagnostic;
import dev.graphine.analyzer.protocol.GraphEdge;
import dev.graphine.analyzer.protocol.GraphNode;
import dev.graphine.analyzer.protocol.ProtocolWriter;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import org.eclipse.jdt.core.dom.IMethodBinding;
import org.eclipse.jdt.core.dom.ITypeBinding;

final class GraphCollector {
    private final Map<String, GraphNode> nodes = new LinkedHashMap<>();
    private final Map<String, GraphEdge> edges = new LinkedHashMap<>();
    private final List<Diagnostic> diagnostics = new ArrayList<>();
    long bindingsResolved;
    long bindingsUnresolved;

    void node(GraphNode node) {
        nodes.merge(node.stableId(), node, (existing, replacement) ->
                existing.filePath() == null && replacement.filePath() != null ? replacement : existing);
    }

    void edge(GraphEdge edge) {
        String key = edge.source() + '\0' + edge.target() + '\0' + edge.kind();
        edges.putIfAbsent(key, edge);
    }

    void diagnostic(Diagnostic diagnostic) {
        diagnostics.add(diagnostic);
        bindingsUnresolved++;
    }

    String externalType(ITypeBinding binding) {
        if (binding == null) return null;
        String id = SymbolIds.type(binding);
        String qualified = SymbolIds.normalizeType(binding);
        String kind = binding.isAnnotation() ? "ANNOTATION_TYPE" : "TYPE";
        node(new GraphNode(id, kind, qualified, binding.getName(), null,
                binding.getPackage() == null ? null : binding.getPackage().getName(), null, null, null,
                "COMPILER_RESOLVED", "eclipse-jdt-binding",
                Map.of("external", true, "type_kind", typeKind(binding)), List.of()));
        return id;
    }

    String externalMethod(IMethodBinding binding) {
        if (binding == null) return null;
        String owner = externalType(binding.getDeclaringClass());
        String id = SymbolIds.method(binding);
        node(new GraphNode(id, binding.isConstructor() ? "CONSTRUCTOR" : "METHOD",
                SymbolIds.normalizeType(binding.getDeclaringClass()) + "#" + binding.getName(),
                binding.isConstructor() ? "<init>" : binding.getName(), null,
                binding.getDeclaringClass().getPackage() == null ? null : binding.getDeclaringClass().getPackage().getName(),
                null, null, null, "COMPILER_RESOLVED", "eclipse-jdt-binding",
                Map.of("external", true, "declaring_type", owner), List.of()));
        return id;
    }

    void resolved() { bindingsResolved++; }

    List<Diagnostic> diagnostics() { return List.copyOf(diagnostics); }
    int nodeCount() { return nodes.size(); }
    int edgeCount() { return edges.size(); }

    void emit(ProtocolWriter writer) throws IOException {
        for (GraphNode node : nodes.values().stream().sorted(Comparator.comparing(GraphNode::stableId)).toList())
            writer.emit("node", node);
        for (GraphEdge edge : edges.values().stream().sorted(Comparator.comparing(GraphEdge::source)
                .thenComparing(GraphEdge::kind).thenComparing(GraphEdge::target)).toList())
            writer.emit("edge", edge);
        for (Diagnostic diagnostic : diagnostics) writer.emit("diagnostic", "diagnostic", diagnostic);
    }

    static String typeKind(ITypeBinding binding) {
        if (binding.isAnnotation()) return "annotation";
        if (binding.isEnum()) return "enum";
        if (binding.isRecord()) return "record";
        if (binding.isInterface()) return "interface";
        return "class";
    }
}
