package dev.graphine.analyzer.spring;

import dev.graphine.analyzer.jdt.CallableIds;
import dev.graphine.analyzer.protocol.Diagnostic;
import dev.graphine.analyzer.protocol.EdgeOccurrence;
import dev.graphine.analyzer.protocol.GraphEdge;
import dev.graphine.analyzer.protocol.GraphNode;
import dev.graphine.analyzer.protocol.GraphSink;
import dev.graphine.analyzer.resolver.SourceRoot;
import java.beans.Introspector;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Comparator;
import java.util.Deque;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import org.eclipse.jdt.core.dom.*;

/**
 * Bounded Spring semantic pass over JDT compilation units that were already parsed for Java facts.
 * The pass stores only normalized descriptors; no AST or source body survives {@link #accept}.
 */
public final class SpringSemanticAnalyzer {
    private static final String PROVENANCE = "spring-static-v1";
    private static final Set<String> STEREOTYPES = Set.of(
            "org.springframework.stereotype.Component", "org.springframework.stereotype.Service",
            "org.springframework.stereotype.Repository", "org.springframework.stereotype.Controller",
            "org.springframework.web.bind.annotation.RestController",
            "org.springframework.context.annotation.Configuration");
    private static final Set<String> REPOSITORY_BASES = Set.of(
            "org.springframework.data.repository.Repository",
            "org.springframework.data.repository.CrudRepository",
            "org.springframework.data.repository.ListCrudRepository",
            "org.springframework.data.repository.PagingAndSortingRepository",
            "org.springframework.data.jpa.repository.JpaRepository");
    private static final Set<String> VALIDATION = Set.of(
            "jakarta.validation.Valid", "org.springframework.validation.annotation.Validated",
            "jakarta.validation.constraints.NotNull", "jakarta.validation.constraints.NotBlank",
            "jakarta.validation.constraints.Size", "jakarta.validation.constraints.Min",
            "jakarta.validation.constraints.Max", "jakarta.validation.constraints.Pattern",
            "jakarta.validation.constraints.Future");
    private static final Pattern PROPERTY_PLACEHOLDER = Pattern.compile("\\$\\{([^}:]+)(?::[^}]*)?}");
    private static final Pattern SECRET_KEY = Pattern.compile("(?i).*(password|passwd|token|secret|credential|private[-_.]?key|api[-_.]?key).*");

    private final Path projectRoot;
    private final Map<String, TypeInfo> types = new LinkedHashMap<>();
    private final List<CallInfo> calls = new ArrayList<>();
    private final List<Diagnostic> diagnostics = new ArrayList<>();

    public SpringSemanticAnalyzer(Path projectRoot) {
        this.projectRoot = projectRoot.toAbsolutePath().normalize();
    }

    public void accept(CompilationUnit unit, Path file, SourceRoot sourceRoot) {
        unit.accept(new Collector(unit, file.toAbsolutePath().normalize(), sourceRoot));
    }

    public void emit(GraphSink graph) {
        Map<String, BeanInfo> beans = deriveBeans(graph);
        deriveInjection(graph, beans);
        deriveSupplementalSemantics(graph);
        deriveRoutes(graph);
        Map<String, RepositoryInfo> repositories = deriveRepositoriesAndEntities(graph);
        deriveRepositoryCalls(graph, repositories);
        deriveConfiguration(graph);
        deriveEvents(graph);
        diagnostics.forEach(graph::diagnostic);
    }

    private Map<String, BeanInfo> deriveBeans(GraphSink graph) {
        List<String> scanPackages = new ArrayList<>(types.values().stream()
                .filter(type -> type.annotations.containsKey("org.springframework.boot.autoconfigure.SpringBootApplication"))
                .map(type -> type.packageName).distinct().toList());
        for (TypeInfo type : types.values()) {
            Ann scan = type.annotations.values().stream().filter(value -> "ComponentScan".equals(value.simpleName()))
                    .findFirst().orElse(null);
            if (scan == null) continue;
            scanPackages.addAll(nonEmpty(scan.list("basePackages"), scan.list("value")));
            for (String className : scan.list("basePackageClasses")) {
                int separator = className.lastIndexOf('.');
                if (separator > 0) scanPackages.add(className.substring(0, separator));
            }
        }
        List<String> resolvedScanPackages = scanPackages.stream()
                .filter(value -> value != null && !value.isBlank()).distinct().toList();
        Map<String, BeanInfo> beans = new LinkedHashMap<>();
        boolean unknownScan = resolvedScanPackages.isEmpty();
        for (TypeInfo type : types.values()) {
            Stereotype stereotype = stereotype(type, new LinkedHashSet<>());
            if (stereotype == null) continue;
            boolean scanned = unknownScan || resolvedScanPackages.stream().anyMatch(root ->
                    type.packageName.equals(root) || type.packageName.startsWith(root + "."));
            if (!scanned) continue;
            String explicit = firstNonBlank(stereotype.annotation.string("value"), stereotype.annotation.string("name"));
            String name = explicit == null ? Introspector.decapitalize(type.simpleName) : explicit;
            String confidence = unknownScan ? "STATIC_INFERRED" : "FRAMEWORK_RESOLVED";
            List<Map<String, Object>> conditions = conditions(type.annotations);
            if (!conditions.isEmpty()) confidence = "STATIC_INFERRED";
            BeanInfo bean = new BeanInfo("bean:" + name, name, type.qualifiedName, type.allTypes,
                    type.annotations.containsKey("org.springframework.context.annotation.Primary"),
                    qualifiers(type.annotations), conditions, type.location, type.id, null);
            beans.put(bean.id, bean);
            graph.node(semanticTypeNode(type, "SPRING_COMPONENT", confidence, Map.of(
                    "stereotype", stereotype.kind, "bean_name", name,
                    "meta_annotation_chain", stereotype.chain, "scan_boundary_known", !unknownScan)));
            graph.node(node(bean.id, "BEAN", name, name, type.location, confidence,
                    mapOf("declared_type", type.qualifiedName, "stereotype", stereotype.kind,
                            "primary", bean.primary, "qualifiers", bean.qualifiers,
                            "conditions", bean.conditions, "implementation_type", type.qualifiedName)));
            graph.edge(edge(type.id, bean.id, "DECLARES_BEAN", confidence,
                    Map.of("selection_name", name), type.location));
        }
        if (unknownScan && !beans.isEmpty()) diagnostics.add(diagnostic("UNKNOWN_COMPONENT_SCAN_BOUNDARY",
                beans.values().iterator().next().location, null,
                "No @SpringBootApplication or explicit component scan root was resolved; candidates are retained statically"));

        for (TypeInfo type : types.values()) {
            if (!type.annotations.containsKey("org.springframework.context.annotation.Configuration")) continue;
            for (MethodInfo method : type.methods) {
                Ann beanAnn = method.annotations.get("org.springframework.context.annotation.Bean");
                if (beanAnn == null) continue;
                List<String> names = beanAnn.list("name");
                if (names.isEmpty()) names = beanAnn.list("value");
                String name = names.isEmpty() ? method.name : names.get(0);
                Set<String> aliases = names.size() <= 1 ? Set.of() : new LinkedHashSet<>(names.subList(1, names.size()));
                List<Map<String, Object>> conditions = new ArrayList<>(conditions(type.annotations));
                conditions.addAll(conditions(method.annotations));
                String confidence = conditions.isEmpty() ? "FRAMEWORK_RESOLVED" : "STATIC_INFERRED";
                Set<String> assignable = new LinkedHashSet<>();
                assignable.add(method.returnType);
                TypeInfo returnInfo = types.get(method.returnType);
                if (returnInfo != null) assignable.addAll(returnInfo.allTypes);
                BeanInfo bean = new BeanInfo("bean:" + name, name, method.returnType, assignable,
                        method.annotations.containsKey("org.springframework.context.annotation.Primary"),
                        qualifiers(method.annotations), conditions, method.location, type.id, method.id);
                beans.put(bean.id, bean);
                Map<String, Object> metadata = mapOf("declared_type", method.returnType, "aliases", aliases,
                        "configuration_class", type.qualifiedName, "factory_method", method.id,
                        "primary", bean.primary, "qualifiers", bean.qualifiers, "conditions", conditions);
                if (method.beanImplementationType != null) metadata.put("implementation_type", method.beanImplementationType);
                graph.node(node(bean.id, "BEAN", name, name, method.location, confidence, metadata));
                graph.edge(edge(method.id, bean.id, "DECLARES_BEAN", confidence, Map.of(), method.location));
            }
        }
        return beans;
    }

