package dev.graphine.analyzer.java;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

import java.lang.reflect.Proxy;
import org.eclipse.jdt.core.dom.IBinding;
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

    private static ITypeBinding typeBinding(String qualifiedName) {
        return (ITypeBinding) Proxy.newProxyInstance(SymbolVisitorTest.class.getClassLoader(),
                new Class<?>[] {ITypeBinding.class}, (proxy, method, arguments) -> switch (method.getName()) {
                    case "isArray", "isPrimitive" -> false;
                    case "getErasure" -> proxy;
                    case "getQualifiedName", "getName" -> qualifiedName;
                    case "getPackage" -> packageBinding(qualifiedName.substring(0, qualifiedName.lastIndexOf('.')));
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
