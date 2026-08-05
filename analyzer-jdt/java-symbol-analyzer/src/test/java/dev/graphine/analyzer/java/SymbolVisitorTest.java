package dev.graphine.analyzer.java;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

import dev.graphine.analyzer.jdt.CallableIds;
import java.lang.reflect.Proxy;
import java.util.List;
import org.eclipse.jdt.core.dom.IBinding;
import org.eclipse.jdt.core.dom.IMethodBinding;
import org.eclipse.jdt.core.dom.IPackageBinding;
import org.eclipse.jdt.core.dom.ITypeBinding;
import org.eclipse.jdt.core.dom.IVariableBinding;
import org.junit.jupiter.api.Test;

final class SymbolVisitorTest {
    @Test
    void recoveredFieldWithoutDeclaringClassIsUnresolved() {
        IVariableBinding declaration = variableBinding(null, "count", null);
        IVariableBinding recoveredField = variableBinding(null, "count", declaration);

        assertNull(SymbolVisitor.resolveFieldBinding(recoveredField));
    }

    @Test
    void fieldWithoutCanonicalDeclarationIsUnresolved() {
        assertNull(SymbolVisitor.resolveFieldBinding(variableBinding(typeBinding("sample.Owner"), "count", null)));
    }

    @Test
    void fieldUsesCanonicalDeclarationOwnerAndName() {
        IVariableBinding declaration = variableBinding(typeBinding("sample.DeclaredOwner"), "declaredCount", null);
        IVariableBinding field = variableBinding(typeBinding("sample.UseSiteOwner"), "useSiteCount", declaration);

        SymbolVisitor.ResolvedField resolved = SymbolVisitor.resolveFieldBinding(field);

        assertEquals("sample.DeclaredOwner", resolved.ownerName());
        assertEquals("declaredCount", resolved.name());
    }

    @Test
    void constructorWithoutDeclaringClassFallsBackToLexicalOwner() {
        IMethodBinding declaration = methodBinding(null, "Owner", null, true, false, new ITypeBinding[0]);
        IMethodBinding constructor = methodBinding(null, "Owner", declaration, true, false, new ITypeBinding[0]);

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                constructor, "sample.LexicalOwner", "LexicalOwner", List.of(), true);

