package dev.graphine.analyzer.java;

import dev.graphine.analyzer.protocol.AnalyzerOptions;
import dev.graphine.analyzer.protocol.Diagnostic;
import dev.graphine.analyzer.protocol.EdgeOccurrence;
import dev.graphine.analyzer.protocol.GraphEdge;
import dev.graphine.analyzer.protocol.GraphNode;
import dev.graphine.analyzer.jdt.CallableIds;
import dev.graphine.analyzer.resolver.SourceRoot;
import java.lang.reflect.Modifier;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Deque;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.stream.Collectors;
import org.eclipse.jdt.core.dom.*;

final class SymbolVisitor extends ASTVisitor {
    private final CompilationUnit unit;
    private final Path file;
    private final Path projectRoot;
    private final SourceRoot sourceRoot;
    private final GraphCollector graph;
    private final AnalyzerOptions options;
    private final Set<Integer> ambiguousLines;
    private final Deque<String> types = new ArrayDeque<>();
    private final Deque<String> methods = new ArrayDeque<>();
    private String packageName = "";

    SymbolVisitor(CompilationUnit unit, Path file, Path projectRoot, SourceRoot sourceRoot,
                  GraphCollector graph, AnalyzerOptions options, Set<Integer> ambiguousLines) {
        super(true);
        this.unit = unit;
        this.file = file;
        this.projectRoot = projectRoot;
        this.sourceRoot = sourceRoot;
        this.graph = graph;
        this.options = options;
        this.ambiguousLines = Set.copyOf(ambiguousLines);
        initializeContainerNodes();
    }

    private void initializeContainerNodes() {
        if (unit.getPackage() != null) packageName = unit.getPackage().getName().getFullyQualifiedName();
        String moduleId = "module:" + sourceRoot.moduleName();
        graph.node(node(moduleId, "MODULE", sourceRoot.moduleName(), sourceRoot.moduleName(), null, Map.of(
                "source_set", sourceRoot.sourceSet())));
        if (!packageName.isBlank()) {
            String packageId = "package:" + packageName;
            graph.node(node(packageId, "PACKAGE", packageName, packageName.substring(packageName.lastIndexOf('.') + 1),
                    null, Map.of("source_set", sourceRoot.sourceSet())));
            graph.edge(edge(moduleId, packageId, "DECLARES", "COMPILER_RESOLVED", Map.of()));
        }
    }

    @Override public boolean visit(TypeDeclaration node) { return enterType(node, node.resolveBinding()); }
    @Override public void endVisit(TypeDeclaration node) { types.pop(); }
    @Override public boolean visit(EnumDeclaration node) { return enterType(node, node.resolveBinding()); }
    @Override public void endVisit(EnumDeclaration node) { types.pop(); }
    @Override public boolean visit(RecordDeclaration node) { return enterType(node, node.resolveBinding()); }
    @Override public void endVisit(RecordDeclaration node) { types.pop(); }
    @Override public boolean visit(AnnotationTypeDeclaration node) { return enterType(node, node.resolveBinding()); }
    @Override public void endVisit(AnnotationTypeDeclaration node) { types.pop(); }

