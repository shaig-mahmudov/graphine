package dev.graphine.analyzer.jdt;

import java.util.Arrays;
import java.util.List;
import java.util.stream.Collectors;
import org.eclipse.jdt.core.dom.IMethodBinding;
import org.eclipse.jdt.core.dom.ITypeBinding;
import org.eclipse.jdt.core.dom.MethodDeclaration;
import org.eclipse.jdt.core.dom.SingleVariableDeclaration;

/** Shared stable-ID resolution for JDT method and constructor bindings. */
public final class CallableIds {
    private CallableIds() {}

    public static CallableSymbol forDeclaration(IMethodBinding binding, String lexicalOwner,
                                                MethodDeclaration declaration) {
        return resolveOrFallback(binding, lexicalOwner, declaration.getName().getIdentifier(),
                astParameterTypes(declaration), declaration.isConstructor());
    }

    public static CallableSymbol resolveOrFallback(IMethodBinding binding, String lexicalOwner, String lexicalName,
                                                   List<String> lexicalParameters, boolean constructor) {
        ResolvedMethod resolved = resolve(binding);
        if (resolved != null) return new CallableSymbol(id(resolved), resolved.parameterTypes(), resolved);
        List<String> parameters = lexicalParameters.stream().map(CallableIds::normalizeTextType).toList();
        return new CallableSymbol(fallback(lexicalOwner, lexicalName, parameters, constructor), parameters, null);
    }

    public static ResolvedMethod resolve(IMethodBinding binding) {
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
                Arrays.stream(parameters).map(CallableIds::normalizeType).toList(), declaration.isConstructor());
    }

    public static String id(ResolvedMethod method) {
        String prefix = method.constructor() ? "constructor:" : "method:";
        String name = method.constructor() ? "<init>" : method.name();
        return prefix + method.ownerName() + "#" + name + "(" + String.join(",", method.parameterTypes()) + ")";
    }

    public static String fallback(String owner, String name, List<String> parameters, boolean constructor) {
        String normalized = parameters.stream().map(CallableIds::normalizeTextType)
                .collect(Collectors.joining(","));
        return (constructor ? "constructor:" : "method:") + owner + "#"
                + (constructor ? "<init>" : name) + "(" + normalized + ")";
    }

    public static List<String> astParameterTypes(MethodDeclaration declaration) {
        return declaration.parameters().stream()
                .map(value -> astParameterType((SingleVariableDeclaration) value)).toList();
    }

    public static boolean isUsableType(ITypeBinding binding) {
        if (binding == null || binding.isRecovered()) return false;
        if (binding.isArray()) return binding.getDimensions() > 0 && isUsableType(binding.getElementType());
        ITypeBinding normalized = binding.isPrimitive() ? binding : binding.getErasure();
        if (normalized == null || normalized.isRecovered()) return false;
        String name = normalized.getQualifiedName();
        if (name == null || name.isBlank()) name = normalized.getName();
        return name != null && !name.isBlank() && !"<unresolved>".equals(name);
    }

    public static String normalizeType(ITypeBinding binding) {
        if (binding == null) return "<unresolved>";
        if (binding.isArray())
            return normalizeType(binding.getElementType()) + "[]".repeat(binding.getDimensions());
        ITypeBinding normalized = binding.isPrimitive() ? binding : binding.getErasure();
        if (normalized == null) return "<unresolved>";
        String name = normalized.getQualifiedName();
        if (name == null || name.isBlank()) name = normalized.getName();
        return name == null || name.isBlank() ? "<unresolved>" : name.replace('$', '.');
    }

    public static String normalizeTextType(String value) {
        return value.replace("...", "[]").replaceAll("\\s+", "").replace('$', '.');
    }

    private static String astParameterType(SingleVariableDeclaration parameter) {
        return normalizeTextType(parameter.getType() + "[]".repeat(parameter.getExtraDimensions())
                + (parameter.isVarargs() ? "[]" : ""));
    }

    public record ResolvedMethod(IMethodBinding declaration, ITypeBinding owner, String ownerName, String name,
                                 List<ITypeBinding> parameterBindings, List<String> parameterTypes,
                                 boolean constructor) {}

    public record CallableSymbol(String id, List<String> parameterTypes, ResolvedMethod resolved) {}
}
