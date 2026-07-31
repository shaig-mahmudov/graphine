package dev.graphine.analyzer.resolver;

import dev.graphine.analyzer.protocol.AnalysisRequest;
import dev.graphine.analyzer.protocol.AnalyzerMode;
import dev.graphine.analyzer.protocol.Diagnostic;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.time.Duration;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.Deque;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;
import javax.xml.XMLConstants;
import javax.xml.parsers.DocumentBuilderFactory;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;

/** A deliberately strict, non-executing subset of the Maven model. */
public final class MavenProjectResolver {
    private static final int MAVEN_OUTPUT_LIMIT = 1_048_576;
    private static final int MAX_MODEL_DEPTH = 24;
    private static final int MAX_DEPENDENCIES = 2_048;
    private static final String ANALYZER_VERSION = "graphine-analyzer 0.1.0";
    private static final String PROTOCOL_VERSION = "2";

    public ProjectModel resolve(AnalysisRequest request, Consumer<Diagnostic> diagnostics) throws Exception {
        Path root = canonical(request.projectRoot());
        if (!Files.isDirectory(root) || !Files.isRegularFile(root.resolve("pom.xml"))) {
            throw new IllegalArgumentException("project root must contain pom.xml");
        }
        List<Path> modules = discoverModules(root);
        Map<Path, PomModel> models = new LinkedHashMap<>();
        for (Path module : modules) model(module.resolve("pom.xml"), models, 0);

        Map<String, PomModel> reactor = new LinkedHashMap<>();
        for (PomModel model : models.values()) reactor.put(model.groupId + ':' + model.artifactId, model);
        List<SourceRoot> sourceRoots = new ArrayList<>();
        Map<Path, String> classpath = new LinkedHashMap<>();
        String javaRelease = "17";
        for (Path module : modules) {
            PomModel model = models.get(canonical(module.resolve("pom.xml")));
            javaRelease = release(model, javaRelease);
            addSourceRoot(sourceRoots, module, module.resolve("src/main/java"), "main", model.artifactId);
            if (request.options().includeTests()) {
                addSourceRoot(sourceRoots, module, module.resolve("src/test/java"), "test", model.artifactId);
            }
            addSourceRoot(sourceRoots, module, module.resolve("target/generated-sources/annotations"),
                    "generated-main", model.artifactId);
            if (request.options().includeTests()) {
                addSourceRoot(sourceRoots, module, module.resolve("target/generated-test-sources/test-annotations"),
                        "generated-test", model.artifactId);
            }
            addIfDirectory(classpath, module.resolve("target/classes"), "local_reactor");
            if (request.options().includeTests()) addIfDirectory(classpath, module.resolve("target/test-classes"), "local_reactor");
        }

        for (PomModel model : models.values()) {
            resolveDependencies(model, reactor, classpath, diagnostics);
        }
        for (String explicit : request.options().explicitClasspath()) {
            Path entry = canonical(Path.of(explicit));
            if (!Files.exists(entry)) throw new IllegalArgumentException("explicit classpath entry does not exist");
            classpath.put(entry, "explicit_classpath");
        }
        long classpathStarted = System.nanoTime();
        if (request.mode() == AnalyzerMode.trusted) {
            for (Path entry : resolveTrustedClasspath(root, request)) classpath.put(entry, "trusted_maven");
        }
        long classpathMs = Duration.ofNanos(System.nanoTime() - classpathStarted).toMillis();
        List<Path> orderedClasspath = classpath.keySet().stream().sorted().toList();
        String fingerprint = fingerprint(root, modules, sourceRoots, orderedClasspath, classpath, request);
        return new ProjectModel(root, List.copyOf(modules), List.copyOf(sourceRoots), orderedClasspath,
                Map.copyOf(classpath), javaRelease, fingerprint, classpathMs);
    }