    private boolean enterType(AbstractTypeDeclaration declaration, ITypeBinding binding) {
        String qualified = binding == null ? fallbackType(declaration.getName().getIdentifier())
                : SymbolIds.normalizeType(binding);
        String id = "type:" + qualified;
        String kind = binding != null && binding.isAnnotation() ? "ANNOTATION_TYPE" : "TYPE";
        Map<String, Object> metadata = new LinkedHashMap<>();
        metadata.put("type_kind", binding == null ? declaration.getClass().getSimpleName() : GraphCollector.typeKind(binding));
        metadata.put("modifiers", Modifier.toString(declaration.getModifiers()));
        metadata.put("abstract", Modifier.isAbstract(declaration.getModifiers()));
        metadata.put("final", Modifier.isFinal(declaration.getModifiers()));
        metadata.put("sealed", hasModifier(declaration, "sealed"));
        metadata.put("source_set", sourceRoot.sourceSet());
        metadata.put("generic_parameters", binding == null ? List.of() : Arrays.stream(binding.getTypeParameters()).map(ITypeBinding::getName).toList());
        graph.node(node(id, kind, qualified, declaration.getName().getIdentifier(), declaration, metadata));
        String container = types.peek();
        if (container == null) container = packageName.isBlank() ? "module:" + sourceRoot.moduleName() : "package:" + packageName;
        graph.edge(edge(container, id, "DECLARES", binding == null ? "STATIC_INFERRED" : "COMPILER_RESOLVED", Map.of()));
        types.push(id);
        if (binding == null) unresolved("UNRESOLVED_TYPE_DECLARATION", declaration, declaration.getName().getIdentifier(), "Type binding unavailable");
        else {
            graph.resolved();
            addTypeRelation(id, binding.getSuperclass(), "EXTENDS");
            for (ITypeBinding implemented : binding.getInterfaces())
                addTypeRelation(id, implemented, binding.isInterface() ? "EXTENDS" : "IMPLEMENTS");
        }
        return true;
    }

    /**
     * Visits a method or constructor declaration, records its symbol and metadata, and adds its relationships to the graph.
     *
     * @param declaration the method or constructor declaration to process
     * @return {@code true} if method-body traversal is enabled or the declaration has no body; {@code false} otherwise
     */
    @Override public boolean visit(MethodDeclaration declaration) {
        String owner = types.peek();
        if (owner == null) return true;
        IMethodBinding binding = declaration.resolveBinding();
        boolean constructor = declaration.isConstructor();
        CallableIds.CallableSymbol method = CallableIds.forDeclaration(binding,
                owner.substring("type:".length()), declaration);
        String id = method.id();
        CallableIds.ResolvedMethod resolved = method.resolved();
        IMethodBinding canonical = resolved == null ? null : resolved.declaration();
        Map<String, Object> metadata = new LinkedHashMap<>();
        metadata.put("declaring_type", owner);
        metadata.put("name", constructor ? "<init>" : declaration.getName().getIdentifier());
        metadata.put("parameter_types", method.parameterTypes());
        metadata.put("return_type", constructor ? "void" : canonical == null
                ? declaration.getReturnType2().toString() : SymbolIds.normalizeType(canonical.getReturnType()));
        metadata.put("modifiers", Modifier.toString(declaration.getModifiers()));
        metadata.put("generic_parameters", canonical == null ? List.of() : Arrays.stream(canonical.getTypeParameters()).map(ITypeBinding::getName).toList());
        metadata.put("thrown_types", canonical == null ? List.of() : Arrays.stream(canonical.getExceptionTypes()).map(SymbolIds::normalizeType).toList());
        metadata.put("has_body", declaration.getBody() != null);
        metadata.put("source_set", sourceRoot.sourceSet());
        String confidence = canonical == null ? "STATIC_INFERRED" : "COMPILER_RESOLVED";
        String provenance = canonical == null ? "eclipse-jdt-ast" : "eclipse-jdt";
        graph.node(node(id, constructor ? "CONSTRUCTOR" : "METHOD", owner.substring(5) + "#" + declaration.getName(),
                constructor ? "<init>" : declaration.getName().getIdentifier(), declaration, metadata,
                confidence, provenance));
        graph.edge(edge(owner, id, "DECLARES", confidence, Map.of()));
        methods.push(id);
        if (canonical == null) unresolved("UNRESOLVED_METHOD_DECLARATION", declaration,
                declaration.getName().getIdentifier(), "Method binding unavailable or incomplete");
        else {
            graph.resolved();
            addMethodTypeEdges(id, resolved);
            addOverrides(id, resolved, declaration);
        }
        return options.includeMethodBodies() || declaration.getBody() == null;
    }

