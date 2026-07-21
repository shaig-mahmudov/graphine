package dev.graphine.analyzer.java;

import dev.graphine.analyzer.protocol.AnalysisSummary;

public record AnalysisResult(AnalysisSummary summary, String status) {}
