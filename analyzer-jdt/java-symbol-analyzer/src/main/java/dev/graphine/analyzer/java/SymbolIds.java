package dev.graphine.analyzer.java;

import java.util.Arrays;
import java.util.List;
import java.util.stream.Collectors;
import org.eclipse.jdt.core.dom.IMethodBinding;
import org.eclipse.jdt.core.dom.ITypeBinding;

public final class SymbolIds {
    private SymbolIds() {}

    public static String type(ITypeBinding binding) {
        return "type:" + normalizeType(binding);
    }

    public static String method(ResolvedMethod method) {
        String parameters = String.join(",", method.parameterTypes());
        String prefix = method.constructor() ? "constructor:" : "method:";
        String name = method.constructor() ? "<init>" : method.name();
        return prefix + method.ownerName() + "#" + name + "(" + parameters + ")";
    }

    static ResolvedMethod resolveMethod(IMethodBinding binding) {
        if (binding == null || binding.isRecovered()) return null;
        IMethodBinding declaration = binding.getMethodDeclaration();
        if (declaration == null || declaration.isRecovered()) return null;
        ITypeBinding owner = declaration.getDeclaringClass();
        String name = declaration.getName();
        if (!isUsableType(owner) || name == null || name.isBlank()) return null;
        ITypeBinding[] parameters = declaration.getParameterTypes();
        if (parameters == null || Arrays.stream(parameters).anyMatch(parameter -> !isUsableType(parameter)))
            return null;
        return new ResolvedMethod(declaration, owner, normalizeType(owner), name, List.of(parameters),
                Arrays.stream(parameters).map(SymbolIds::normalizeType).toList(), declaration.isConstructor());
    }

    static boolean isUsableType(ITypeBinding binding) {
        if (binding == null || binding.isRecovered()) return false;
        if (binding.isArray()) return binding.getDimensions() > 0 && isUsableType(binding.getElementType());
        ITypeBinding normalized = binding.isPrimitive() ? binding : binding.getErasure();
        if (normalized == null || normalized.isRecovered()) return false;
        String name = normalized.getQualifiedName();
        if (name == null || name.isBlank()) name = normalized.getName();
        return name != null && !name.isBlank() && !"<unresolved>".equals(name);
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

    record ResolvedMethod(IMethodBinding declaration, ITypeBinding owner, String ownerName, String name,
                          List<ITypeBinding> parameterBindings, List<String> parameterTypes,
                          boolean constructor) {}
}