    @Override public void endVisit(MethodDeclaration node) { methods.pop(); }

    @Override public boolean visit(FieldDeclaration declaration) {
        String owner = types.peek();
        if (owner == null) return true;
        for (Object value : declaration.fragments()) {
            VariableDeclarationFragment fragment = (VariableDeclarationFragment) value;
            IVariableBinding binding = fragment.resolveBinding();
            String id = SymbolIds.field(owner.substring(5), fragment.getName().getIdentifier());
            Map<String, Object> metadata = new LinkedHashMap<>();
            metadata.put("declaring_type", owner);
            metadata.put("field_type", binding == null ? declaration.getType().toString() : SymbolIds.normalizeType(binding.getType()));
            metadata.put("modifiers", Modifier.toString(declaration.getModifiers()));
            metadata.put("source_set", sourceRoot.sourceSet());
            graph.node(node(id, "FIELD", owner.substring(5) + "#" + fragment.getName(), fragment.getName().getIdentifier(), fragment, metadata));
            graph.edge(edge(owner, id, "DECLARES", binding == null ? "STATIC_INFERRED" : "COMPILER_RESOLVED", Map.of()));
            if (binding != null) {
                graph.resolved();
                addTypeEdge(id, binding.getType(), "REFERENCES_TYPE", Map.of("usage", "field_type"));
            }
        }
        return true;
    }

    @Override public boolean visit(EnumConstantDeclaration declaration) {
        String owner = types.peek();
        if (owner != null) {
            String id = SymbolIds.field(owner.substring(5), declaration.getName().getIdentifier());
            graph.node(node(id, "ENUM_CONSTANT", owner.substring(5) + "#" + declaration.getName(), declaration.getName().getIdentifier(), declaration,
                    Map.of("source_set", sourceRoot.sourceSet())));
            graph.edge(edge(owner, id, "DECLARES", "COMPILER_RESOLVED", Map.of()));
        }
        return true;
    }

    @Override public boolean visit(MethodInvocation invocation) {
        call(invocation.resolveMethodBinding(), invocation, invocation.getName().getIdentifier(), false);
        return true;
    }

    @Override public boolean visit(SuperMethodInvocation invocation) {
        call(invocation.resolveMethodBinding(), invocation, invocation.getName().getIdentifier(), false);
        return true;
    }

    @Override public boolean visit(ClassInstanceCreation creation) {
        call(creation.resolveConstructorBinding(), creation, creation.getType().toString(), true);
        return true;
    }

    @Override public boolean visit(ConstructorInvocation invocation) {
        call(invocation.resolveConstructorBinding(), invocation, "this", true);
        return true;
    }

    @Override public boolean visit(SuperConstructorInvocation invocation) {
        call(invocation.resolveConstructorBinding(), invocation, "super", true);
        return true;
    }

    @Override public boolean visit(ExpressionMethodReference reference) { methodReference(reference.resolveMethodBinding(), reference); return true; }
    @Override public boolean visit(TypeMethodReference reference) { methodReference(reference.resolveMethodBinding(), reference); return true; }
    @Override public boolean visit(SuperMethodReference reference) { methodReference(reference.resolveMethodBinding(), reference); return true; }
    @Override public boolean visit(CreationReference reference) { methodReference(reference.resolveMethodBinding(), reference); return true; }

    @Override public boolean visit(LambdaExpression lambda) {
        ITypeBinding type = lambda.resolveTypeBinding();
        String source = methods.peek();
        if (source != null && type != null) addTypeEdge(source, type, "REFERENCES_TYPE", Map.of("usage", "lambda_functional_interface", "runtime_execution", "not_proven"));
        else if (source != null) unresolved("UNRESOLVED_LAMBDA_TARGET", lambda, "lambda", "Functional interface target unavailable");
        return true;
    }