    private void deriveInjection(GraphSink graph, Map<String, BeanInfo> beans) {
        for (TypeInfo type : types.values()) {
            List<MethodInfo> constructors = type.methods.stream().filter(method -> method.constructor).toList();
            List<MethodInfo> selected = constructors.stream()
                    .filter(method -> method.annotations.containsKey("org.springframework.beans.factory.annotation.Autowired"))
                    .toList();
            if (selected.isEmpty() && constructors.size() == 1) selected = constructors;
            for (MethodInfo constructor : selected) {
                if (constructor.synthetic) {
                    graph.node(node(constructor.id, "CONSTRUCTOR", type.qualifiedName + "#<init>", "<init>",
                            constructor.location, "FRAMEWORK_RESOLVED", mapOf("declaring_type", type.id,
                                    "parameter_types", constructor.parameters.stream().map(parameter -> parameter.type).toList(),
                                    "framework_synthetic", "record-canonical-constructor")));
                    graph.edge(edge(type.id, constructor.id, "DECLARES", "FRAMEWORK_RESOLVED",
                            Map.of("framework_synthetic", true), constructor.location));
                }
                for (ParameterInfo parameter : constructor.parameters) {
                    String required = parameter.containerElementType != null ? parameter.containerElementType : parameter.type;
                    List<BeanInfo> candidates = beans.values().stream()
                            .filter(bean -> bean.assignableTypes.contains(required)).toList();
                    if ("org.springframework.context.ApplicationEventPublisher".equals(required)) {
                        BeanInfo infrastructure = new BeanInfo("bean:applicationEventPublisher", "applicationEventPublisher",
                                required, Set.of(required), false, Set.of(), List.of(), parameter.location, null, null);
                        if (!beans.containsKey(infrastructure.id)) {
                            beans.put(infrastructure.id, infrastructure);
                            graph.node(node(infrastructure.id, "BEAN", infrastructure.name, infrastructure.name,
                                    parameter.location, "STATIC_INFERRED", Map.of("declared_type", required,
                                            "framework_infrastructure", true)));
                        }
                        candidates = List.of(infrastructure);
                    }
                    String qualifier = qualifier(parameter.annotations);
                    BeanInfo chosen = null;
                    String reason = null;
                    if (qualifier != null) {
                        chosen = candidates.stream().filter(bean -> bean.name.equals(qualifier)
                                || bean.qualifiers.contains(qualifier)).findFirst().orElse(null);
                        reason = "QUALIFIER";
                        if (chosen == null) diagnostics.add(diagnostic("UNRESOLVED_QUALIFIER", parameter.location,
                                qualifier, "No statically known bean matches the qualifier"));
                    } else if (candidates.size() == 1) {
                        chosen = candidates.get(0); reason = "UNIQUE_CANDIDATE";
                    } else {
                        List<BeanInfo> primaries = candidates.stream().filter(bean -> bean.primary).toList();
                        if (primaries.size() == 1) { chosen = primaries.get(0); reason = "PRIMARY"; }
                        else if (primaries.size() > 1) diagnostics.add(diagnostic("MULTIPLE_PRIMARY_BEANS",
                                parameter.location, required, "Multiple @Primary candidates remain"));
                        else {
                            chosen = candidates.stream().filter(bean -> bean.name.equals(parameter.name)).findFirst().orElse(null);
                            if (chosen != null) reason = "INJECTION_POINT_NAME";
                        }
                    }
                    for (BeanInfo candidate : candidates) graph.edge(edge(constructor.id, candidate.id,
                            "BEAN_CANDIDATE", candidates.size() > 1 ? "AMBIGUOUS" : "FRAMEWORK_RESOLVED",
                            mapOf("parameter", parameter.name, "required_type", required,
                                    "collection", parameter.collection, "optional", parameter.optional), parameter.location));
                    boolean knownConsumer = beans.values().stream().anyMatch(bean -> Objects.equals(bean.typeId, type.id));
                    if (chosen != null) {
                        String confidence = chosen.conditions.isEmpty() ? "FRAMEWORK_RESOLVED" : "STATIC_INFERRED";
                        graph.edge(edge(constructor.id, chosen.id, "SELECTED_BEAN", confidence,
                                mapOf("parameter", parameter.name, "required_type", required,
                                        "selection_reason", reason, "static_context_only", !chosen.conditions.isEmpty()),
                                parameter.location));
                        if (knownConsumer) graph.edge(edge(type.id, chosen.id, "INJECTS", confidence,
                                mapOf("injection_point", constructor.id, "parameter", parameter.name), parameter.location));
                    } else if (candidates.size() > 1 && !parameter.collection) {
                        diagnostics.add(diagnostic("AMBIGUOUS_BEAN_INJECTION", parameter.location, required,
                                "Multiple valid bean candidates remain; no bean was selected"));
                    }
                }
            }
        }
        for (BeanInfo ownerBean : beans.values().stream().filter(bean -> bean.factoryMethod != null).toList()) {
            MethodInfo factory = types.values().stream().flatMap(type -> type.methods.stream())
                    .filter(method -> method.id.equals(ownerBean.factoryMethod)).findFirst().orElse(null);
            if (factory == null) continue;
            for (ParameterInfo parameter : factory.parameters) {
                String required = parameter.containerElementType != null ? parameter.containerElementType : parameter.type;
                List<BeanInfo> candidates = beans.values().stream().filter(bean -> !bean.id.equals(ownerBean.id)
                        && bean.assignableTypes.contains(required)).toList();
                List<BeanInfo> primaries = candidates.stream().filter(bean -> bean.primary).toList();
                BeanInfo chosen = candidates.size() == 1 ? candidates.get(0)
                        : primaries.size() == 1 ? primaries.get(0) : null;
                String qualifier = qualifier(parameter.annotations);
                if (qualifier != null) chosen = candidates.stream().filter(bean -> bean.name.equals(qualifier)
                        || bean.qualifiers.contains(qualifier)).findFirst().orElse(null);
                for (BeanInfo candidate : candidates) graph.edge(edge(factory.id, candidate.id, "BEAN_CANDIDATE",
                        candidates.size() == 1 ? "FRAMEWORK_RESOLVED" : "AMBIGUOUS",
                        Map.of("parameter", parameter.name, "required_type", required), parameter.location));
                if (chosen != null) {
                    graph.edge(edge(factory.id, chosen.id, "SELECTED_BEAN", "FRAMEWORK_RESOLVED",
                            Map.of("parameter", parameter.name, "selection_reason", qualifier == null ? "FACTORY_PARAMETER" : "QUALIFIER"), parameter.location));
                    graph.edge(edge(ownerBean.id, chosen.id, "INJECTS", "FRAMEWORK_RESOLVED",
                            Map.of("factory_method", factory.id), parameter.location));
                }
            }
        }
    }

    private void deriveSupplementalSemantics(GraphSink graph) {
        for (TypeInfo type : types.values()) {
            Ann imported = type.annotations.values().stream().filter(value -> "Import".equals(value.simpleName()))
                    .findFirst().orElse(null);
            if (imported != null) for (String importedType : imported.list("value")) {
                TypeInfo target = types.get(importedType);
                if (target != null) graph.edge(edge(type.id, target.id, "IMPORTS_CONFIGURATION",
                        "FRAMEWORK_RESOLVED", Map.of(), type.location));
            }
            for (MethodInfo method : type.methods) {
                Ann transaction = method.annotations.values().stream()
                        .filter(value -> "Transactional".equals(value.simpleName())).findFirst().orElse(null);
                List<String> validation = validationNames(method.annotations);
                if (transaction == null && validation.isEmpty()) continue;
                Map<String, Object> metadata = new LinkedHashMap<>();
                if (transaction != null) metadata.put("transaction", mapOf(
                        "read_only", Optional.ofNullable(transaction.value("readOnly")).orElse(false),
                        "runtime_proxy", "not_confirmed"));
                if (!validation.isEmpty()) metadata.put("validation", validation);
                graph.node(node(method.id, "METHOD", type.qualifiedName + "#" + method.name, method.name,
                        method.location, "STATIC_INFERRED", metadata));
            }
        }
    }

