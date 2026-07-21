package dev.graphine.analyzer.resolver;

import java.nio.file.Path;
import java.util.List;

public record ProjectModel(Path root, List<Path> moduleRoots, List<SourceRoot> sourceRoots,
                           List<Path> classpath, String javaRelease, String fingerprint,
                           long classpathResolutionMs) {}
