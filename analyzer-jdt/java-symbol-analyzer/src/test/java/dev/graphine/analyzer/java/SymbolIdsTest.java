package dev.graphine.analyzer.java;

import static org.junit.jupiter.api.Assertions.*;
import java.util.List;
import org.junit.jupiter.api.Test;

class SymbolIdsTest {
    @Test void fallbackIdsDistinguishOverloadsConstructorsArraysAndVarargs() {
        assertNotEquals(
                SymbolIds.fallbackMethod("example.Type", "call", List.of("String"), false),
                SymbolIds.fallbackMethod("example.Type", "call", List.of("int"), false));
        assertEquals("method:example.Type#call(java.lang.String[])",
                SymbolIds.fallbackMethod("example.Type", "call", List.of("java.lang.String..."), false));
        assertEquals("constructor:example.Type#<init>(example.Dependency)",
                SymbolIds.fallbackMethod("example.Type", "Type", List.of("example.Dependency"), true));
    }
}