    private void deriveRoutes(GraphSink graph) {
        Map<String, List<MethodInfo>> handlers = new LinkedHashMap<>();
        for (TypeInfo type : types.values()) {
            if (!type.annotations.containsKey("org.springframework.web.bind.annotation.RestController")
                    && !type.annotations.containsKey("org.springframework.stereotype.Controller")) continue;
            Mapping classMapping = mapping(type.annotations);
            List<String> prefixes = classMapping == null || classMapping.paths.isEmpty() ? List.of("") : classMapping.paths;
            for (MethodInfo method : type.methods) {
                Mapping mapping = mapping(method.annotations);
                if (mapping == null) continue;
                for (ParameterInfo parameter : method.parameters) {
                    TypeInfo dto = types.get(parameter.type);
                    if (dto == null) continue;
                    List<Map<String, Object>> constrained = dto.fields.stream()
                            .filter(field -> !validationNames(field.annotations).isEmpty())
                            .map(field -> mapOf("component", field.name, "java_type", field.type,
                                    "validation", validationNames(field.annotations))).toList();
                    if (!constrained.isEmpty()) graph.node(semanticTypeNode(dto, "TYPE", "FRAMEWORK_RESOLVED",
                            Map.of("validation_components", constrained)));
                }
                List<String> suffixes = mapping.paths.isEmpty() ? List.of("") : mapping.paths;
                Set<String> verbs = mapping.methods.isEmpty() ? Set.of("ANY") : mapping.methods;
                for (String verb : verbs) for (String prefix : prefixes) for (String suffix : suffixes) {
                    if ("ANY".equals(verb)) {
                        diagnostics.add(diagnostic("UNRESOLVED_REQUEST_MAPPING_VALUE", method.location,
                                method.name, "@RequestMapping does not declare an HTTP method"));
                        continue;
                    }
                    String path = normalizePath(prefix, suffix);
                    String routeId = "route:" + verb + ":" + path;
                    handlers.computeIfAbsent(routeId, ignored -> new ArrayList<>()).add(method);
                    Map<String, Object> metadata = mapOf("http_method", verb, "path", path,
                            "consumes", mapping.consumes, "produces", mapping.produces,
                            "params", mapping.params, "headers", mapping.headers,
                            "handler_parameters", method.parameters.stream().map(this::parameterMetadata).toList(),
                            "return_type", method.returnType,
                            "validation", validationNames(method.annotations));
                    Ann status = method.annotations.get("org.springframework.web.bind.annotation.ResponseStatus");
                    if (status != null) putIfNotNull(metadata, "response_status",
                            firstNonBlank(status.string("code"), status.string("value")));
                    graph.node(node(routeId, "ROUTE", verb + " " + path, method.name, method.location,
                            "FRAMEWORK_RESOLVED", metadata));
                    graph.edge(edge(type.id, routeId, "EXPOSES_ROUTE", "FRAMEWORK_RESOLVED", Map.of(), method.location));
                    graph.edge(edge(routeId, method.id, "HANDLED_BY", "FRAMEWORK_RESOLVED", Map.of(), method.location));
                }
            }
        }
        handlers.forEach((route, methods) -> {
            if (methods.size() > 1) {
                Location location = methods.get(0).location;
                graph.node(node(route, "ROUTE", route.substring(6).replaceFirst(":", " "), "conflicting route",
                        location, "AMBIGUOUS", Map.of("handlers", methods.stream().map(method -> method.id).toList())));
                diagnostics.add(diagnostic("DUPLICATE_ROUTE_MAPPING", location, route,
                        "Multiple handlers map to the same normalized route"));
            }
        });
    }

    private Map<String, RepositoryInfo> deriveRepositoriesAndEntities(GraphSink graph) {
        Map<String, TypeInfo> entities = types.values().stream()
                .filter(type -> type.annotations.containsKey("jakarta.persistence.Entity"))
                .collect(Collectors.toMap(type -> type.qualifiedName, type -> type, (a, b) -> a, LinkedHashMap::new));
        for (TypeInfo entity : entities.values()) {
            Ann entityAnn = entity.annotations.get("jakarta.persistence.Entity");
            Ann table = entity.annotations.get("jakarta.persistence.Table");
            graph.node(semanticTypeNode(entity, "ENTITY", "FRAMEWORK_RESOLVED", mapOf(
                    "entity_name", firstNonBlank(entityAnn.string("name"), entity.simpleName),
                    "table_name", table == null ? null : table.string("name"))));
            for (FieldInfo field : entity.fields) {
                Map<String, Object> metadata = new LinkedHashMap<>();
                metadata.put("field_type", field.type);
                metadata.put("id", field.annotations.containsKey("jakarta.persistence.Id")
                        || field.annotations.containsKey("jakarta.persistence.EmbeddedId"));
                Ann column = field.annotations.get("jakarta.persistence.Column");
                if (column != null) {
                    putIfNotNull(metadata, "column_name", column.string("name"));
                    putIfNotNull(metadata, "nullable", column.value("nullable"));
                    putIfNotNull(metadata, "unique", column.value("unique"));
                }
                String relation = relationship(field.annotations);
                if (relation != null) {
                    Ann ann = field.annotations.values().stream().filter(value -> relation.equals(value.simpleName())).findFirst().orElse(null);
                    metadata.put("relationship", relation);
                    putIfNotNull(metadata, "mapped_by", ann == null ? null : ann.string("mappedBy"));
                    putIfNotNull(metadata, "fetch", ann == null ? null : ann.string("fetch"));
                    metadata.put("cascade", ann == null ? List.of() : ann.list("cascade"));
                    metadata.put("owning_side", ann == null || ann.string("mappedBy") == null);
                }
                graph.node(node(field.id, "ENTITY_FIELD", entity.qualifiedName + "#" + field.name,
                        field.name, field.location, "FRAMEWORK_RESOLVED", metadata));
                if (relation != null) {
                    String targetType = field.containerElementType != null ? field.containerElementType : field.type;
                    TypeInfo target = entities.get(targetType);
                    if (target != null) graph.edge(edge(field.id, target.id, "RELATES_TO_ENTITY",
                            "FRAMEWORK_RESOLVED", metadata, field.location));
                    else diagnostics.add(diagnostic("UNRESOLVED_ENTITY_RELATION", field.location, targetType,
                            "Relationship target is not a resolved project entity"));
                }
            }
        }

        Map<String, RepositoryInfo> repositories = new LinkedHashMap<>();
        for (TypeInfo type : types.values()) {
            RepositoryTypes repositoryTypes = repositoryTypes(type.bindingInterfaces);
            if (repositoryTypes == null) continue;
            RepositoryInfo repository = new RepositoryInfo(type, repositoryTypes.domain, repositoryTypes.idType);
            repositories.put(type.qualifiedName, repository);
            graph.node(semanticTypeNode(type, "REPOSITORY", "FRAMEWORK_RESOLVED",
                    mapOf("domain_type", repository.domain, "id_type", repository.idType,
                            "framework_contract", repositoryTypes.contract)));
            TypeInfo entity = entities.get(repository.domain);
            if (entity != null) graph.edge(edge(type.id, entity.id, "REPOSITORY_FOR", "FRAMEWORK_RESOLVED",
                    Map.of("id_type", repository.idType), type.location));
            else diagnostics.add(diagnostic("UNRESOLVED_REPOSITORY_DOMAIN", type.location, repository.domain,
                    "Repository domain type is not a resolved @Entity"));
            emitInheritedRepositoryMethods(graph, repository);
            for (MethodInfo method : type.methods) deriveRepositoryMethod(graph, repository, method, entity);
        }
        return repositories;
    }