    @Override public boolean visit(MarkerAnnotation annotation) { handleAnnotation(annotation); return true; }
    @Override public boolean visit(NormalAnnotation annotation) { handleAnnotation(annotation); return true; }
    @Override public boolean visit(SingleMemberAnnotation annotation) { handleAnnotation(annotation); return true; }

    private void handleAnnotation(Annotation annotation) {
        String target = methods.peek();
        if (target == null) target = types.peek();
        ITypeBinding binding = annotation.resolveTypeBinding();
        if (target != null && binding != null) {
            String annotationId = graph.externalType(binding);
            Map<String, Object> metadata = new LinkedHashMap<>();
            metadata.put("explicit_values", annotationValues(annotation));
            metadata.put("spring_semantics", "not_analyzed");
            graph.edge(edgeAt(target, annotationId, "ANNOTATED_BY", "COMPILER_RESOLVED", metadata, annotation));
            graph.resolved();
        } else if (target != null) unresolved("UNRESOLVED_ANNOTATION", annotation, annotation.getTypeName().getFullyQualifiedName(), "Annotation binding unavailable");
    }

    @Override public boolean visit(SimpleName name) {
        if (!options.includeFieldAccess() || methods.isEmpty() || isDeclarationName(name)) return true;
        IBinding resolved = name.resolveBinding();
        if (resolved instanceof IVariableBinding variable && variable.isField()) {
            ResolvedField field = resolveFieldBinding(variable);
            if (field == null) {
                unresolved("UNRESOLVED_FIELD_ACCESS", name, name.getIdentifier(),
                        "Field declaring class or canonical declaration unavailable");
                return true;
            }
            String target = SymbolIds.field(field.ownerName(), field.name());
            graph.externalType(field.owner());
            graph.node(new GraphNode(target, "FIELD", field.ownerName() + "#" + field.name(),
                    field.name(), null, field.owner().getPackage() == null ? null : field.owner().getPackage().getName(),
                    null, null, null, "COMPILER_RESOLVED", "eclipse-jdt-binding", Map.of("external", !types.contains("type:" + field.ownerName())), List.of()));
            Access access = classifyAccess(name);
            if (access.read) graph.edge(edgeAt(methods.peek(), target, "READS_FIELD", "COMPILER_RESOLVED", Map.of(), name));
            if (access.write) graph.edge(edgeAt(methods.peek(), target, "WRITES_FIELD", "COMPILER_RESOLVED", Map.of(), name));
            graph.resolved();
        }
        return true;
    }

    @Override public boolean visit(ImportDeclaration declaration) {
        IBinding binding = declaration.resolveBinding();
        if (binding instanceof ITypeBinding type) {
            String source = packageName.isBlank() ? "module:" + sourceRoot.moduleName() : "package:" + packageName;
            graph.edge(edge(source, graph.externalType(type), "IMPORTS", "COMPILER_RESOLVED", Map.of("static", declaration.isStatic())));
        }
        return true;
    }

    /**
     * Records a compiler-resolved method or constructor call from the current method.
     *
     * @param binding     the method binding for the call
     * @param location    the source location of the call
     * @param text        the source text associated with the call
     * @param constructor whether the call invokes a constructor
     */
    private void call(IMethodBinding binding, ASTNode location, String text, boolean constructor) {
        String source = methods.peek();
        if (source == null) return;
        if (ambiguousLines.contains(startLine(location))) return;
        CallableIds.ResolvedMethod resolved = CallableIds.resolve(binding);
        if (resolved == null) {
            unresolved(constructor ? "UNRESOLVED_CONSTRUCTOR_CALL" : "UNRESOLVED_METHOD_CALL", location,
                    text, "Canonical method declaration, owner, or parameters unavailable");
            return;
        }
        IMethodBinding canonical = resolved.declaration();
        String target = graph.externalMethod(resolved);
        String dispatch = resolved.constructor() || Modifier.isStatic(canonical.getModifiers())
                || Modifier.isPrivate(canonical.getModifiers()) || Modifier.isFinal(canonical.getModifiers())
                || Modifier.isFinal(resolved.owner().getModifiers())
                ? "exactly_resolved" : "virtual_declared_target";
        graph.edge(edgeAt(source, target, constructor ? "CONSTRUCTS" : "CALLS",
                "COMPILER_RESOLVED", Map.of("dispatch", dispatch), location));
        graph.resolved();
    }