    private static List<Path> discoverModules(Path root) throws Exception {
        Set<Path> modules = new LinkedHashSet<>();
        Deque<ModuleDepth> pending = new ArrayDeque<>();
        pending.add(new ModuleDepth(root, 0));
        while (!pending.isEmpty()) {
            ModuleDepth current = pending.removeFirst();
            if (current.depth > MAX_MODEL_DEPTH) throw new IllegalArgumentException("Maven module nesting is too deep");
            Path module = canonical(current.path);
            if (!module.startsWith(root) || !Files.isRegularFile(module.resolve("pom.xml"))) {
                throw new IllegalArgumentException("invalid Maven module path");
            }
            if (!modules.add(module)) continue;
            Element project = parsePom(module.resolve("pom.xml")).getDocumentElement();
            Element declared = childElement(project, "modules");
            if (declared == null) continue;
            for (Element name : childElements(declared, "module")) {
                Path nested = module.resolve(name.getTextContent().trim()).normalize();
                pending.add(new ModuleDepth(nested, current.depth + 1));
            }
        }
        return modules.stream().sorted().toList();
    }

    private static PomModel model(Path pomPath, Map<Path, PomModel> cache, int depth) throws Exception {
        if (depth > MAX_MODEL_DEPTH) throw new IllegalArgumentException("Maven parent nesting is too deep");
        Path canonicalPom = canonical(pomPath);
        PomModel cached = cache.get(canonicalPom);
        if (cached != null) return cached;
        Element project = parsePom(canonicalPom).getDocumentElement();
        Element parentElement = childElement(project, "parent");
        PomModel parent = null;
        String parentGroup = directText(parentElement, "groupId");
        String parentArtifact = directText(parentElement, "artifactId");
        String parentVersion = directText(parentElement, "version");
        if (parentElement != null) {
            String relative = directText(parentElement, "relativePath");
            if (relative == null) relative = "../pom.xml";
            if (!relative.isBlank()) {
                Path localParent = canonicalPom.getParent().resolve(relative).normalize();
                if (Files.isRegularFile(localParent)) parent = model(localParent, cache, depth + 1);
            }
            if (parent == null && parentGroup != null && parentArtifact != null && parentVersion != null) {
                Path cachedParent = localRepository().resolve(parentGroup.replace('.', '/')).resolve(parentArtifact)
                        .resolve(parentVersion).resolve(parentArtifact + '-' + parentVersion + ".pom");
                if (Files.isRegularFile(cachedParent)) parent = model(cachedParent, cache, depth + 1);
            }
        }

        Map<String, String> properties = new LinkedHashMap<>();
        Map<String, String> management = new LinkedHashMap<>();
        if (parent != null) {
            properties.putAll(parent.properties);
            management.putAll(parent.dependencyManagement);
        }
        Element propertyElement = childElement(project, "properties");
        if (propertyElement != null) {
            for (Element property : childElements(propertyElement, null)) {
                properties.put(property.getTagName(), property.getTextContent().trim());
            }
        }
        String group = first(directText(project, "groupId"), parent == null ? parentGroup : parent.groupId);
        String artifact = directText(project, "artifactId");
        String version = first(directText(project, "version"), parent == null ? parentVersion : parent.version);
        if (artifact == null || artifact.isBlank()) throw new IllegalArgumentException("project artifactId is required");
        properties.put("project.groupId", nullToEmpty(group));
        properties.put("pom.groupId", nullToEmpty(group));
        properties.put("project.artifactId", artifact);
        properties.put("pom.artifactId", artifact);
        properties.put("project.version", nullToEmpty(version));
        properties.put("pom.version", nullToEmpty(version));
        group = interpolate(group, properties);
        version = interpolate(version, properties);

        Element managementElement = childElement(childElement(project, "dependencyManagement"), "dependencies");
        for (Dependency dependency : dependencies(managementElement, properties)) {
            if (dependency.version != null) management.put(dependency.ga(), dependency.version);
        }
        PomModel result = new PomModel(canonicalPom, canonicalPom.getParent(), group, artifact, version,
                Map.copyOf(properties), Map.copyOf(management), dependencies(childElement(project, "dependencies"), properties));
        cache.put(canonicalPom, result);
        return result;
    }