    /**
     * Adds framework-provided repository methods and their declaration edges to the graph.
     *
     * @param repository the repository whose inherited methods are emitted
     */
    private void emitInheritedRepositoryMethods(GraphSink graph, RepositoryInfo repository) {
        List<FrameworkMethod> methods = List.of(
                new FrameworkMethod("save", List.of(repository.domain), repository.domain, "WRITE"),
                new FrameworkMethod("findById", List.of(repository.idType), "java.util.Optional", "READ"),
                new FrameworkMethod("findAll", List.of(), "java.util.List", "READ"),
                new FrameworkMethod("deleteById", List.of(repository.idType), "void", "WRITE"),
                new FrameworkMethod("existsById", List.of(repository.idType), "boolean", "READ"),
                new FrameworkMethod("count", List.of(), "long", "READ"));
        for (FrameworkMethod method : methods) {
            String id = CallableIds.fallback(repository.type.qualifiedName, method.name, method.parameters, false);
            graph.node(node(id, "METHOD", repository.type.qualifiedName + "#" + method.name,
                    method.name, repository.type.location, "FRAMEWORK_RESOLVED", mapOf(
                            "declaring_type", repository.type.id, "parameter_types", method.parameters,
                            "return_type", method.returnType, "framework_provided", true,
                            "provenance_contract", "spring-data-contract", "entity_access", method.access)));
            graph.edge(edge(repository.type.id, id, "DECLARES", "FRAMEWORK_RESOLVED",
                    Map.of("framework_provided", true), repository.type.location));
        }
    }

    private void deriveRepositoryMethod(GraphSink graph, RepositoryInfo repository, MethodInfo method, TypeInfo entity) {
        Map<String, Object> metadata = new LinkedHashMap<>();
        metadata.put("spring_data_repository", true);
        metadata.put("domain_type", repository.domain);
        Ann query = method.annotations.get("org.springframework.data.jpa.repository.Query");
        boolean modifying = method.annotations.containsKey("org.springframework.data.jpa.repository.Modifying");
        if (query != null) {
            String raw = bounded(query.string("value"), 1000);
            putIfNotNull(metadata, "query", raw);
            metadata.put("modifying", modifying);
            metadata.put("query_kind", modifying ? "WRITE" : "READ");
        } else {
            DerivedQuery derived = parseDerivedQuery(method.name, entity);
            if (derived != null) metadata.putAll(derived.metadata);
            else if (method.name.matches("(?i)(find|read|get|exists|count|delete).*By.*"))
                diagnostics.add(diagnostic("UNPARSEABLE_DERIVED_QUERY", method.location, method.name,
                        "Derived query method is outside the supported bounded grammar"));
        }
        graph.node(node(method.id, "METHOD", repository.type.qualifiedName + "#" + method.name,
                method.name, method.location, "FRAMEWORK_RESOLVED", metadata));
    }

    private void deriveRepositoryCalls(GraphSink graph, Map<String, RepositoryInfo> repositories) {
        for (CallInfo call : calls) {
            RepositoryInfo repository = repositories.get(call.receiverType);
            if (repository == null) continue;
            graph.edge(edge(call.sourceMethod, repository.type.id, "USES_REPOSITORY", "COMPILER_RESOLVED",
                    Map.of("method", call.methodName), call.location));
            String access = repositoryAccess(call.methodName, call.annotations);
            if (access != null) {
                TypeInfo entity = types.get(repository.domain);
                if (entity != null) graph.edge(edge(call.sourceMethod, entity.id,
                        "WRITE".equals(access) ? "WRITES_ENTITY" : "READS_ENTITY",
                        call.conditional ? "STATIC_INFERRED" : "FRAMEWORK_RESOLVED",
                        Map.of("repository_method", call.methodName), call.location));
            }
        }
    }

    private void deriveConfiguration(GraphSink graph) {
        Set<String> emitted = new LinkedHashSet<>();
        for (TypeInfo type : types.values()) {
            Ann properties = type.annotations.get("org.springframework.boot.context.properties.ConfigurationProperties");
            String prefix = properties == null ? null : firstNonBlank(properties.string("prefix"), properties.string("value"));
            for (FieldInfo field : type.fields) {
                Ann value = field.annotations.get("org.springframework.beans.factory.annotation.Value");
                if (value != null) {
                    Matcher matcher = PROPERTY_PLACEHOLDER.matcher(Optional.ofNullable(value.string("value")).orElse(""));
                    if (matcher.find()) emitConfig(graph, emitted, matcher.group(1), field.id, field.location, null);
                    else diagnostics.add(diagnostic("UNRESOLVED_CONFIG_PROPERTY", field.location, field.name,
                            "@Value does not contain a statically resolvable property placeholder"));
                }
                if (prefix != null) emitConfig(graph, emitted, prefix + "." + kebab(field.name), field.id, field.location, null);
            }
        }
        scanConfigurationFiles(graph, emitted);
    }

    private void emitConfig(GraphSink graph, Set<String> emitted, String key, String source, Location location, String profile) {
        String normalized = key.trim().toLowerCase(Locale.ROOT);
        String id = "config:" + normalized;
        if (emitted.add(id)) graph.node(node(id, "CONFIG_PROPERTY", normalized, normalized, location,
                "FRAMEWORK_RESOLVED", mapOf("key", normalized, "profile", profile,
                        "value_stored", false, "secret_like", SECRET_KEY.matcher(normalized).matches())));
        if (source != null) graph.edge(edge(source, id, "CONFIGURED_BY", "FRAMEWORK_RESOLVED", Map.of(), location));
    }

    private void scanConfigurationFiles(GraphSink graph, Set<String> emitted) {
        try (var stream = Files.walk(projectRoot)) {
            stream.filter(Files::isRegularFile).filter(path -> {
                String name = path.getFileName().toString();
                return name.matches("application(?:-[A-Za-z0-9_.-]+)?\\.(?:properties|ya?ml)");
            }).sorted().forEach(path -> {
                String name = path.getFileName().toString();
                Matcher profileMatcher = Pattern.compile("application-([^.]+)\\.").matcher(name);
                String profile = profileMatcher.find() ? profileMatcher.group(1) : null;
                try {
                    List<String> lines = Files.readAllLines(path, StandardCharsets.UTF_8);
                    if (name.endsWith(".properties")) {
                        for (int index = 0; index < lines.size(); index++) {
                            String line = lines.get(index).trim();
                            if (line.isEmpty() || line.startsWith("#") || line.startsWith("!")) continue;
                            int separator = line.indexOf('=');
                            if (separator < 0) separator = line.indexOf(':');
                            if (separator > 0) emitConfig(graph, emitted, line.substring(0, separator), null,
                                    location(path, index + 1, index + 1), profile);
                        }
                    } else scanYamlKeys(graph, emitted, path, lines, profile);
                } catch (IOException error) {
                    diagnostics.add(diagnostic("UNRESOLVED_CONFIG_PROPERTY", location(path, 1, 1), name,
                            "Configuration keys could not be read"));
                }
            });
        } catch (IOException ignored) {
            // Project source discovery already reports inaccessible trees; avoid duplicate unbounded diagnostics.
        }
    }

    private void scanYamlKeys(GraphSink graph, Set<String> emitted, Path path, List<String> lines, String profile) {
        Deque<YamlPart> stack = new ArrayDeque<>();
        for (int index = 0; index < lines.size(); index++) {
            String raw = lines.get(index);
            String trimmed = raw.trim();
            if (trimmed.isEmpty() || trimmed.startsWith("#") || trimmed.startsWith("-") || !trimmed.contains(":")) continue;
            int indent = raw.length() - raw.stripLeading().length();
            while (!stack.isEmpty() && stack.peek().indent >= indent) stack.pop();
            String key = trimmed.substring(0, trimmed.indexOf(':')).trim();
            if (key.isEmpty() || key.contains(" ")) continue;
            List<String> parts = new ArrayList<>();
            stack.descendingIterator().forEachRemaining(part -> parts.add(part.key));
            parts.add(key);
            emitConfig(graph, emitted, String.join(".", parts), null, location(path, index + 1, index + 1), profile);
            if (trimmed.substring(trimmed.indexOf(':') + 1).trim().isEmpty()) stack.push(new YamlPart(indent, key));
        }
    }

