package dev.graphine.analyzer.java;

import dev.graphine.analyzer.protocol.Diagnostic;
import dev.graphine.analyzer.protocol.GraphEdge;
import dev.graphine.analyzer.protocol.GraphNode;
import dev.graphine.analyzer.protocol.GraphSink;
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

final class GraphCollector implements GraphSink {
    private final Map<String, GraphNode> nodes = new LinkedHashMap<>();
    private final Map<String, GraphEdge> edges = new LinkedHashMap<>();
    private final List<Diagnostic> diagnostics = new ArrayList<>();
    long bindingsResolved;
    long bindingsUnresolved;

    @Override public void node(GraphNode node) {
        nodes.merge(node.stableId(), node, GraphCollector::mergeNode);
    }

    @Override public void edge(GraphEdge edge) {
        String key = edge.source() + '\0' + edge.target() + '\0' + edge.kind();
        edges.merge(key, edge, (existing, occurrence) -> {
            List<dev.graphine.analyzer.protocol.EdgeOccurrence> merged = new ArrayList<>(existing.occurrences());
            merged.addAll(occurrence.occurrences());
            return new GraphEdge(existing.source(), existing.target(), existing.kind(), existing.confidence(),
                    existing.provenance(), existing.metadata(), merged);
        });
    }

    @Override public void diagnostic(Diagnostic diagnostic) {
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

    private static GraphNode mergeNode(GraphNode existing, GraphNode replacement) {
        Map<String, Object> metadata = new LinkedHashMap<>(existing.metadata());
        metadata.putAll(replacement.metadata());
        Set<String> unresolved = new LinkedHashSet<>(existing.unresolved());
        unresolved.addAll(replacement.unresolved());
        boolean framework = replacement.provenance().startsWith("spring-static");
        return new GraphNode(existing.stableId(), framework ? replacement.kind() : existing.kind(),
                existing.qualifiedName(), existing.simpleName(), existing.moduleName(), existing.packageName(),
                existing.filePath() != null ? existing.filePath() : replacement.filePath(),
                existing.startLine() != null ? existing.startLine() : replacement.startLine(),
                existing.endLine() != null ? existing.endLine() : replacement.endLine(),
                framework ? replacement.confidence() : existing.confidence(),
                framework ? existing.provenance() + "+" + replacement.provenance() : existing.provenance(),
                metadata, List.copyOf(unresolved));
    }
}
