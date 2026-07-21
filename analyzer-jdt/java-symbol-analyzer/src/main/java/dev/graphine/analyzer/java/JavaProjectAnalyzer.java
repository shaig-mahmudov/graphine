package dev.graphine.analyzer.java;

import dev.graphine.analyzer.protocol.AnalysisRequest;
import dev.graphine.analyzer.protocol.AnalysisSummary;
import dev.graphine.analyzer.protocol.Diagnostic;
import dev.graphine.analyzer.protocol.ProtocolWriter;
import dev.graphine.analyzer.resolver.ProjectModel;
import dev.graphine.analyzer.resolver.SourceRoot;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.atomic.AtomicLong;
import org.eclipse.jdt.core.JavaCore;
import org.eclipse.jdt.core.compiler.IProblem;
import org.eclipse.jdt.core.dom.AST;
import org.eclipse.jdt.core.dom.ASTParser;
import org.eclipse.jdt.core.dom.CompilationUnit;
import org.eclipse.jdt.core.dom.FileASTRequestor;

public final class JavaProjectAnalyzer {
    public AnalysisResult analyze(ProjectModel model, AnalysisRequest request, ProtocolWriter writer)
            throws Exception {
        long started = System.nanoTime();
        GraphCollector collector = new GraphCollector();
        List<Path> files = discoverJavaFiles(model.sourceRoots());
        Map<Path, SourceRoot> ownership = ownership(model.sourceRoots(), files);
        String[] paths = files.stream().map(Path::toString).toArray(String[]::new);
        String[] encodings = files.stream().map(path -> "UTF-8").toArray(String[]::new);
        AtomicLong parsed = new AtomicLong();
        AtomicLong failed = new AtomicLong();
        AtomicLong peakMemory = new AtomicLong();
        long parsingStarted = System.nanoTime();

        ASTParser parser = ASTParser.newParser(AST.getJLSLatest());
        parser.setResolveBindings(true);
        parser.setBindingsRecovery(true);
        parser.setStatementsRecovery(true);
        parser.setEnvironment(model.classpath().stream().map(Path::toString).toArray(String[]::new),
                model.sourceRoots().stream().map(root -> root.path().toString()).toArray(String[]::new),
                model.sourceRoots().stream().map(root -> "UTF-8").toArray(String[]::new), true);
        Map<String, String> options = new HashMap<>(JavaCore.getOptions());
        JavaCore.setComplianceOptions(model.javaRelease(), options);
        options.put(JavaCore.COMPILER_SOURCE, model.javaRelease());
        options.put(JavaCore.COMPILER_COMPLIANCE, model.javaRelease());
        options.put(JavaCore.COMPILER_CODEGEN_TARGET_PLATFORM, model.javaRelease());
        parser.setCompilerOptions(options);
        parser.createASTs(paths, encodings, new String[0], new FileASTRequestor() {
            @Override public void acceptAST(String sourceFilePath, CompilationUnit unit) {
                Path file = Path.of(sourceFilePath).toAbsolutePath().normalize();
                SourceRoot sourceRoot = ownership.get(file);
                if (sourceRoot == null) return;
                parsed.incrementAndGet();
                boolean syntaxFailure = false;
                Set<Integer> ambiguousLines = new HashSet<>();
                for (IProblem problem : unit.getProblems()) {
                    if (problem.isError()) {
                        String lower = problem.getMessage().toLowerCase();
                        if (lower.contains("syntax")) {
                            syntaxFailure = true;
                            collector.diagnostic(new Diagnostic("JAVA_PARSE_ERROR", relative(model.root(), file),
                                    positive(problem.getSourceLineNumber()), positive(problem.getSourceLineNumber()),
                                    null, bounded(problem.getMessage()), "error"));
                        } else if (lower.contains("ambiguous")) {
                            ambiguousLines.add(positive(problem.getSourceLineNumber()));
                            collector.diagnostic(new Diagnostic("AMBIGUOUS_BINDING", relative(model.root(), file),
                                    positive(problem.getSourceLineNumber()), positive(problem.getSourceLineNumber()),
                                    null, bounded(problem.getMessage()), "warning"));
                        }
                    }
                }
                if (syntaxFailure) failed.incrementAndGet();
                unit.accept(new SymbolVisitor(unit, file, model.root(), sourceRoot, collector, request.options(),
                        ambiguousLines));
                long used = Runtime.getRuntime().totalMemory() - Runtime.getRuntime().freeMemory();
                peakMemory.accumulateAndGet(used, Math::max);
            }
        }, null);
        long parsingMs = Duration.ofNanos(System.nanoTime() - parsingStarted).toMillis();
        long serializationStarted = System.nanoTime();
        collector.emit(writer);
        long serializationMs = Duration.ofNanos(System.nanoTime() - serializationStarted).toMillis();
        String status = failed.get() == 0 ? "complete" : "partial";
        AnalysisSummary summary = new AnalysisSummary(files.size(), parsed.get(), failed.get(),
                collector.bindingsResolved, collector.bindingsUnresolved, collector.nodeCount(), collector.edgeCount(),
                Duration.ofNanos(System.nanoTime() - started).toMillis(), model.classpathResolutionMs(), parsingMs,
                serializationMs, peakMemory.get(), status);
        return new AnalysisResult(summary, status);
    }

    private static List<Path> discoverJavaFiles(List<SourceRoot> roots) throws IOException {
        List<Path> files = new ArrayList<>();
        for (SourceRoot root : roots) {
            try (var stream = Files.walk(root.path())) {
                stream.filter(path -> Files.isRegularFile(path) && path.toString().endsWith(".java"))
                        .map(path -> path.toAbsolutePath().normalize()).forEach(files::add);
            }
        }
        files.sort(Comparator.comparing(Path::toString));
        return files;
    }

    private static Map<Path, SourceRoot> ownership(List<SourceRoot> roots, List<Path> files) {
        Map<Path, SourceRoot> result = new HashMap<>();
        for (Path file : files) roots.stream().filter(root -> file.startsWith(root.path()))
                .max(Comparator.comparingInt(root -> root.path().getNameCount())).ifPresent(root -> result.put(file, root));
        return result;
    }

    static String relative(Path root, Path file) {
        return root.relativize(file).toString().replace('\\', '/');
    }

    private static Integer positive(int value) { return value > 0 ? value : 1; }
    private static String bounded(String value) { return value.length() <= 500 ? value : value.substring(0, 500); }
}