    private void methodReference(IMethodBinding binding, ASTNode location) {
        String source = methods.peek();
        if (source == null) return;
        if (ambiguousLines.contains(startLine(location))) return;
        CallableIds.ResolvedMethod resolved = CallableIds.resolve(binding);
        if (resolved == null) unresolved("UNRESOLVED_METHOD_REFERENCE", location, "method reference",
                "Canonical method declaration, owner, or parameters unavailable");
        else {
            graph.edge(edgeAt(source, graph.externalMethod(resolved), resolved.constructor() ? "CONSTRUCTS" : "CALLS",
                    "COMPILER_RESOLVED", Map.of("method_reference", true, "runtime_execution", "not_proven"), location));
            graph.resolved();
        }
    }

    private void addMethodTypeEdges(String source, CallableIds.ResolvedMethod method) {
        IMethodBinding declaration = method.declaration();
        ITypeBinding returnType = declaration.getReturnType();
        if (!method.constructor() && CallableIds.isUsableType(returnType) && !returnType.isPrimitive())
            addTypeEdge(source, returnType, "RETURNS_TYPE", Map.of());
        for (ITypeBinding parameter : method.parameterBindings())
            addTypeEdge(source, parameter, "ACCEPTS_TYPE", Map.of());
        ITypeBinding[] exceptions = declaration.getExceptionTypes();
        if (exceptions != null)
            for (ITypeBinding thrown : exceptions) addTypeEdge(source, thrown, "THROWS_TYPE", Map.of());
    }

    /**
     * Records methods overridden by the specified method in its superclass and implemented interfaces.
     *
     * @param source   the source file containing the method
     * @param method   the resolved method whose overrides are analyzed
     * @param location the AST location associated with the method
     */
    private void addOverrides(String source, CallableIds.ResolvedMethod method, ASTNode location) {
        ITypeBinding owner = method.owner();
        collectOverridden(source, method, owner.getSuperclass(), location);
        ITypeBinding[] interfaces = owner.getInterfaces();
        if (interfaces != null)
            for (ITypeBinding iface : interfaces) collectOverridden(source, method, iface, location);
    }

    /**
     * Records resolved methods overridden by the specified method across a type's superclass and interfaces.
     *
     * @param source   the source identifier for the declaring method
     * @param method   the resolved method being analyzed
     * @param parent   the type whose declared and inherited relationships are examined
     * @param location the AST location associated with unresolved override diagnostics
     */
    private void collectOverridden(String source, CallableIds.ResolvedMethod method,
                                   ITypeBinding parent, ASTNode location) {
        if (parent == null) return;
        if (!CallableIds.isUsableType(parent)) {
            unresolved("UNRESOLVED_OVERRIDE_TARGET", location, method.name(),
                    "Parent type binding unavailable or incomplete");
            return;
        }
        IMethodBinding[] candidates = parent.getDeclaredMethods();
        if (candidates != null) for (IMethodBinding candidate : candidates) {
            CallableIds.ResolvedMethod resolvedCandidate = CallableIds.resolve(candidate);
            if (resolvedCandidate == null) {
                unresolved("UNRESOLVED_OVERRIDE_TARGET", location, method.name(),
                        "Override candidate binding unavailable or incomplete");
                continue;
            }
            if (method.declaration().overrides(resolvedCandidate.declaration())) {
                Map<String, Object> metadata = new LinkedHashMap<>();
                ITypeBinding methodReturn = method.declaration().getReturnType();
                ITypeBinding candidateReturn = resolvedCandidate.declaration().getReturnType();
                if (CallableIds.isUsableType(methodReturn) && CallableIds.isUsableType(candidateReturn))
                    metadata.put("covariant_return", !SymbolIds.normalizeType(methodReturn)
                            .equals(SymbolIds.normalizeType(candidateReturn)));
                graph.edge(edge(source, graph.externalMethod(resolvedCandidate), "OVERRIDES",
                        "COMPILER_RESOLVED", metadata));
                graph.resolved();
            }
        }
        collectOverridden(source, method, parent.getSuperclass(), location);
        ITypeBinding[] interfaces = parent.getInterfaces();
        if (interfaces != null)
            for (ITypeBinding iface : interfaces) collectOverridden(source, method, iface, location);
    }