    private void deriveEvents(GraphSink graph) {
        for (TypeInfo type : types.values()) for (MethodInfo method : type.methods) {
            if (method.annotations.containsKey("org.springframework.context.event.EventListener")
                    && !method.parameters.isEmpty()) {
                String eventType = method.parameters.get(0).type;
                if (eventType == null || eventType.isBlank()) {
                    diagnostics.add(diagnostic("UNRESOLVED_EVENT_TYPE", method.location, method.name,
                            "Event listener parameter type is unavailable"));
                    continue;
                }
                TypeInfo event = types.get(eventType);
                if (event != null) graph.node(semanticTypeNode(event, "EVENT_TYPE", "FRAMEWORK_RESOLVED",
                        Map.of("listener_count", 1)));
                else graph.node(node("type:" + eventType, "EVENT_TYPE", eventType, simpleName(eventType), null,
                        "STATIC_INFERRED", Map.of("external", true, "listener_count", 1)));
                graph.edge(edge(method.id, "type:" + eventType, "LISTENS_TO_EVENT", "FRAMEWORK_RESOLVED",
                        mapOf("async", method.annotations.containsKey("org.springframework.scheduling.annotation.Async"),
                                "runtime_delivery", "not_confirmed"), method.location));
            }
        }
        for (CallInfo call : calls) if ("org.springframework.context.ApplicationEventPublisher".equals(call.receiverType)
                && "publishEvent".equals(call.methodName)) {
            if (call.argumentType == null || call.argumentType.isBlank()) {
                diagnostics.add(diagnostic("UNRESOLVED_EVENT_TYPE", call.location, call.methodName,
                        "Event publisher argument type is unavailable"));
                continue;
            }
            TypeInfo event = types.get(call.argumentType);
            if (event != null) graph.node(semanticTypeNode(event, "EVENT_TYPE", "FRAMEWORK_RESOLVED", Map.of()));
            else graph.node(node("type:" + call.argumentType, "EVENT_TYPE", call.argumentType,
                    simpleName(call.argumentType), null, "STATIC_INFERRED", Map.of("external", true)));
            graph.edge(edge(call.sourceMethod, "type:" + call.argumentType, "PUBLISHES_EVENT",
                    "FRAMEWORK_RESOLVED", Map.of("runtime_delivery", "not_confirmed"), call.location));
        }
    }

    private DerivedQuery parseDerivedQuery(String name, TypeInfo entity) {
        Matcher matcher = Pattern.compile("^(find|read|get|exists|count|delete)By(.+)$").matcher(name);
        if (!matcher.matches() || entity == null) return null;
        String expression = matcher.group(2);
        String order = null;
        int orderIndex = expression.indexOf("OrderBy");
        if (orderIndex >= 0) { order = expression.substring(orderIndex + 7); expression = expression.substring(0, orderIndex); }
        List<Map<String, Object>> predicates = new ArrayList<>();
        for (String part : expression.split("And|Or")) {
            String operator = "EQUALS";
            for (String suffix : List.of("IsNotNull", "IsNull", "StartsWith", "EndsWith", "Containing",
                    "GreaterThan", "LessThan", "Between", "True", "False", "In", "IgnoreCase")) {
                if (part.endsWith(suffix)) { operator = camelToUpperSnake(suffix); part = part.substring(0, part.length() - suffix.length()); break; }
            }
            String field = Introspector.decapitalize(part);
            FieldInfo resolved = entity.fields.stream().filter(candidate -> candidate.name.equals(field)).findFirst().orElse(null);
            if (resolved == null) return null;
            predicates.add(mapOf("field", field, "operator", operator));
        }
        Map<String, Object> metadata = mapOf("derived_query", true, "operation", matcher.group(1).toUpperCase(Locale.ROOT),
                "predicates", predicates);
        if (order != null) {
            String direction = order.endsWith("Desc") ? "DESC" : "ASC";
            String field = order.replaceFirst("(Asc|Desc)$", "");
            metadata.put("order_by", mapOf("field", Introspector.decapitalize(field), "direction", direction));
        }
        return new DerivedQuery(metadata);
    }

    private Stereotype stereotype(TypeInfo type, Set<String> seen) {
        for (Ann ann : type.annotations.values()) {
            if (STEREOTYPES.contains(ann.name)) return new Stereotype(ann.simpleName(), ann, List.of(ann.name));
            if (!seen.add(ann.name)) continue;
            TypeInfo annotationType = types.get(ann.name);
            if (annotationType != null) {
                Stereotype nested = stereotype(annotationType, seen);
                if (nested != null) {
                    List<String> chain = new ArrayList<>(); chain.add(ann.name); chain.addAll(nested.chain);
                    return new Stereotype(nested.kind, ann, chain);
                }
            }
        }
        return null;
    }

    private static List<Map<String, Object>> conditions(Map<String, Ann> annotations) {
        List<Map<String, Object>> result = new ArrayList<>();
        Ann profile = annotations.get("org.springframework.context.annotation.Profile");
        if (profile != null) result.add(mapOf("kind", "PROFILE", "expression", profile.list("value"), "evaluation", "UNKNOWN"));
        for (String name : List.of("ConditionalOnProperty", "ConditionalOnMissingBean", "ConditionalOnBean")) {
            Ann ann = annotations.values().stream().filter(value -> name.equals(value.simpleName())).findFirst().orElse(null);
            if (ann != null) result.add(mapOf("kind", camelToUpperSnake(name.replace("ConditionalOn", "")),
                    "attributes", ann.values, "evaluation", "UNKNOWN"));
        }
        return result;
    }

    private static Mapping mapping(Map<String, Ann> annotations) {
        for (Ann ann : annotations.values()) {
            String method = switch (ann.simpleName()) {
                case "GetMapping" -> "GET"; case "PostMapping" -> "POST"; case "PutMapping" -> "PUT";
                case "PatchMapping" -> "PATCH"; case "DeleteMapping" -> "DELETE"; default -> null;
            };
            if (method != null) return new Mapping(nonEmpty(ann.list("path"), ann.list("value")), Set.of(method),
                    ann.list("consumes"), ann.list("produces"), ann.list("params"), ann.list("headers"));
            if ("RequestMapping".equals(ann.simpleName())) {
                Set<String> methods = ann.list("method").stream().map(value -> value.substring(value.lastIndexOf('.') + 1)).collect(Collectors.toCollection(LinkedHashSet::new));
                return new Mapping(nonEmpty(ann.list("path"), ann.list("value")), methods,
                        ann.list("consumes"), ann.list("produces"), ann.list("params"), ann.list("headers"));
            }
        }
        return null;
    }

    private Map<String, Object> parameterMetadata(ParameterInfo parameter) {
        String binding = "JAVA";
        String externalName = parameter.name;
        boolean required = !parameter.optional;
        for (Ann ann : parameter.annotations.values()) {
            binding = switch (ann.simpleName()) {
                case "RequestBody" -> "REQUEST_BODY"; case "PathVariable" -> "PATH_VARIABLE";
                case "RequestParam" -> "REQUEST_PARAM"; case "RequestHeader" -> "REQUEST_HEADER";
                case "ModelAttribute" -> "MODEL_ATTRIBUTE"; default -> binding;
            };
            String explicit = firstNonBlank(ann.string("name"), ann.string("value"));
            if (explicit != null) externalName = explicit;
            Object explicitRequired = ann.value("required");
            if (explicitRequired instanceof Boolean bool) required = bool;
        }
        return mapOf("java_type", parameter.type, "binding_kind", binding, "external_name", externalName,
                "required", required, "validation", validationNames(parameter.annotations));
    }

    private static RepositoryTypes repositoryTypes(List<InterfaceInfo> interfaces) {
        for (InterfaceInfo info : interfaces) if (REPOSITORY_BASES.contains(info.name) && info.arguments.size() >= 2)
            return new RepositoryTypes(info.arguments.get(0), info.arguments.get(1), info.name);
        return null;
    }

    private static String repositoryAccess(String method, Map<String, Ann> annotations) {
        if (annotations.containsKey("org.springframework.data.jpa.repository.Modifying")) return "WRITE";
        if (method.startsWith("save") || method.startsWith("delete") || method.startsWith("remove")) return "WRITE";
        if (method.startsWith("find") || method.startsWith("read") || method.startsWith("get")
                || method.startsWith("exists") || method.startsWith("count")) return "READ";
        return null;
    }

    private static String relationship(Map<String, Ann> annotations) {
        return annotations.values().stream().map(Ann::simpleName)
                .filter(Set.of("OneToOne", "OneToMany", "ManyToOne", "ManyToMany")::contains).findFirst().orElse(null);
    }

    private GraphNode semanticTypeNode(TypeInfo type, String kind, String confidence, Map<String, Object> metadata) {
        return new GraphNode(type.id, kind, type.qualifiedName, type.simpleName, type.moduleName, type.packageName,
                type.location.file, type.location.start, type.location.end, confidence, PROVENANCE, metadata, List.of());
    }

