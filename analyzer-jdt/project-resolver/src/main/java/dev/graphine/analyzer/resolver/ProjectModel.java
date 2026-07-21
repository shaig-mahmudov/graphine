package dev.graphine.analyzer.resolver;

import java.nio.file.Path;
import java.util.List;
import java.util.Map;

public record ProjectModel(Path root, List<Path> moduleRoots, List<SourceRoot> sourceRoots,
                           List<Path> classpath, Map<Path, String> classpathProvenance,
                           String javaRelease, String fingerprint, long classpathResolutionMs) {
    public ProjectModel(Path root, List<Path> moduleRoots, List<SourceRoot> sourceRoots,
                        List<Path> classpath, String javaRelease, String fingerprint,
                        long classpathResolutionMs) {
        this(root, moduleRoots, sourceRoots, classpath, Map.of(), javaRelease, fingerprint,
                classpathResolutionMs);
    }
}