    /**
     * Adds a type relationship when the target resolves to a type other than {@code java.lang.Object}.
     *
     * @param source the source type identifier
     * @param target the target type binding
     * @param kind   the relationship kind
     */
    private void addTypeRelation(String source, ITypeBinding target, String kind) {
        if (target != null && !"java.lang.Object".equals(SymbolIds.normalizeType(target))) addTypeEdge(source, target, kind, Map.of());
    }

    /**
     * Adds a compiler-resolved edge from a source symbol to a usable reference type.
     *
     * @param source   the source symbol identifier
     * @param target   the referenced type
     * @param kind     the relationship kind
     * @param metadata metadata associated with the edge
     */
    private void addTypeEdge(String source, ITypeBinding target, String kind, Map<String, Object> metadata) {
        if (!CallableIds.isUsableType(target) || target.isPrimitive()) return;
        graph.edge(edge(source, graph.externalType(target), kind, "COMPILER_RESOLVED", metadata));
    }

    /**
     * Creates a graph node using compiler-resolved provenance metadata.
     *
     * @param id         the node identifier
     * @param kind       the node kind
     * @param qualified  the qualified node name
     * @param simple     the simple node name
     * @param location   the AST location associated with the node
     * @param metadata   additional node metadata
     * @return           the created graph node
     */
    private GraphNode node(String id, String kind, String qualified, String simple, ASTNode location,
                           Map<String, Object> metadata) {
        return node(id, kind, qualified, simple, location, metadata, "COMPILER_RESOLVED", "eclipse-jdt");
    }

    /**
     * Creates a graph node with source location, module, package, metadata, confidence, and provenance information.
     *
     * @param id         the node identifier
     * @param kind       the node kind
     * @param qualified  the qualified name
     * @param simple     the simple name
     * @param location   the AST location associated with the node
     * @param metadata   the node metadata
     * @param confidence the confidence level for the node
     * @param provenance the source provenance for the node
     * @return           the graph node populated with the specified information
     */
    private GraphNode node(String id, String kind, String qualified, String simple, ASTNode location,
                           Map<String, Object> metadata, String confidence, String provenance) {
        return new GraphNode(id, kind, qualified, simple, sourceRoot.moduleName(), packageName,
                location == null ? null : JavaProjectAnalyzer.relative(projectRoot, file),
                location == null ? null : startLine(location), location == null ? null : endLine(location),
                confidence, provenance, metadata, List.of());
    }

    /**
     * Creates a graph edge with the specified endpoints, kind, confidence, and metadata.
     *
     * @param source     the source node identifier
     * @param target     the target node identifier
     * @param kind       the edge kind
     * @param confidence the confidence level for the edge
     * @param metadata   additional edge metadata
     * @return the configured graph edge
     */
    private GraphEdge edge(String source, String target, String kind, String confidence, Map<String, Object> metadata) {
        return new GraphEdge(source, target, kind, confidence, "eclipse-jdt-binding", metadata);
    }

