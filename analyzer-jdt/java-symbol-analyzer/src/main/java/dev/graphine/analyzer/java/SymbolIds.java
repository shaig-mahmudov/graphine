package dev.graphine.analyzer.java;

import dev.graphine.analyzer.jdt.CallableIds;
import org.eclipse.jdt.core.dom.ITypeBinding;

public final class SymbolIds {
    private SymbolIds() {}

    public static String type(ITypeBinding binding) {
        return "type:" + normalizeType(binding);
    }

    public static String method(CallableIds.ResolvedMethod method) {
        return CallableIds.id(method);
    }

    public static String field(String owner, String name) {
        return "field:" + owner + "#" + name;
    }

    public static String normalizeType(ITypeBinding binding) {
        return CallableIds.normalizeType(binding);
    }

    public static String fallbackMethod(String owner, String name, java.util.List<String> parameters,
                                        boolean constructor) {
        return CallableIds.fallback(owner, name, parameters, constructor);
    }
}
