package dev.graphine.analyzer.java;

import java.util.Arrays;
import java.util.stream.Collectors;
import org.eclipse.jdt.core.dom.IMethodBinding;
import org.eclipse.jdt.core.dom.ITypeBinding;

public final class SymbolIds {
    private SymbolIds() {}

    public static String type(ITypeBinding binding) {
        return "type:" + normalizeType(binding);
    }

    public static String method(IMethodBinding binding) {
        IMethodBinding declaration = binding.getMethodDeclaration();
        String owner = normalizeType(declaration.getDeclaringClass());
        String parameters = Arrays.stream(declaration.getParameterTypes())
                .map(SymbolIds::normalizeType).collect(Collectors.joining(","));
        String prefix = declaration.isConstructor() ? "constructor:" : "method:";
        String name = declaration.isConstructor() ? "<init>" : declaration.getName();
        return prefix + owner + "#" + name + "(" + parameters + ")";
    }

    public static String field(String owner, String name) {
        return "field:" + owner + "#" + name;
    }

    public static String normalizeType(ITypeBinding binding) {
        if (binding == null) return "<unresolved>";
        if (binding.isArray()) return normalizeType(binding.getElementType()) + "[]".repeat(binding.getDimensions());
        ITypeBinding normalized = binding.isPrimitive() ? binding : binding.getErasure();
        String name = normalized.getQualifiedName();
        if (name == null || name.isBlank()) name = normalized.getName();
        return name.replace('$', '.');
    }

    public static String fallbackMethod(String owner, String name, java.util.List<String> parameters,
                                        boolean constructor) {
        String normalized = parameters.stream().map(SymbolIds::normalizeTextType)
                .collect(Collectors.joining(","));
        return (constructor ? "constructor:" : "method:") + owner + "#"
                + (constructor ? "<init>" : name) + "(" + normalized + ")";
    }

    static String normalizeTextType(String value) {
        return value.replace("...", "[]").replaceAll("\\s+", "").replace('$', '.');
    }
}