    private GraphEdge edgeAt(String source, String target, String kind, String confidence,
                             Map<String, Object> metadata, ASTNode location) {
        EdgeOccurrence occurrence = new EdgeOccurrence(
                JavaProjectAnalyzer.relative(projectRoot, file), startLine(location), endLine(location), Map.of());
        return new GraphEdge(source, target, kind, confidence, "eclipse-jdt-binding", metadata,
                List.of(occurrence));
    }

    private void unresolved(String kind, ASTNode location, String symbol, String reason) {
        graph.diagnostic(new Diagnostic(kind, JavaProjectAnalyzer.relative(projectRoot, file), startLine(location),
                endLine(location), bounded(symbol), reason, "warning"));
    }

    private int startLine(ASTNode node) { return Math.max(1, unit.getLineNumber(node.getStartPosition())); }
    private int endLine(ASTNode node) { return Math.max(startLine(node), unit.getLineNumber(node.getStartPosition() + Math.max(0, node.getLength() - 1))); }
    private String fallbackType(String simple) { return types.isEmpty() ? (packageName.isBlank() ? simple : packageName + '.' + simple) : types.peek().substring(5) + '.' + simple; }
    private static String bounded(String value) { return value.length() <= 200 ? value : value.substring(0, 200); }

    private static boolean hasModifier(AbstractTypeDeclaration declaration, String keyword) {
        return declaration.modifiers().stream().anyMatch(value -> value.toString().equals(keyword));
    }

    private static Map<String, String> annotationValues(Annotation annotation) {
        Map<String, String> values = new LinkedHashMap<>();
        if (annotation.isSingleMemberAnnotation()) values.put("value", bounded(((SingleMemberAnnotation) annotation).getValue().toString()));
        else if (annotation.isNormalAnnotation()) for (Object value : ((NormalAnnotation) annotation).values()) {
            MemberValuePair pair = (MemberValuePair) value;
            values.put(pair.getName().getIdentifier(), bounded(pair.getValue().toString()));
        }
        return values;
    }

    private static boolean isDeclarationName(SimpleName name) {
        ASTNode parent = name.getParent();
        return (parent instanceof VariableDeclarationFragment fragment && fragment.getName() == name)
                || (parent instanceof SingleVariableDeclaration declaration && declaration.getName() == name)
                || (parent instanceof MethodDeclaration declaration && declaration.getName() == name)
                || (parent instanceof AbstractTypeDeclaration declaration && declaration.getName() == name)
                || (parent instanceof EnumConstantDeclaration declaration && declaration.getName() == name);
    }

    private static Access classifyAccess(SimpleName name) {
        ASTNode expression = name;
        ASTNode parent = name.getParent();
        if (parent instanceof FieldAccess field && field.getName() == name) { expression = field; parent = field.getParent(); }
        if (parent instanceof QualifiedName qualified && qualified.getName() == name) { expression = qualified; parent = qualified.getParent(); }
        if (parent instanceof Assignment assignment && assignment.getLeftHandSide() == expression)
            return assignment.getOperator() == Assignment.Operator.ASSIGN ? new Access(false, true) : new Access(true, true);
        if (parent instanceof PostfixExpression) return new Access(true, true);
        if (parent instanceof PrefixExpression prefix && (prefix.getOperator() == PrefixExpression.Operator.INCREMENT
                || prefix.getOperator() == PrefixExpression.Operator.DECREMENT)) return new Access(true, true);
        return new Access(true, false);
    }

    static ResolvedField resolveFieldBinding(IVariableBinding variable) {
        IVariableBinding declaration = variable.getVariableDeclaration();
        if (declaration == null) return null;
        ITypeBinding owner = declaration.getDeclaringClass();
        if (owner == null) owner = variable.getDeclaringClass();
        String canonicalName = declaration.getName();
        if (owner == null || canonicalName == null || canonicalName.isBlank()) return null;
        return new ResolvedField(owner, SymbolIds.normalizeType(owner), canonicalName);
    }

    record ResolvedField(ITypeBinding owner, String ownerName, String name) {}

    private record Access(boolean read, boolean write) {}
}
