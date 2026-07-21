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
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HexFormat;
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
import org.w3c.dom.NodeList;

public final class MavenProjectResolver {
    private static final int MAVEN_OUTPUT_LIMIT = 1_048_576;

    public ProjectModel resolve(AnalysisRequest request, Consumer<Diagnostic> diagnostics) throws Exception {
        Path root = canonical(request.projectRoot());
        if (!Files.isDirectory(root) || !Files.isRegularFile(root.resolve("pom.xml"))) {
            throw new IllegalArgumentException("project root must contain pom.xml");
        }
        List<Path> modules = discoverModules(root);
        List<SourceRoot> sourceRoots = new ArrayList<>();
        Set<Path> classpath = new LinkedHashSet<>();
        String javaRelease = "17";
        for (Path module : modules) {
            Document pom = parsePom(module.resolve("pom.xml"));
            javaRelease = discoverJavaRelease(pom, javaRelease);
            String moduleName = artifactId(pom, module);
            addSourceRoot(sourceRoots, module, module.resolve("src/main/java"), "main", moduleName);
            if (request.options().includeTests()) {
                addSourceRoot(sourceRoots, module, module.resolve("src/test/java"), "test", moduleName);
            }
            addSourceRoot(sourceRoots, module, module.resolve("target/generated-sources/annotations"), "generated-main", moduleName);
            if (request.options().includeTests()) {
                addSourceRoot(sourceRoots, module, module.resolve("target/generated-test-sources/test-annotations"), "generated-test", moduleName);
            }
            addIfDirectory(classpath, module.resolve("target/classes"));
            addIfDirectory(classpath, module.resolve("target/test-classes"));
            discoverLocalDependencies(pom, classpath, diagnostics);
        }
        for (String explicit : request.options().explicitClasspath()) {
            Path entry = canonical(Path.of(explicit));
            classpath.add(entry);
        }
        long classpathStarted = System.nanoTime();
        if (request.mode() == AnalyzerMode.trusted) {
            classpath.addAll(resolveTrustedClasspath(root, request));
        }
        long classpathMs = Duration.ofNanos(System.nanoTime() - classpathStarted).toMillis();
        return new ProjectModel(root, List.copyOf(modules), List.copyOf(sourceRoots),
                List.copyOf(classpath), javaRelease, fingerprint(root, modules, sourceRoots), classpathMs);
    }

    private static List<Path> discoverModules(Path root) throws Exception {
        List<Path> modules = new ArrayList<>();
        modules.add(root);
        Document parent = parsePom(root.resolve("pom.xml"));
        NodeList names = parent.getElementsByTagName("module");
        for (int index = 0; index < names.getLength(); index++) {
            Path module = canonical(root.resolve(names.item(index).getTextContent().trim()).normalize());
            if (!module.startsWith(root) || !Files.isRegularFile(module.resolve("pom.xml"))) {
                throw new IllegalArgumentException("invalid Maven module path");
            }
            modules.add(module);
        }
        return modules.stream().distinct().sorted().toList();
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

    private static String artifactId(Document pom, Path module) {
        NodeList ids = pom.getDocumentElement().getElementsByTagName("artifactId");
        return ids.getLength() == 0 ? module.getFileName().toString() : ids.item(0).getTextContent().trim();
    }

    private static String discoverJavaRelease(Document pom, String fallback) {
        for (String tag : List.of("maven.compiler.release", "java.version", "maven.compiler.source")) {
            NodeList values = pom.getElementsByTagName(tag);
            if (values.getLength() > 0) {
                String value = values.item(0).getTextContent().trim();
                if (value.matches("(?:1\\.)?[0-9]{1,2}")) return value.startsWith("1.") ? value.substring(2) : value;
            }
        }
        NodeList releases = pom.getElementsByTagName("release");
        return releases.getLength() > 0 ? releases.item(0).getTextContent().trim() : fallback;
    }

    private static void addSourceRoot(List<SourceRoot> roots, Path module, Path candidate,
                                      String sourceSet, String moduleName) throws IOException {
        if (Files.isDirectory(candidate)) roots.add(new SourceRoot(canonical(candidate), sourceSet, moduleName, module));
    }

    private static void addIfDirectory(Set<Path> entries, Path candidate) throws IOException {
        if (Files.isDirectory(candidate)) entries.add(canonical(candidate));
    }

    private static void discoverLocalDependencies(Document pom, Set<Path> classpath,
                                                  Consumer<Diagnostic> diagnostics) {
        String repository = System.getProperty("user.home") + "/.m2/repository";
        NodeList dependencies = pom.getElementsByTagName("dependency");
        for (int i = 0; i < dependencies.getLength(); i++) {
            Element dependency = (Element) dependencies.item(i);
            String group = child(dependency, "groupId");
            String artifact = child(dependency, "artifactId");
            String version = child(dependency, "version");
            if (group == null || artifact == null || version == null || version.contains("${")) continue;
            Path jar = Path.of(repository, group.replace('.', '/'), artifact, version,
                    artifact + "-" + version + ".jar");
            if (Files.isRegularFile(jar)) classpath.add(jar.toAbsolutePath().normalize());
            else diagnostics.accept(new Diagnostic("UNRESOLVED_MAVEN_DEPENDENCY", "pom.xml", 1, 1,
                    group + ":" + artifact + ":" + version, "Artifact is absent from the local Maven cache", "warning"));
        }
    }

    private static String child(Element parent, String name) {
        NodeList values = parent.getElementsByTagName(name);
        return values.getLength() == 0 ? null : values.item(0).getTextContent().trim();
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
            if (!finished) {
                process.destroyForcibly();
                throw new IOException("trusted Maven classpath resolution timed out");
            }
            reader.join(2_000);
            if (process.exitValue() != 0) throw new IOException("trusted Maven classpath resolution failed");
            if (!Files.isRegularFile(output)) return List.of();
            String text = Files.readString(output, StandardCharsets.UTF_8).trim();
            if (text.isEmpty()) return List.of();
            List<Path> result = new ArrayList<>();
            for (String entry : text.split(java.io.File.pathSeparator)) {
                Path path = Path.of(entry);
                if (Files.exists(path)) result.add(canonical(path));
            }
            return result;
        } finally {
            Files.deleteIfExists(output);
        }
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
        } catch (IOException ignored) {
            // The controlling thread reports a generic safe Maven failure.
        }
    }

    private static String fingerprint(Path root, List<Path> modules, List<SourceRoot> roots) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        List<Path> files = new ArrayList<>();
        for (Path module : modules) files.add(module.resolve("pom.xml"));
        for (SourceRoot sourceRoot : roots) {
            try (var stream = Files.walk(sourceRoot.path())) {
                stream.filter(path -> path.toString().endsWith(".java")).forEach(files::add);
            }
        }
        files.sort(Comparator.comparing(Path::toString));
        for (Path file : files) {
            String value = root.relativize(file).toString().replace('\\', '/') + ':'
                    + Files.size(file) + ':' + Files.getLastModifiedTime(file).toMillis();
            digest.update(value.getBytes(StandardCharsets.UTF_8));
        }
        return HexFormat.of().formatHex(digest.digest());
    }

    private static Path canonical(Path path) throws IOException {
        try {
            return path.toRealPath();
        } catch (java.nio.file.AccessDeniedException denied) {
            Path normalized = path.toAbsolutePath().normalize();
            if (!Files.exists(normalized)) throw denied;
            return normalized;
        }
    }
}
