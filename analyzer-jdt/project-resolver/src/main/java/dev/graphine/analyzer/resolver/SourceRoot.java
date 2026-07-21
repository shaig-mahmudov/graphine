package dev.graphine.analyzer.resolver;

import java.nio.file.Path;

public record SourceRoot(Path path, String sourceSet, String moduleName, Path moduleRoot) {}