    private GraphNode node(String id, String kind, String qualified, String simple, Location location,
                           String confidence, Map<String, Object> metadata) {
        return new GraphNode(id, kind, qualified, simple, location == null ? null : location.module,
                location == null ? null : location.packageName, location == null ? null : location.file,
                location == null ? null : location.start, location == null ? null : location.end,
                confidence, PROVENANCE, metadata, List.of());
    }

    private static GraphEdge edge(String source, String target, String kind, String confidence,
                                  Map<String, Object> metadata, Location location) {
        List<EdgeOccurrence> occurrences = location == null ? List.of()
                : List.of(new EdgeOccurrence(location.file, location.start, location.end, Map.of()));
        return new GraphEdge(source, target, kind, confidence, PROVENANCE, metadata, occurrences);
    }

    private Diagnostic diagnostic(String kind, Location location, String symbol, String reason) {
        return new Diagnostic(kind, location == null ? null : location.file, location == null ? null : location.start,
                location == null ? null : location.end, bounded(symbol, 200), reason, "warning");
    }

    private Location location(Path file, int start, int end) {
        return new Location(relative(file), start, end, null, null);
    }

    private String relative(Path file) { return projectRoot.relativize(file).toString().replace('\\', '/'); }

    private final class Collector extends ASTVisitor {
        private final CompilationUnit unit;
        private final Path file;
        private final SourceRoot sourceRoot;
        private final Deque<TypeInfo> typeStack = new ArrayDeque<>();
        private final Deque<MethodInfo> methodStack = new ArrayDeque<>();
        private final Map<String, String> imports = new LinkedHashMap<>();
        private String packageName = "";

        Collector(CompilationUnit unit, Path file, SourceRoot sourceRoot) {
            super(true); this.unit = unit; this.file = file; this.sourceRoot = sourceRoot;
            if (unit.getPackage() != null) packageName = unit.getPackage().getName().getFullyQualifiedName();
            for (Object value : unit.imports()) {
                ImportDeclaration declaration = (ImportDeclaration) value;
                if (!declaration.isStatic() && !declaration.isOnDemand()) {
                    String name = declaration.getName().getFullyQualifiedName();
                    imports.put(name.substring(name.lastIndexOf('.') + 1), name);
                }
            }
        }

        @Override public boolean visit(TypeDeclaration node) { return enterType(node, node.resolveBinding()); }
        @Override public void endVisit(TypeDeclaration node) { typeStack.pop(); }
        @Override public boolean visit(EnumDeclaration node) { return enterType(node, node.resolveBinding()); }
        @Override public void endVisit(EnumDeclaration node) { typeStack.pop(); }
        @Override public boolean visit(RecordDeclaration node) { return enterType(node, node.resolveBinding()); }
        @Override public void endVisit(RecordDeclaration node) { typeStack.pop(); }
        @Override public boolean visit(AnnotationTypeDeclaration node) { return enterType(node, node.resolveBinding()); }
        @Override public void endVisit(AnnotationTypeDeclaration node) { typeStack.pop(); }

        /**
         * Registers a type declaration and prepares it for nested AST traversal.
         *
         * @param declaration the type declaration to register
         * @param binding the resolved type binding, or {@code null} when unavailable
         * @return {@code true} after the type has been registered
         */
        private boolean enterType(AbstractTypeDeclaration declaration, ITypeBinding binding) {
            String simpleName = declaration.getName().getIdentifier();
            String lexicalName = typeStack.isEmpty() ? (packageName.isBlank() ? simpleName : packageName + "." + simpleName)
                    : typeStack.peek().qualifiedName + "." + simpleName;
            String qualified = binding == null ? lexicalName : normalize(binding);
            TypeInfo type = new TypeInfo("type:" + qualified, qualified, declaration.getName().getIdentifier(),
                    packageName, sourceRoot.moduleName(), location(declaration), annotations(declaration.modifiers()),
                    binding != null && binding.isInterface());
            if (binding != null) {
                collectTypes(binding, type.allTypes, new LinkedHashSet<>());
                collectInterfaces(binding, type.bindingInterfaces, new LinkedHashSet<>());
            } else type.allTypes.add(qualified);
            if (declaration instanceof TypeDeclaration declared) for (Object value : declared.superInterfaceTypes())
                addTextInterface(type, (Type) value);
            if (declaration instanceof RecordDeclaration record) for (Object value : record.recordComponents()) {
                SingleVariableDeclaration component = (SingleVariableDeclaration) value;
                ITypeBinding componentType = component.getType().resolveBinding();
                String fieldType = typeName(componentType, component.getType().toString());
                type.fields.add(new FieldInfo("field:" + qualified + "#" + component.getName(),
                        component.getName().getIdentifier(), fieldType, containerElement(componentType),
                        annotations(component.modifiers()), location(component)));
            }
            if (declaration instanceof RecordDeclaration record) {
                List<ParameterInfo> components = new ArrayList<>();
                for (Object value : record.recordComponents()) {
                    SingleVariableDeclaration component = (SingleVariableDeclaration) value;
                    ITypeBinding componentType = component.getType().resolveBinding();
                    components.add(new ParameterInfo(component.getName().getIdentifier(),
                            typeName(componentType, component.getType().toString()), containerElement(componentType),
                            isCollection(componentType), isOptional(componentType),
                            annotations(component.modifiers()), location(component)));
                }
                type.methods.add(new MethodInfo(CallableIds.fallback(qualified, "<init>",
                        components.stream().map(parameter -> parameter.type).toList(), true), "<init>", true,
                        "void", components, Map.of(), location(record), true));
            }
            types.put(qualified, type); typeStack.push(type); return true;
        }

        private void addTextInterface(TypeInfo owner, Type syntax) {
            Type raw = syntax;
            List<String> arguments = List.of();
            if (syntax instanceof ParameterizedType parameterized) {
                raw = parameterized.getType();
                arguments = parameterized.typeArguments().stream().map(value -> qualifyTypeText(value.toString())).toList();
            }
            String name = qualifyTypeText(raw.toString());
            if (owner.bindingInterfaces.stream().noneMatch(info -> info.name.equals(name)))
                owner.bindingInterfaces.add(new InterfaceInfo(name, arguments));
        }

        /**
         * Collects a method or constructor declaration and records its parameters, metadata, return type, and simple factory implementation type.
         *
         * @param declaration the method or constructor declaration to collect
         * @return true to continue traversing the declaration
         */
        @Override public boolean visit(MethodDeclaration declaration) {
            if (typeStack.isEmpty()) return true;
            IMethodBinding binding = declaration.resolveBinding();
            boolean constructor = declaration.isConstructor();
            List<ParameterInfo> parameters = new ArrayList<>();
            for (Object value : declaration.parameters()) {
                SingleVariableDeclaration parameter = (SingleVariableDeclaration) value;
                IVariableBinding variable = parameter.resolveBinding();
                ITypeBinding type = variable == null ? parameter.getType().resolveBinding() : variable.getType();
                parameters.add(new ParameterInfo(parameter.getName().getIdentifier(), typeName(type, parameter.getType().toString()),
                        containerElement(type), isCollection(type), isOptional(type), annotations(parameter.modifiers()), location(parameter)));
            }
            CallableIds.CallableSymbol callable = CallableIds.forDeclaration(binding,
                    typeStack.peek().qualifiedName, declaration);
            String id = callable.id();
            IMethodBinding canonical = callable.resolved() == null ? null : callable.resolved().declaration();
            String returnType = constructor ? "void" : typeName(canonical == null
                            ? declaration.getReturnType2().resolveBinding() : canonical.getReturnType(),
                    declaration.getReturnType2() == null ? "void" : declaration.getReturnType2().toString());
            MethodInfo method = new MethodInfo(id, declaration.getName().getIdentifier(), constructor, returnType,
                    parameters, annotations(declaration.modifiers()), location(declaration), false);
            if (!constructor && declaration.getBody() != null && declaration.getBody().statements().size() == 1
                    && declaration.getBody().statements().get(0) instanceof ReturnStatement statement) {
                Expression expression = statement.getExpression();
                if (expression instanceof ClassInstanceCreation creation) method.beanImplementationType = typeName(creation.resolveTypeBinding(), creation.getType().toString());
            }
            typeStack.peek().methods.add(method); methodStack.push(method); return true;
        }

        @Override public void endVisit(MethodDeclaration node) { methodStack.pop(); }

