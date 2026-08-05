package dev.graphine.analyzer.spring;

import static org.junit.jupiter.api.Assertions.assertFalse;

import org.junit.jupiter.api.Test;

final class SpringSemanticAnalyzerTest {
    @Test
    void incomplete_container_types_are_not_treated_as_injection_containers() {
        assertFalse(SpringSemanticAnalyzer.isContainer(null));
        assertFalse(SpringSemanticAnalyzer.isCollection(null));
        assertFalse(SpringSemanticAnalyzer.isOptional(null));
    }
}