    private static void resolveDependencies(PomModel initial, Map<String, PomModel> reactor,
                                            Map<Path, String> classpath, Consumer<Diagnostic> diagnostics) throws Exception {
        Deque<DependencyDepth> pending = new ArrayDeque<>();
        for (Dependency dependency : initial.dependencies) pending.add(new DependencyDepth(dependency, initial, 0));
        Set<String> visited = new LinkedHashSet<>();
        int count = 0;
        while (!pending.isEmpty()) {
            DependencyDepth current = pending.removeFirst();
            if (++count > MAX_DEPENDENCIES || current.depth > MAX_MODEL_DEPTH) {
                diagnostics.accept(diagnostic(current.dependency.ga(), "Safe Maven dependency traversal limit reached"));
                return;
            }
            Dependency dependency = current.dependency;
            if ("provided".equals(dependency.scope) || "system".equals(dependency.scope)
                    || "import".equals(dependency.scope) || dependency.optional) continue;
            PomModel reactorModel = reactor.get(dependency.ga());
            String version = first(dependency.version, current.owner.dependencyManagement.get(dependency.ga()));
            if (version == null && reactorModel != null) version = reactorModel.version;
            version = interpolate(version, current.owner.properties);
            if (reactorModel != null && (version == null || version.equals(reactorModel.version))) {
                addIfDirectory(classpath, reactorModel.moduleRoot.resolve("target/classes"), "local_reactor");
                continue;
            }
            if (version == null || version.isBlank() || version.contains("${")) {
                diagnostics.accept(diagnostic(dependency.ga(), "Dependency version is unresolved in the supported safe Maven subset"));
                continue;
            }
            String coordinate = dependency.ga() + ':' + version;
            if (!visited.add(coordinate)) continue;
            Path base = localRepository().resolve(dependency.groupId.replace('.', '/')).resolve(dependency.artifactId).resolve(version);
            Path jar = base.resolve(dependency.artifactId + '-' + version + ".jar");
            if (Files.isRegularFile(jar)) classpath.put(canonical(jar), "local_cache");
            else {
                diagnostics.accept(diagnostic(coordinate, "Artifact is absent from the local Maven cache"));
                continue;
            }
            Path pom = base.resolve(dependency.artifactId + '-' + version + ".pom");
            if (Files.isRegularFile(pom)) {
                Map<Path, PomModel> isolated = new LinkedHashMap<>();
                PomModel dependencyModel = model(pom, isolated, current.depth + 1);
                for (Dependency transitive : dependencyModel.dependencies) {
                    pending.add(new DependencyDepth(transitive, dependencyModel, current.depth + 1));
                }
            }
        }
    }

    private static Diagnostic diagnostic(String symbol, String reason) {
        return new Diagnostic("UNRESOLVED_MAVEN_DEPENDENCY", "pom.xml", 1, 1, symbol, reason, "warning");
    }

    private static List<Dependency> dependencies(Element container, Map<String, String> properties) {
        if (container == null) return List.of();
        List<Dependency> result = new ArrayList<>();
        for (Element element : childElements(container, "dependency")) {
            String group = interpolate(directText(element, "groupId"), properties);
            String artifact = interpolate(directText(element, "artifactId"), properties);
            if (group == null || artifact == null) continue;
            result.add(new Dependency(group, artifact, interpolate(directText(element, "version"), properties),
                    first(directText(element, "scope"), "compile"),
                    Boolean.parseBoolean(first(directText(element, "optional"), "false"))));
        }
        return result;
    }

    private static String interpolate(String value, Map<String, String> properties) {
        if (value == null) return null;
        String current = value;
        Set<String> seen = new LinkedHashSet<>();
        for (int depth = 0; depth < MAX_MODEL_DEPTH && current.contains("${"); depth++) {
            if (!seen.add(current)) throw new IllegalArgumentException("Maven property interpolation cycle");
            int start = current.indexOf("${");
            int end = current.indexOf('}', start + 2);
            if (end < 0) return current;
            String key = current.substring(start + 2, end);
            String replacement = properties.get(key);
            if (replacement == null) return current;
            current = current.substring(0, start) + replacement + current.substring(end + 1);
        }
        if (current.contains("${")) throw new IllegalArgumentException("Maven property interpolation is too deep");
        return current;
    }