        @Override public boolean visit(FieldDeclaration declaration) {
            if (typeStack.isEmpty()) return true;
            Map<String, Ann> annotations = annotations(declaration.modifiers());
            for (Object value : declaration.fragments()) {
                VariableDeclarationFragment fragment = (VariableDeclarationFragment) value;
                IVariableBinding binding = fragment.resolveBinding();
                ITypeBinding type = binding == null ? declaration.getType().resolveBinding() : binding.getType();
                typeStack.peek().fields.add(new FieldInfo("field:" + typeStack.peek().qualifiedName + "#" + fragment.getName(),
                        fragment.getName().getIdentifier(), typeName(type, declaration.getType().toString()),
                        containerElement(type), annotations, location(fragment)));
            }
            return true;
        }

        @Override public boolean visit(MethodInvocation invocation) {
            if (methodStack.isEmpty()) return true;
            IMethodBinding binding = invocation.resolveMethodBinding();
            CallableIds.ResolvedMethod resolved = CallableIds.resolve(binding);
            ITypeBinding receiver = invocation.getExpression() == null
                    ? (resolved == null ? null : resolved.owner()) : invocation.getExpression().resolveTypeBinding();
            ITypeBinding argument = invocation.arguments().isEmpty() ? null : ((Expression) invocation.arguments().get(0)).resolveTypeBinding();
            calls.add(new CallInfo(methodStack.peek().id, typeName(receiver, null), invocation.getName().getIdentifier(),
                    typeName(argument, null), resolved == null ? Map.of() : bindingAnnotations(resolved.declaration()), location(invocation),
                    isConditional(invocation)));
            return true;
        }

        private Map<String, Ann> annotations(List<?> modifiers) {
            Map<String, Ann> result = new LinkedHashMap<>();
            for (Object value : modifiers) if (value instanceof Annotation annotation) {
                Ann ann = ann(annotation); result.put(ann.name, ann);
            }
            return result;
        }

        private Ann ann(Annotation annotation) {
            IAnnotationBinding binding = annotation.resolveAnnotationBinding();
            String name = binding == null || binding.getAnnotationType().isRecovered()
                    ? qualifyTypeText(annotation.getTypeName().getFullyQualifiedName())
                    : normalize(binding.getAnnotationType());
            Map<String, Object> values = new LinkedHashMap<>();
            if (binding != null) for (IMemberValuePairBinding pair : binding.getDeclaredMemberValuePairs())
                values.put(pair.getName(), annotationValue(pair.getValue()));
            if (values.isEmpty()) values.putAll(syntaxAnnotationValues(annotation));
            return new Ann(name, values);
        }

        private Map<String, Object> syntaxAnnotationValues(Annotation annotation) {
            Map<String, Object> result = new LinkedHashMap<>();
            if (annotation instanceof SingleMemberAnnotation single) result.put("value", syntaxValue(single.getValue()));
            else if (annotation instanceof NormalAnnotation normal) for (Object value : normal.values()) {
                MemberValuePair pair = (MemberValuePair) value;
                result.put(pair.getName().getIdentifier(), syntaxValue(pair.getValue()));
            }
            return result;
        }

        private Object syntaxValue(Expression expression) {
            if (expression instanceof StringLiteral literal) return literal.getLiteralValue();
            if (expression instanceof BooleanLiteral literal) return literal.booleanValue();
            if (expression instanceof ArrayInitializer array) return array.expressions().stream()
                    .map(value -> syntaxValue((Expression) value)).toList();
            if (expression instanceof Name name) return name.getFullyQualifiedName();
            return expression.toString();
        }

        private String typeName(ITypeBinding binding, String fallback) {
            String value = binding != null && binding.isRecovered() ? binding.getName() : normalize(binding);
            if (value == null || value.isBlank() || !value.contains(".")) value = qualifyTypeText(value == null ? fallback : value);
            return value;
        }

        private String qualifyTypeText(String text) {
            if (text == null) return null;
            String value = text.replaceAll("\\s+", "").replace("...", "[]");
            int generic = value.indexOf('<');
            if (generic >= 0) value = value.substring(0, generic);
            String suffix = value.endsWith("[]") ? "[]" : "";
            if (!suffix.isEmpty()) value = value.substring(0, value.length() - 2);
            if (value.contains(".")) return value + suffix;
            String imported = imports.get(value);
            if (imported != null) return imported + suffix;
            if (Set.of("void", "boolean", "byte", "short", "int", "long", "float", "double", "char").contains(value)) return value + suffix;
            return (packageName.isBlank() ? value : packageName + "." + value) + suffix;
        }

        private Location location(ASTNode node) {
            int start = Math.max(1, unit.getLineNumber(node.getStartPosition()));
            int end = Math.max(start, unit.getLineNumber(node.getStartPosition() + Math.max(0, node.getLength() - 1)));
            return new Location(relative(file), start, end, sourceRoot.moduleName(), packageName);
        }
    }

    private static Map<String, Ann> bindingAnnotations(IMethodBinding binding) {
        Map<String, Ann> result = new LinkedHashMap<>();
        for (IAnnotationBinding annotation : binding.getAnnotations()) {
            Map<String, Object> values = new LinkedHashMap<>();
            for (IMemberValuePairBinding pair : annotation.getDeclaredMemberValuePairs()) values.put(pair.getName(), annotationValue(pair.getValue()));
            Ann ann = new Ann(normalize(annotation.getAnnotationType()), values); result.put(ann.name, ann);
        }
        return result;
    }

    private static Object annotationValue(Object value) {
        if (value instanceof Object[] array) return Arrays.stream(array).map(SpringSemanticAnalyzer::annotationValue).toList();
        if (value instanceof IVariableBinding variable) return variable.getName();
        if (value instanceof ITypeBinding type) return normalize(type);
        if (value instanceof IAnnotationBinding annotation) return normalize(annotation.getAnnotationType());
        return value == null ? null : value;
    }

    private static void collectTypes(ITypeBinding binding, Set<String> result, Set<String> seen) {
        if (binding == null || !seen.add(binding.getKey())) return;
        result.add(normalize(binding)); collectTypes(binding.getSuperclass(), result, seen);
        for (ITypeBinding iface : binding.getInterfaces()) collectTypes(iface, result, seen);
    }

    private static void collectInterfaces(ITypeBinding binding, List<InterfaceInfo> result, Set<String> seen) {
        if (binding == null || !seen.add(binding.getKey())) return;
        for (ITypeBinding iface : binding.getInterfaces()) {
            result.add(new InterfaceInfo(normalize(iface), Arrays.stream(iface.getTypeArguments()).map(SpringSemanticAnalyzer::normalize).toList()));
            collectInterfaces(iface, result, seen);
        }
        collectInterfaces(binding.getSuperclass(), result, seen);
    }

    /**
     * Normalizes a type binding to its qualified source-style name.
     *
     * @param binding the type binding to normalize
     * @return the normalized type name, including array dimensions, or {@code null} if the binding is {@code null}
     */
    private static String normalize(ITypeBinding binding) {
        if (binding == null) return null;
        if (binding.isArray()) return normalize(binding.getElementType()) + "[]".repeat(binding.getDimensions());
        ITypeBinding normalized = binding.isPrimitive() ? binding : binding.getErasure();
        String name = normalized.getQualifiedName();
        if (name == null || name.isBlank()) name = normalized.getName();
        return name.replace('$', '.');
    }

    private static String normalizeOrText(ITypeBinding binding, String fallback) {
        String value = normalize(binding); return value == null || value.isBlank() ? fallback.replaceAll("\\s+", "") : value;
    }

    /**
     * Determines the element type of a single-parameter container type.
     *
     * @param type the type to inspect
     * @return the normalized element type when the type is a supported container with one type argument; otherwise, {@code null}
     */
    private static String containerElement(ITypeBinding type) {
        if (type == null || type.getTypeArguments().length != 1) return null;
        String raw = normalize(type);
        return isContainer(raw) ? normalize(type.getTypeArguments()[0]) : null;
    }

    /**
     * Determines whether a type represents a supported container or provider.
     *
     * @param type the fully qualified type name
     * @return {@code true} if the type is a supported container or provider, {@code false} otherwise
     */
    static boolean isContainer(String type) {
        return type != null && Set.of(
                "java.util.List",
                "java.util.Set",
                "java.util.Collection",
                "java.util.Optional",
                "org.springframework.beans.factory.ObjectProvider",
                "jakarta.inject.Provider"
        ).contains(type);
    }