        assertNull(resolved.resolved());
        assertEquals("constructor:sample.LexicalOwner#<init>()", resolved.id());
    }

    @Test
    void constructorCallWithoutDeclaringClassDoesNotEmitExternalTarget() {
        IMethodBinding constructor = methodBinding(null, "Owner", null,
                true, false, new ITypeBinding[0]);
        GraphCollector collector = new GraphCollector();

        CallableIds.ResolvedMethod resolved = CallableIds.resolve(constructor);
        String target = collector.externalMethod(resolved);

        assertNull(resolved);
        assertNull(target);
        assertEquals(0, collector.nodeCount());
    }

    @Test
    void methodReferenceWithoutDeclaringClassDoesNotEmitExternalTarget() {
        IMethodBinding methodReference = methodBinding(null, "call", null,
                false, false, new ITypeBinding[] {typeBinding("java.lang.String")});
        GraphCollector collector = new GraphCollector();

        CallableIds.ResolvedMethod resolved = CallableIds.resolve(methodReference);
        String target = collector.externalMethod(resolved);

        assertNull(resolved);
        assertNull(target);
        assertEquals(0, collector.nodeCount());
    }

    @Test
    void constructorWithBlankNormalizedOwnerFallsBackToLexicalOwner() {
        IMethodBinding declaration = methodBinding(typeBinding(""), "Owner", null,
                true, false, new ITypeBinding[0]);

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                declaration, "sample.LexicalOwner", "LexicalOwner", List.of("java.lang.String"), true);

        assertNull(resolved.resolved());
        assertEquals("constructor:sample.LexicalOwner#<init>(java.lang.String)", resolved.id());
    }

    @Test
    void methodWithoutCanonicalDeclarationFallsBackToLexicalOwner() {
        IMethodBinding method = methodWithoutDeclaration(typeBinding("sample.BindingOwner"), "call");

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                method, "sample.LexicalOwner", "call", List.of("String", "int..."), false);

        assertNull(resolved.resolved());
        assertEquals("method:sample.LexicalOwner#call(String,int[])", resolved.id());
    }

    @Test
    void recoveredMethodFallsBackToLexicalOwner() {
        IMethodBinding recovered = methodBinding(typeBinding("sample.BindingOwner"), "call", null,
                false, true, new ITypeBinding[] {typeBinding("java.lang.String")});

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                recovered, "sample.LexicalOwner", "call", List.of("String"), false);

        assertNull(resolved.resolved());
        assertEquals("method:sample.LexicalOwner#call(String)", resolved.id());
    }

    @Test
    void methodWithUnusableCanonicalParameterFallsBackToAstSignature() {
        IMethodBinding declaration = methodBinding(typeBinding("sample.BindingOwner"), "call", null,
                false, false, new ITypeBinding[] {typeBinding("")});

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                declaration, "sample.LexicalOwner", "call", List.of("String"), false);

        assertNull(resolved.resolved());
        assertEquals("method:sample.LexicalOwner#call(String)", resolved.id());
    }

    @Test
    void fallbackMethodIdsHaveNonBlankLexicalOwners() {
        IMethodBinding constructor = methodBinding(null, "Owner", null, true, false, new ITypeBinding[0]);
        IMethodBinding method = methodWithoutDeclaration(typeBinding("sample.Owner"), "call");

        List<String> ids = List.of(
                CallableIds.resolveOrFallback(constructor, "sample.LexicalOwner", "LexicalOwner",
                        List.of(), true).id(),
                CallableIds.resolveOrFallback(method, "sample.LexicalOwner", "call",
                        List.of("String"), false).id());

        assertTrue(ids.stream().allMatch(id -> id.matches("(?:constructor|method):[^#]+#[^()]+\\(.*\\)")));
        assertFalse(ids.stream().anyMatch(id -> id.startsWith("constructor:#") || id.startsWith("method:#")));
    }

    @Test
    void validMethodUsesCanonicalDeclaration() {
        IMethodBinding declaration = methodBinding(typeBinding("sample.DeclaredOwner"), "declaredCall", null,
                false, false, new ITypeBinding[] {typeBinding("java.lang.String")});
        IMethodBinding method = methodBinding(typeBinding("sample.UseSiteOwner"), "useSiteCall", declaration,
                false, false, new ITypeBinding[0]);

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                method, "sample.LexicalOwner", "lexicalCall", List.of("Object"), false);

        assertSame(declaration, resolved.resolved().declaration());
        assertEquals("method:sample.DeclaredOwner#declaredCall(java.lang.String)", resolved.id());
        assertEquals(List.of("java.lang.String"), resolved.parameterTypes());
    }

    @Test
    void validConstructorUsesCanonicalDeclaration() {
        IMethodBinding declaration = methodBinding(typeBinding("sample.DeclaredOwner"), "DeclaredOwner", null,
                true, false, new ITypeBinding[] {typeBinding("int")});

        CallableIds.CallableSymbol resolved = CallableIds.resolveOrFallback(
                declaration, "sample.LexicalOwner", "LexicalOwner", List.of("Object"), true);

        assertSame(declaration, resolved.resolved().declaration());
        assertEquals("constructor:sample.DeclaredOwner#<init>(int)", resolved.id());
    }

    private static IVariableBinding variableBinding(ITypeBinding owner, String name, IVariableBinding declaration) {
        return (IVariableBinding) Proxy.newProxyInstance(SymbolVisitorTest.class.getClassLoader(),
                new Class<?>[] {IVariableBinding.class}, (proxy, method, arguments) -> switch (method.getName()) {
                    case "isField" -> true;
                    case "getDeclaringClass" -> owner;
                    case "getName" -> name;
                    case "getVariableDeclaration" -> declaration;
                    case "getKind" -> IBinding.VARIABLE;
                    case "toString" -> name;
                    default -> defaultValue(method.getReturnType());
                });
    }

    private static IMethodBinding methodBinding(ITypeBinding owner, String name, IMethodBinding declaration,
                                                boolean constructor, boolean recovered,
                                                ITypeBinding[] parameterTypes) {
        return (IMethodBinding) Proxy.newProxyInstance(SymbolVisitorTest.class.getClassLoader(),
                new Class<?>[] {IMethodBinding.class}, (proxy, method, arguments) -> switch (method.getName()) {
                    case "getDeclaringClass" -> owner;
                    case "getName" -> name;
                    case "getMethodDeclaration" -> declaration == null ? proxy : declaration;
                    case "getParameterTypes" -> parameterTypes;
                    case "isConstructor" -> constructor;
                    case "isRecovered" -> recovered;
                    case "getKind" -> IBinding.METHOD;
                    case "toString" -> name;
                    default -> defaultValue(method.getReturnType());
                });
    }

    private static IMethodBinding methodWithoutDeclaration(ITypeBinding owner, String name) {
        return (IMethodBinding) Proxy.newProxyInstance(SymbolVisitorTest.class.getClassLoader(),
                new Class<?>[] {IMethodBinding.class}, (proxy, method, arguments) -> switch (method.getName()) {
                    case "getDeclaringClass" -> owner;
                    case "getName" -> name;
                    case "getMethodDeclaration", "getParameterTypes" -> null;
                    case "isConstructor", "isRecovered" -> false;
                    case "getKind" -> IBinding.METHOD;
                    case "toString" -> name;
                    default -> defaultValue(method.getReturnType());
                });
    }

    private static ITypeBinding typeBinding(String qualifiedName) {
        return (ITypeBinding) Proxy.newProxyInstance(SymbolVisitorTest.class.getClassLoader(),
                new Class<?>[] {ITypeBinding.class}, (proxy, method, arguments) -> switch (method.getName()) {
                    case "isArray", "isPrimitive", "isRecovered" -> false;
                    case "getErasure" -> proxy;
                    case "getQualifiedName", "getName" -> qualifiedName;
                    case "getPackage" -> packageBinding(qualifiedName.contains(".")
                            ? qualifiedName.substring(0, qualifiedName.lastIndexOf('.')) : "");
                    case "toString" -> qualifiedName;
                    default -> defaultValue(method.getReturnType());
                });
    }

    private static IPackageBinding packageBinding(String name) {
        return (IPackageBinding) Proxy.newProxyInstance(SymbolVisitorTest.class.getClassLoader(),
                new Class<?>[] {IPackageBinding.class}, (proxy, method, arguments) -> switch (method.getName()) {
                    case "getName" -> name;
                    case "toString" -> name;
                    default -> defaultValue(method.getReturnType());
                });
    }

    private static Object defaultValue(Class<?> type) {
        if (!type.isPrimitive()) return null;
        if (type == boolean.class) return false;
        if (type == char.class) return '\0';
        if (type == byte.class) return (byte) 0;
        if (type == short.class) return (short) 0;
        if (type == int.class) return 0;
        if (type == long.class) return 0L;
        if (type == float.class) return 0F;
        if (type == double.class) return 0D;
        return null;
    }
}
