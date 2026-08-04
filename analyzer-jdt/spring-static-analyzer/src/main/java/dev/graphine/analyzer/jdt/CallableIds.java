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

    /**
     * Creates a stable callable symbol for a method or constructor declaration.
     *
     * @param binding      the resolved method binding, when available
     * @param lexicalOwner the lexical owner name used when resolution is unavailable
     * @param declaration  the method or constructor declaration
     * @return             the generated callable symbol
     */
    public static CallableSymbol forDeclaration(IMethodBinding binding, String lexicalOwner,
                                                MethodDeclaration declaration) {
        return resolveOrFallback(binding, lexicalOwner, declaration.getName().getIdentifier(),
                astParameterTypes(declaration), declaration.isConstructor());
    }

    /**
     * Resolves a callable binding and generates a stable identifier, or uses lexical information when resolution is unavailable.
     *
     * @param binding          the method or constructor binding to resolve
     * @param lexicalOwner     the lexical owner name used for fallback identification
     * @param lexicalName      the lexical callable name used for fallback identification
     * @param lexicalParameters the lexical parameter types used for fallback identification
     * @param constructor      whether the callable is a constructor
     * @return a callable symbol containing the generated identifier, normalized parameter types, and resolved method data when available
     */
    public static CallableSymbol resolveOrFallback(IMethodBinding binding, String lexicalOwner, String lexicalName,
                                                   List<String> lexicalParameters, boolean constructor) {
        ResolvedMethod resolved = resolve(binding);
        if (resolved != null) return new CallableSymbol(id(resolved), resolved.parameterTypes(), resolved);
        List<String> parameters = lexicalParameters.stream().map(CallableIds::normalizeTextType).toList();
        return new CallableSymbol(fallback(lexicalOwner, lexicalName, parameters, constructor), parameters, null);
    }

    /**
     * Resolves a usable method binding and its declaring type information.
     *
     * @param binding the method binding to resolve
     * @return the resolved method details, or {@code null} when required binding or type information is unavailable
     */
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

    /**
     * Builds a stable identifier for a resolved method or constructor.
     *
     * @param method the resolved callable whose identifier is generated
     * @return an identifier containing the callable kind, owner, name, and normalized parameter types
     */
    public static String id(ResolvedMethod method) {
        String prefix = method.constructor() ? "constructor:" : "method:";
        String name = method.constructor() ? "<init>" : method.name();
        return prefix + method.ownerName() + "#" + name + "(" + String.join(",", method.parameterTypes()) + ")";
    }

    /**
     * Builds a callable identifier from lexical owner, name, parameter, and constructor information.
     *
     * @param owner       the lexical qualified name of the declaring type
     * @param name        the callable name
     * @param parameters  the lexical parameter type names
     * @param constructor whether the callable is a constructor
     * @return the normalized callable identifier
     */
    public static String fallback(String owner, String name, List<String> parameters, boolean constructor) {
        String normalized = parameters.stream().map(CallableIds::normalizeTextType)
                .collect(Collectors.joining(","));
        return (constructor ? "constructor:" : "method:") + owner + "#"
                + (constructor ? "<init>" : name) + "(" + normalized + ")";
    }

    /**
     * Extracts normalized declared parameter types from a method declaration.
     *
     * @param declaration the method declaration whose parameter types are extracted
     * @return the normalized parameter type names
     */
    public static List<String> astParameterTypes(MethodDeclaration declaration) {
        return declaration.parameters().stream()
                .map(value -> astParameterType((SingleVariableDeclaration) value)).toList();
    }

    /**
     * Determines whether a type binding provides a resolved, usable type name.
     *
     * @param binding the type binding to evaluate
     * @return {@code true} if the binding is resolved and has a usable name, {@code false} otherwise
     */
    public static boolean isUsableType(ITypeBinding binding) {
        if (binding == null || binding.isRecovered()) return false;
        if (binding.isArray()) return binding.getDimensions() > 0 && isUsableType(binding.getElementType());
        ITypeBinding normalized = binding.isPrimitive() ? binding : binding.getErasure();
        if (normalized == null || normalized.isRecovered()) return false;
        String name = normalized.getQualifiedName();
        if (name == null || name.isBlank()) name = normalized.getName();
        return name != null && !name.isBlank() && !"<unresolved>".equals(name);
    }

    /**
     * Converts a JDT type binding to a normalized qualified type name, preserving array dimensions.
     *
     * @param binding the type binding to normalize
     * @return the normalized qualified type name, or {@code <unresolved>} when the binding has no usable name
     */
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

    /**
     * Normalizes a textual type by converting varargs to array notation, removing whitespace, and replacing dollar signs with dots.
     *
     * @param value the textual type to normalize
     * @return the normalized textual type
     */
    public static String normalizeTextType(String value) {
        return value.replace("...", "[]").replaceAll("\\s+", "").replace('$', '.');
    }

    /**
     * Builds a normalized textual type name for a method parameter.
     *
     * @param parameter the parameter declaration
     * @return the normalized parameter type, including extra array dimensions and varargs
     */
    private static String astParameterType(SingleVariableDeclaration parameter) {
        return normalizeTextType(parameter.getType() + "[]".repeat(parameter.getExtraDimensions())
                + (parameter.isVarargs() ? "[]" : ""));
    }

    public record ResolvedMethod(IMethodBinding declaration, ITypeBinding owner, String ownerName, String name,
                                 List<ITypeBinding> parameterBindings, List<String> parameterTypes,
                                 boolean constructor) {}

    public record CallableSymbol(String id, List<String> parameterTypes, ResolvedMethod resolved) {}
}