    private static String release(PomModel model, String fallback) {
        for (String key : List.of("maven.compiler.release", "java.version", "maven.compiler.source")) {
            String value = interpolate(model.properties.get(key), model.properties);
            if (value != null && value.matches("(?:1\\.)?[0-9]{1,2}")) return value.startsWith("1.") ? value.substring(2) : value;
        }
        return fallback;
    }

    private static Document parsePom(Path pom) throws Exception {
        DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
        factory.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
        factory.setFeature("http://xml.org/sax/features/external-general-entities", false);
        factory.setFeature("http://xml.org/sax/features/external-parameter-entities", false);
        factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");
        factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_SCHEMA, "");
        return factory.newDocumentBuilder().parse(pom.toFile());
    }

    private static Element childElement(Element parent, String name) {
        if (parent == null) return null;
        for (Node node = parent.getFirstChild(); node != null; node = node.getNextSibling()) {
            if (node instanceof Element element && (name == null || name.equals(element.getTagName()))) return element;
        }
        return null;
    }

    private static List<Element> childElements(Element parent, String name) {
        if (parent == null) return List.of();
        List<Element> result = new ArrayList<>();
        for (Node node = parent.getFirstChild(); node != null; node = node.getNextSibling()) {
            if (node instanceof Element element && (name == null || name.equals(element.getTagName()))) result.add(element);
        }
        return result;
    }

    private static String directText(Element parent, String name) {
        Element child = childElement(parent, name);
        return child == null ? null : child.getTextContent().trim();
    }

    private static void addSourceRoot(List<SourceRoot> roots, Path module, Path candidate,
                                      String sourceSet, String moduleName) throws IOException {
        if (Files.isDirectory(candidate)) roots.add(new SourceRoot(canonical(candidate), sourceSet, moduleName, module));
    }

    private static void addIfDirectory(Map<Path, String> entries, Path candidate, String provenance) throws IOException {
        if (Files.isDirectory(candidate)) entries.put(canonical(candidate), provenance);
    }

    private static List<Path> resolveTrustedClasspath(Path root, AnalysisRequest request) throws Exception {
        Path output = Files.createTempFile("graphine-classpath-", ".txt");
        try {
            List<String> command = List.of(request.mavenExecutable().toString(), "-q", "-DincludeScope=test",
                    "dependency:build-classpath", "-Dmdep.outputAbsoluteArtifactFilename=true",
                    "-Dmdep.outputFile=" + output.toAbsolutePath());
            ProcessBuilder builder = new ProcessBuilder(command).directory(root.toFile()).redirectErrorStream(true);
            Map<String, String> original = System.getenv();
            builder.environment().clear();
            for (String allowed : List.of("PATH", "PATHEXT", "JAVA_HOME", "HOME", "USERPROFILE", "TEMP", "TMP",
                    "SystemRoot", "WINDIR", "ComSpec", "APPDATA", "LOCALAPPDATA")) {
                original.entrySet().stream().filter(entry -> entry.getKey().equalsIgnoreCase(allowed)).findFirst()
                        .ifPresent(entry -> builder.environment().put(entry.getKey(), entry.getValue()));
            }
            Process process = builder.start();
            ByteArrayOutputStream captured = new ByteArrayOutputStream();
            Thread reader = new Thread(() -> copyBounded(process.getInputStream(), captured), "graphine-maven-output");
            reader.setDaemon(true);
            reader.start();
            boolean finished = process.waitFor(Math.min(request.timeoutMs(), 120_000), TimeUnit.MILLISECONDS);
            if (!finished) { process.destroyForcibly(); throw new IOException("trusted Maven classpath resolution timed out"); }
            reader.join(2_000);
            if (process.exitValue() != 0) throw new IOException("trusted Maven classpath resolution failed");
            if (!Files.isRegularFile(output)) return List.of();
            String text = Files.readString(output, StandardCharsets.UTF_8).trim();
            if (text.isEmpty()) return List.of();
            List<Path> result = new ArrayList<>();
            for (String entry : text.split(java.io.File.pathSeparator)) if (Files.exists(Path.of(entry))) result.add(canonical(Path.of(entry)));
            return result;
        } finally { Files.deleteIfExists(output); }
    }

    private static void copyBounded(InputStream input, ByteArrayOutputStream output) {
        try (input) {
            byte[] buffer = new byte[8192];
            int total = 0;
            for (int read; (read = input.read(buffer)) >= 0;) {
                total += read;
                if (total > MAVEN_OUTPUT_LIMIT) throw new IOException("Maven output exceeded limit");
                output.write(buffer, 0, read);
            }
        } catch (IOException ignored) { /* controlling thread reports a safe generic error */ }
    }

    private static String fingerprint(Path root, List<Path> modules, List<SourceRoot> roots,
                                      List<Path> classpath, Map<Path, String> provenance,
                                      AnalysisRequest request) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        update(digest, "analyzer=" + ANALYZER_VERSION + "\nprotocol=" + PROTOCOL_VERSION
                + "\nmode=" + request.mode() + "\nsource_sets=" + request.sourceSets()
                + "\ninclude_method_bodies=" + request.options().includeMethodBodies()
                + "\ninclude_field_access=" + request.options().includeFieldAccess()
                + "\ninclude_tests=" + request.options().includeTests() + '\n');
        List<Path> files = new ArrayList<>();
        for (Path module : modules) files.add(module.resolve("pom.xml"));
        for (SourceRoot sourceRoot : roots) {
            try (var stream = Files.walk(sourceRoot.path())) {
                stream.filter(path -> Files.isRegularFile(path) && path.toString().endsWith(".java")).forEach(files::add);
            }
        }
        files.sort(Comparator.comparing(path -> root.relativize(path).toString().replace('\\', '/')));
        for (Path file : files) {
            update(digest, "repo:" + root.relativize(file).toString().replace('\\', '/') + '\0');
            digest.update(Files.readAllBytes(file));
        }
        for (Path entry : classpath) {
            update(digest, "classpath:" + provenance.getOrDefault(entry, "unknown") + ':' + portableClasspathName(entry) + '\0');
            hashPathContents(digest, entry);
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static void hashPathContents(MessageDigest digest, Path entry) throws IOException {
        if (Files.isRegularFile(entry)) { digest.update(Files.readAllBytes(entry)); return; }
        if (!Files.isDirectory(entry)) return;
        List<Path> files;
        try (var stream = Files.walk(entry)) { files = stream.filter(Files::isRegularFile).sorted().toList(); }
        for (Path file : files) {
            update(digest, entry.relativize(file).toString().replace('\\', '/') + '\0');
            digest.update(Files.readAllBytes(file));
        }
    }

    private static String portableClasspathName(Path path) {
        Path repository = localRepository().toAbsolutePath().normalize();
        Path absolute = path.toAbsolutePath().normalize();
        return absolute.startsWith(repository)
                ? repository.relativize(absolute).toString().replace('\\', '/')
                : path.getFileName().toString();
    }

    private static void update(MessageDigest digest, String value) {
        digest.update(value.getBytes(StandardCharsets.UTF_8));
    }

    private static Path localRepository() {
        return Path.of(System.getProperty("user.home"), ".m2", "repository");
    }

    private static Path canonical(Path path) throws IOException {
        try { return path.toRealPath(); }
        catch (java.nio.file.AccessDeniedException denied) {
            Path normalized = path.toAbsolutePath().normalize();
            if (!Files.exists(normalized)) throw denied;
            return normalized;
        }
    }

    private static String first(String value, String fallback) {
        return value == null || value.isBlank() ? fallback : value;
    }

    private static String nullToEmpty(String value) { return value == null ? "" : value; }

    private record ModuleDepth(Path path, int depth) {}
    private record DependencyDepth(Dependency dependency, PomModel owner, int depth) {}
    private record Dependency(String groupId, String artifactId, String version, String scope, boolean optional) {
        String ga() { return groupId + ':' + artifactId; }
    }
    private record PomModel(Path pom, Path moduleRoot, String groupId, String artifactId, String version,
                            Map<String, String> properties, Map<String, String> dependencyManagement,
                            List<Dependency> dependencies) {}
}
