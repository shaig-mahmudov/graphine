package dev.graphine.analyzer.protocol;

/** Language-neutral destination for normalized analyzer facts. */
public interface GraphSink {
    void node(GraphNode node);
    void edge(GraphEdge edge);
    void diagnostic(Diagnostic diagnostic);
}
