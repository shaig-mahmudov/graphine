package dev.graphine.analyzer.java;

import dev.graphine.analyzer.jdt.CallableIds;
import org.eclipse.jdt.core.dom.ITypeBinding;

public final class SymbolIds {
    private SymbolIds() {}

    /**
     * Creates a normalized identifier for a type binding.
     *
     * @param binding the type binding to identify
     * @return the type identifier
     */
    public static String type(ITypeBinding binding) {
        return "type:" + normalizeType(binding);
    }

    /**
     * Creates an identifier for a resolved method.
     *
     * @param method the resolved method to identify
     * @return the method identifier
     */
    public static String method(CallableIds.ResolvedMethod method) {
        return CallableIds.id(method);
    }

    /**
     * Creates a normalized identifier for a field.
     *
     * @param owner the identifier of the field's declaring type
     * @param name  the field name
     * @return      the field identifier
     */
    public static String field(String owner, String name) {
        return "field:" + owner + "#" + name;
    }

    /**
     * Normalizes a type binding into its identifier representation.
     *
     * @param binding the type binding to normalize
     * @return the normalized type identifier
     */
    public static String normalizeType(ITypeBinding binding) {
        return CallableIds.normalizeType(binding);
    }

    /**
     * Generates a fallback identifier for a method or constructor.
     *
     * @param owner       the identifier of the declaring type
     * @param name        the method or constructor name
     * @param parameters  the parameter type identifiers
     * @param constructor whether the symbol represents a constructor
     * @return the generated fallback method identifier
     */
    public static String fallbackMethod(String owner, String name, java.util.List<String> parameters,
                                        boolean constructor) {
        return CallableIds.fallback(owner, name, parameters, constructor);
    }
}