    /**
     * Determines whether a type represents a supported collection type.
     *
     * @param type the type to examine
     * @return {@code true} if the type is a list, set, or collection, {@code false} otherwise
     */
    static boolean isCollection(ITypeBinding type) {
        String raw = normalize(type);
        return raw != null && Set.of(
                "java.util.List",
                "java.util.Set",
                "java.util.Collection"
        ).contains(raw);
    }

    /**
     * Determines whether a type represents an optional or provider-style dependency.
     *
     * @param type the type to inspect
     * @return {@code true} if the type is {@code Optional}, {@code ObjectProvider}, or {@code Provider}; {@code false} otherwise
     */
    static boolean isOptional(ITypeBinding type) {
        String raw = normalize(type);
        return raw != null && ("java.util.Optional".equals(raw)
                || "org.springframework.beans.factory.ObjectProvider".equals(raw)
                || "jakarta.inject.Provider".equals(raw));
    }

    /**
     * Determines whether a node occurs within a conditional or lambda expression context.
     *
     * @param node the node to inspect
     * @return {@code true} if an enclosing conditional statement, conditional expression, or lambda expression is found; {@code false} otherwise
     */
    private static boolean isConditional(ASTNode node) {
        for (ASTNode parent = node.getParent(); parent != null; parent = parent.getParent())
            if (parent instanceof IfStatement || parent instanceof ConditionalExpression || parent instanceof LambdaExpression) return true;
        return false;
    }

    private static String qualifier(Map<String, Ann> annotations) {
        Ann qualifier = annotations.values().stream()
                .filter(value -> "Qualifier".equals(value.simpleName())).findFirst().orElse(null);
        return qualifier == null ? null : qualifier.string("value");
    }

    private static Set<String> qualifiers(Map<String, Ann> annotations) {
        String qualifier = qualifier(annotations); return qualifier == null ? Set.of() : Set.of(qualifier);
    }

    private static List<String> validationNames(Map<String, Ann> annotations) {
        Set<String> simple = VALIDATION.stream().map(value -> value.substring(value.lastIndexOf('.') + 1)).collect(Collectors.toSet());
        return annotations.values().stream().filter(value -> VALIDATION.contains(value.name)
                        || simple.contains(value.simpleName())).map(Ann::name).sorted().toList();
    }

    private static List<String> nonEmpty(List<String> first, List<String> second) { return first.isEmpty() ? second : first; }
    /**
     * Combines path segments into a normalized path with a single leading slash.
     *
     * @param prefix the first path segment
     * @param suffix the second path segment
     * @return the normalized path without a trailing slash, except for the root path
     */
    private static String normalizePath(String prefix, String suffix) {
        String path = ("/" + Optional.ofNullable(prefix).orElse("") + "/" + Optional.ofNullable(suffix).orElse(""))
                .replaceAll("/+", "/");
        return path.length() > 1 && path.endsWith("/") ? path.substring(0, path.length() - 1) : path;
    }
    /**
     * Selects the first non-blank value from the provided values.
     *
     * @param values candidate values to inspect
     * @return the first non-blank value, or {@code null} if none is available
     */
    private static String firstNonBlank(String... values) {
        for (String value : values) {
            if (value != null && !value.isBlank()) {
                return value;
            }
        }
        return null;
    }

    /**
     * Extracts the final segment from a dot-delimited name.
     *
     * @param value the dot-delimited name
     * @return the substring after the final dot
     */
    private static String simpleName(String value) {
        return value.substring(value.lastIndexOf('.') + 1);
    }

    /**
     * Converts camelCase text to lowercase kebab-case.
     *
     * @param value the text to convert
     * @return the converted text
     */
    private static String kebab(String value) {
        return value.replaceAll("([a-z0-9])([A-Z])", "$1-$2").toLowerCase(Locale.ROOT);
    }

    /**
     * Converts a camel-case string to uppercase snake case.
     *
     * @param value the camel-case string to convert
     * @return the uppercase snake-case representation
     */
    private static String camelToUpperSnake(String value) {
        return value.replaceAll("([a-z0-9])([A-Z])", "$1_$2").toUpperCase(Locale.ROOT);
    }

    /**
     * Restricts a string to the specified maximum length.
     *
     * @param value the string to limit
     * @param limit the maximum number of characters
     * @return the original string if it fits within the limit, the truncated string otherwise, or {@code null} if the value is {@code null}
     */
    private static String bounded(String value, int limit) {
        return value == null ? null : value.length() <= limit ? value : value.substring(0, limit);
    }

    private static Map<String, Object> mapOf(Object... values) {
        Map<String, Object> result = new LinkedHashMap<>();
        for (int index = 0; index < values.length; index += 2) if (values[index + 1] != null)
            result.put((String) values[index], values[index + 1]);
        return result;
    }

    private static void putIfNotNull(Map<String, Object> target, String key, Object value) {
        if (value != null) target.put(key, value);
    }

    private static final class TypeInfo {
        final String id, qualifiedName, simpleName, packageName, moduleName;
        final Location location; final Map<String, Ann> annotations; final boolean interfaceType;
        final Set<String> allTypes = new LinkedHashSet<>(); final List<InterfaceInfo> bindingInterfaces = new ArrayList<>();
        final List<MethodInfo> methods = new ArrayList<>(); final List<FieldInfo> fields = new ArrayList<>();
        TypeInfo(String id, String qualifiedName, String simpleName, String packageName, String moduleName,
                 Location location, Map<String, Ann> annotations, boolean interfaceType) {
            this.id=id; this.qualifiedName=qualifiedName; this.simpleName=simpleName; this.packageName=packageName;
            this.moduleName=moduleName; this.location=location; this.annotations=annotations; this.interfaceType=interfaceType;
        }
    }
    private static final class MethodInfo {
        final String id, name, returnType; final boolean constructor; final List<ParameterInfo> parameters;
        final Map<String, Ann> annotations; final Location location; String beanImplementationType;
        final boolean synthetic;
        MethodInfo(String id,String name,boolean constructor,String returnType,List<ParameterInfo> parameters,Map<String,Ann> annotations,Location location,boolean synthetic){this.id=id;this.name=name;this.constructor=constructor;this.returnType=returnType;this.parameters=parameters;this.annotations=annotations;this.location=location;this.synthetic=synthetic;}
    }
    private record ParameterInfo(String name,String type,String containerElementType,boolean collection,boolean optional,Map<String,Ann> annotations,Location location) {}
    private record FieldInfo(String id,String name,String type,String containerElementType,Map<String,Ann> annotations,Location location) {}
    private record InterfaceInfo(String name,List<String> arguments) {}
    private record Location(String file,int start,int end,String module,String packageName) {}
    private record CallInfo(String sourceMethod,String receiverType,String methodName,String argumentType,Map<String,Ann> annotations,Location location,boolean conditional) {}
    private record BeanInfo(String id,String name,String declaredType,Set<String> assignableTypes,boolean primary,Set<String> qualifiers,List<Map<String,Object>> conditions,Location location,String typeId,String factoryMethod) {}
    private record RepositoryInfo(TypeInfo type,String domain,String idType) {}
    private record RepositoryTypes(String domain,String idType,String contract) {}
    private record FrameworkMethod(String name,List<String> parameters,String returnType,String access) {}
    private record Mapping(List<String> paths,Set<String> methods,List<String> consumes,List<String> produces,List<String> params,List<String> headers) {}
    private record Stereotype(String kind,Ann annotation,List<String> chain) {}
    private record DerivedQuery(Map<String,Object> metadata) {}
    private record YamlPart(int indent,String key) {}

    private record Ann(String name, Map<String, Object> values) {
        String simpleName() { return name.substring(name.lastIndexOf('.') + 1); }
        Object value(String key) { return values.get(key); }
        String string(String key) { Object value=values.get(key); return value == null ? null : String.valueOf(value); }
        List<String> list(String key) {
            Object value=values.get(key); if (value == null) return List.of();
            if (value instanceof Collection<?> collection) return collection.stream().map(String::valueOf).filter(item -> !item.isBlank()).toList();
            String text=String.valueOf(value); return text.isBlank() ? List.of() : List.of(text);
        }
    }
}
