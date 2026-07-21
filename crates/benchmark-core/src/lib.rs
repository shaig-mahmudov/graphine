use anyhow::{Context, Result, anyhow, bail};
use jsonschema::Validator;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIDENCE_LEVELS: &[&str] = &[
    "RUNTIME_CONFIRMED",
    "BYTECODE_CONFIRMED",
    "COMPILER_RESOLVED",
    "FRAMEWORK_RESOLVED",
    "STATIC_INFERRED",
    "AMBIGUOUS",
    "UNRESOLVED",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Question {
    pub id: String,
    pub fixture: String,
    pub category: String,
    pub question: String,
    pub difficulty: String,
    pub expected_capabilities: Vec<String>,
    pub token_budget: u64,
    #[serde(default = "default_support")]
    pub support: String,
}

fn default_support() -> String {
    "phase-0".to_owned()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Evidence {
    pub file: String,
    pub start_line: u64,
    pub end_line: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroundTruth {
    pub question_id: String,
    pub status: String,
    pub expected: Value,
    pub confidence_required: String,
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub allowed_ambiguities: Vec<String>,
    #[serde(default)]
    pub forbidden_claims: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Corpus {
    pub questions: Vec<Question>,
    pub ground_truth: Vec<GroundTruth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorpusStats {
    pub question_count: usize,
    pub ground_truth_count: usize,
    pub by_fixture: BTreeMap<String, usize>,
    pub by_category: BTreeMap<String, usize>,
    pub by_difficulty: BTreeMap<String, usize>,
    pub by_status: BTreeMap<String, usize>,
    pub by_support: BTreeMap<String, usize>,
    pub total_token_budget: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunReport {
    pub schema_version: String,
    pub report_kind: String,
    pub corpus: CorpusStats,
    pub metrics: Metrics,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Metrics {
    pub correctness: Option<f64>,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub unsupported_claims: Option<u64>,
    pub unresolved_count: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub tool_calls: Option<u64>,
    pub file_reads: Option<u64>,
    pub files_opened: Option<u64>,
    pub wall_time_ms: Option<u64>,
    pub query_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GraphClaim {
    Node {
        stable_id: String,
        kind: String,
    },
    Edge {
        source: String,
        target: String,
        kind: String,
    },
    Diagnostic {
        kind: String,
        symbol_text: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExpectedGraphClaim {
    pub question_id: String,
    pub category: String,
    #[serde(flatten)]
    pub claim: GraphClaim,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UnsupportedQuestion {
    pub question_id: String,
    pub category: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphGroundTruth {
    pub fixture: String,
    pub expected: Vec<ExpectedGraphClaim>,
    #[serde(default)]
    pub forbidden: Vec<ExpectedGraphClaim>,
    #[serde(default)]
    pub unsupported_questions: Vec<UnsupportedQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphAccuracyReport {
    pub fixture: String,
    pub precision: f64,
    pub recall: f64,
    pub true_positives: usize,
    pub false_positives: Vec<ExpectedGraphClaim>,
    pub false_negatives: Vec<ExpectedGraphClaim>,
    pub unsupported_claims: Vec<ExpectedGraphClaim>,
    pub question_results: Vec<QuestionAccuracyResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestionAccuracyResult {
    pub question_id: String,
    pub category: String,
    pub status: String,
    pub matched: usize,
    pub expected: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Evaluates normalized analyzer JSONL claims against the supported graph contract.
/// Claims outside a declared supported/forbidden scope are intentionally not scored.
///
/// # Errors
///
/// Returns an error for unreadable/malformed JSONL or malformed ground truth.
pub fn evaluate_graph_jsonl(input: &Path, truth_path: &Path) -> Result<GraphAccuracyReport> {
    let truth: GraphGroundTruth = serde_json::from_str(
        &fs::read_to_string(truth_path)
            .with_context(|| format!("failed to read {}", truth_path.display()))?,
    )
    .with_context(|| format!("malformed graph ground truth {}", truth_path.display()))?;
    let mut actual = BTreeSet::new();
    for (index, line) in fs::read_to_string(input)
        .with_context(|| format!("failed to read {}", input.display()))?
        .lines()
        .enumerate()
    {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("malformed analyzer JSONL line {}", index + 1))?;
        match value.get("type").and_then(Value::as_str) {
            Some("node") => {
                if let (Some(stable_id), Some(kind)) = (
                    value.get("stable_id").and_then(Value::as_str),
                    value.get("kind").and_then(Value::as_str),
                ) {
                    actual.insert(GraphClaim::Node {
                        stable_id: stable_id.to_owned(),
                        kind: kind.to_owned(),
                    });
                }
            }
            Some("edge") => {
                if let (Some(source), Some(target), Some(kind)) = (
                    value.get("source").and_then(Value::as_str),
                    value.get("target").and_then(Value::as_str),
                    value.get("kind").and_then(Value::as_str),
                ) {
                    actual.insert(GraphClaim::Edge {
                        source: source.to_owned(),
                        target: target.to_owned(),
                        kind: kind.to_owned(),
                    });
                }
            }
            Some("diagnostic") => {
                let diagnostic = value.get("diagnostic").unwrap_or(&value);
                if let Some(kind) = diagnostic.get("kind").and_then(Value::as_str) {
                    actual.insert(GraphClaim::Diagnostic {
                        kind: kind.to_owned(),
                        symbol_text: diagnostic
                            .get("symbol_text")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(evaluate_graph_claims(&truth, &actual))
}

#[must_use]
pub fn evaluate_graph_claims(
    truth: &GraphGroundTruth,
    actual: &BTreeSet<GraphClaim>,
) -> GraphAccuracyReport {
    let false_negatives: Vec<_> = truth
        .expected
        .iter()
        .filter(|expected| !actual.contains(&expected.claim))
        .cloned()
        .collect();
    let false_positives: Vec<_> = truth
        .forbidden
        .iter()
        .filter(|forbidden| actual.contains(&forbidden.claim))
        .cloned()
        .collect();
    let true_positives = truth.expected.len().saturating_sub(false_negatives.len());
    let precision_denominator = true_positives.saturating_add(false_positives.len());
    let precision = if precision_denominator == 0 {
        1.0
    } else {
        bounded_ratio(true_positives, precision_denominator)
    };
    let recall = if truth.expected.is_empty() {
        1.0
    } else {
        bounded_ratio(true_positives, truth.expected.len())
    };
    let mut grouped: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
    for expected in &truth.expected {
        let entry = grouped
            .entry((expected.question_id.clone(), expected.category.clone()))
            .or_default();
        entry.1 += 1;
        if actual.contains(&expected.claim) {
            entry.0 += 1;
        }
    }
    let mut question_results: Vec<_> = grouped
        .into_iter()
        .map(
            |((question_id, category), (matched, expected))| QuestionAccuracyResult {
                question_id,
                category,
                status: if matched == expected {
                    "passed"
                } else {
                    "failed"
                }
                .to_owned(),
                matched,
                expected,
                reason: None,
            },
        )
        .collect();
    question_results.extend(truth.unsupported_questions.iter().map(|question| {
        QuestionAccuracyResult {
            question_id: question.question_id.clone(),
            category: question.category.clone(),
            status: "unsupported".to_owned(),
            matched: 0,
            expected: 0,
            reason: Some(question.reason.clone()),
        }
    }));
    question_results.sort_by(|left, right| left.question_id.cmp(&right.question_id));
    GraphAccuracyReport {
        fixture: truth.fixture.clone(),
        precision,
        recall,
        true_positives,
        unsupported_claims: false_positives.clone(),
        false_positives,
        false_negatives,
        question_results,
    }
}

fn bounded_ratio(numerator: usize, denominator: usize) -> f64 {
    let numerator = u32::try_from(numerator).unwrap_or(u32::MAX);
    let denominator = u32::try_from(denominator).unwrap_or(u32::MAX);
    f64::from(numerator) / f64::from(denominator)
}

/// Writes a deterministic graph accuracy report.
///
/// # Errors
///
/// Returns an error if the destination cannot be written.
pub fn write_graph_accuracy_report(path: &Path, report: &GraphAccuracyReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = serde_json::to_string_pretty(report)?;
    output.push('\n');
    fs::write(path, output).with_context(|| format!("failed to write {}", path.display()))
}

/// Generates a deterministic medium Maven corpus without external dependencies.
///
/// # Errors
///
/// Returns an error when the destination is non-empty or files cannot be written.
pub fn generate_medium_corpus(output: &Path, type_count: usize) -> Result<()> {
    if !(50..=2_000).contains(&type_count) {
        bail!("type count must be between 50 and 2000");
    }
    if output.exists() && fs::read_dir(output)?.next().is_some() {
        bail!("medium corpus output must be empty");
    }
    let sources = output.join("src/main/java/dev/graphine/medium");
    fs::create_dir_all(&sources)?;
    fs::write(
        output.join("pom.xml"),
        "<project><modelVersion>4.0.0</modelVersion><groupId>dev.graphine</groupId><artifactId>medium-corpus</artifactId><version>1</version><properties><maven.compiler.release>17</maven.compiler.release></properties></project>\n",
    )?;
    for index in 0..type_count {
        let next = (index + 1) % type_count;
        let source = format!(
            "package dev.graphine.medium;\n\npublic final class C{index:04} {{\n    private int state;\n\n    public int compute(int input) {{\n        state += input;\n        int first = helper(input);\n        int second = helper(input + 1);\n        return first + second + helper(input + 2);\n    }}\n\n    private int helper(int value) {{\n        return value + state;\n    }}\n\n    public int forward(C{next:04} next, int input) {{\n        return next.compute(input);\n    }}\n}}\n"
        );
        fs::write(sources.join(format!("C{index:04}.java")), source)?;
    }
    Ok(())
}

/// Generates a deterministic, dependency-free Spring-shaped Maven corpus.
/// Framework annotations are source stubs with canonical Spring package names,
/// allowing safe-mode static analysis without executing Maven or using a network.
///
/// # Errors
///
/// Returns an error when the destination is non-empty, the requested size is
/// outside the bounded range, or files cannot be written.
pub fn generate_spring_medium_corpus(output: &Path, controller_count: usize) -> Result<()> {
    if !(25..=500).contains(&controller_count) {
        bail!("controller count must be between 25 and 500");
    }
    if output.exists() && fs::read_dir(output)?.next().is_some() {
        bail!("Spring medium corpus output must be empty");
    }
    let sources = output.join("src/main/java");
    fs::create_dir_all(&sources)?;
    fs::write(
        output.join("pom.xml"),
        "<project><modelVersion>4.0.0</modelVersion><groupId>dev.graphine</groupId><artifactId>spring-medium</artifactId><version>1</version><properties><maven.compiler.release>17</maven.compiler.release></properties></project>\n",
    )?;
    let stubs = [
        (
            "org/springframework/stereotype/Service.java",
            "package org.springframework.stereotype; public @interface Service { String value() default \"\"; }\n",
        ),
        (
            "org/springframework/web/bind/annotation/RestController.java",
            "package org.springframework.web.bind.annotation; public @interface RestController { String value() default \"\"; }\n",
        ),
        (
            "org/springframework/web/bind/annotation/RequestMapping.java",
            "package org.springframework.web.bind.annotation; public @interface RequestMapping { String[] value() default {}; String[] path() default {}; }\n",
        ),
        (
            "org/springframework/web/bind/annotation/GetMapping.java",
            "package org.springframework.web.bind.annotation; public @interface GetMapping { String[] value() default {}; String[] path() default {}; }\n",
        ),
        (
            "org/springframework/boot/autoconfigure/SpringBootApplication.java",
            "package org.springframework.boot.autoconfigure; public @interface SpringBootApplication {}\n",
        ),
    ];
    for (relative, contents) in stubs {
        let path = sources.join(relative);
        fs::create_dir_all(path.parent().context("stub path has no parent")?)?;
        fs::write(path, contents)?;
    }
    let application = sources.join("dev/graphine/springmedium/MediumApplication.java");
    let application_parent = application
        .parent()
        .context("application path has no parent")?;
    fs::create_dir_all(application_parent)?;
    fs::write(
        &application,
        "package dev.graphine.springmedium; import org.springframework.boot.autoconfigure.SpringBootApplication; @SpringBootApplication public class MediumApplication {}\n",
    )?;
    for index in 0..controller_count {
        let service = format!(
            "package dev.graphine.springmedium; import org.springframework.stereotype.Service; @Service public final class Service{index:03} {{ public String find(long id) {{ return \"item-\" + id; }} }}\n"
        );
        let controller = format!(
            "package dev.graphine.springmedium; import org.springframework.web.bind.annotation.*; @RestController @RequestMapping(\"/api/c{index:03}\") public final class Controller{index:03} {{ private final Service{index:03} service; public Controller{index:03}(Service{index:03} service) {{ this.service=service; }} @GetMapping(\"/{{id}}\") public String get(long id) {{ return service.find(id); }} }}\n"
        );
        fs::write(
            application_parent.join(format!("Service{index:03}.java")),
            service,
        )?;
        fs::write(
            application_parent.join(format!("Controller{index:03}.java")),
            controller,
        )?;
    }
    Ok(())
}

impl Corpus {
    /// Loads and validates the benchmark corpus rooted at `root`.
    ///
    /// # Errors
    ///
    /// Returns an error when schemas or corpus files cannot be read, YAML or
    /// schema validation fails, or a cross-document invariant is violated.
    pub fn load(root: &Path) -> Result<Self> {
        let question_schema = load_schema(&root.join("benchmarks/schema/question.schema.json"))?;
        let truth_schema = load_schema(&root.join("benchmarks/schema/ground-truth.schema.json"))?;
        let question_validator = jsonschema::validator_for(&question_schema)
            .context("failed to compile question schema")?;
        let truth_validator = jsonschema::validator_for(&truth_schema)
            .context("failed to compile ground-truth schema")?;

        let questions =
            load_documents::<Question>(&root.join("benchmarks/questions"), &question_validator)?;
        let ground_truth =
            load_documents::<GroundTruth>(&root.join("benchmarks/ground-truth"), &truth_validator)?;
        let corpus = Self {
            questions,
            ground_truth,
        };
        corpus.validate(root)?;
        Ok(corpus)
    }

    /// Checks uniqueness, question-to-answer coverage, confidence values, and evidence.
    ///
    /// # Errors
    ///
    /// Returns an error on the first invalid corpus relationship or evidence range.
    pub fn validate(&self, root: &Path) -> Result<()> {
        let mut question_ids = BTreeSet::new();
        for question in &self.questions {
            if !question_ids.insert(&question.id) {
                bail!("duplicate question ID: {}", question.id);
            }
        }

        let fixture_by_question: BTreeMap<_, _> = self
            .questions
            .iter()
            .map(|question| (question.id.as_str(), question.fixture.as_str()))
            .collect();
        let mut truth_ids = BTreeSet::new();
        for truth in &self.ground_truth {
            if !truth_ids.insert(&truth.question_id) {
                bail!("duplicate ground-truth ID: {}", truth.question_id);
            }
            if !question_ids.contains(&truth.question_id) {
                bail!("ground truth has no question: {}", truth.question_id);
            }
            if !CONFIDENCE_LEVELS.contains(&truth.confidence_required.as_str()) {
                bail!(
                    "unsupported confidence value for {}: {}",
                    truth.question_id,
                    truth.confidence_required
                );
            }
            let fixture = fixture_by_question[truth.question_id.as_str()];
            validate_evidence(root, fixture, truth)?;
        }
        if let Some(id) = question_ids.difference(&truth_ids).next() {
            bail!("missing ground truth for question: {id}");
        }
        Ok(())
    }

    #[must_use]
    pub fn stats(&self) -> CorpusStats {
        let mut stats = CorpusStats {
            question_count: self.questions.len(),
            ground_truth_count: self.ground_truth.len(),
            by_fixture: BTreeMap::new(),
            by_category: BTreeMap::new(),
            by_difficulty: BTreeMap::new(),
            by_status: BTreeMap::new(),
            by_support: BTreeMap::new(),
            total_token_budget: 0,
        };
        for question in &self.questions {
            *stats
                .by_fixture
                .entry(question.fixture.clone())
                .or_default() += 1;
            *stats
                .by_category
                .entry(question.category.clone())
                .or_default() += 1;
            *stats
                .by_difficulty
                .entry(question.difficulty.clone())
                .or_default() += 1;
            *stats
                .by_support
                .entry(question.support.clone())
                .or_default() += 1;
            stats.total_token_budget += question.token_budget;
        }
        for truth in &self.ground_truth {
            *stats.by_status.entry(truth.status.clone()).or_default() += 1;
        }
        stats
    }

    #[must_use]
    pub fn report(&self) -> RunReport {
        RunReport {
            schema_version: "1.0.0".to_owned(),
            report_kind: "corpus".to_owned(),
            corpus: self.stats(),
            metrics: Metrics::default(),
        }
    }
}

fn load_schema(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read schema {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("malformed schema {}", path.display()))
}

fn collect_yaml_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("failed to read {}", directory.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(collect_yaml_files(&path)?);
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yaml" | "yml")
        ) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn load_documents<T>(directory: &Path, validator: &Validator) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let mut values = Vec::new();
    for path in collect_yaml_files(directory)? {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        for (index, document) in serde_yaml::Deserializer::from_str(&text).enumerate() {
            let yaml = serde_yaml::Value::deserialize(document).with_context(|| {
                format!(
                    "malformed YAML in {} document {}",
                    path.display(),
                    index + 1
                )
            })?;
            let json = serde_json::to_value(yaml)?;
            if let Err(error) = validator.validate(&json) {
                return Err(anyhow!(
                    "schema validation failed in {} document {}: {error}",
                    path.display(),
                    index + 1
                ));
            }
            values.push(serde_json::from_value(json).with_context(|| {
                format!("invalid data in {} document {}", path.display(), index + 1)
            })?);
        }
    }
    Ok(values)
}

fn validate_evidence(root: &Path, fixture: &str, truth: &GroundTruth) -> Result<()> {
    for evidence in &truth.evidence {
        if evidence.start_line == 0 || evidence.end_line < evidence.start_line {
            bail!(
                "invalid evidence range for {}: {}:{}-{}",
                truth.question_id,
                evidence.file,
                evidence.start_line,
                evidence.end_line
            );
        }
        let relative = Path::new(&evidence.file);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            bail!("evidence path must be fixture-relative: {}", evidence.file);
        }
        let source = root.join("fixtures").join(fixture).join(relative);
        let contents = fs::read_to_string(&source).with_context(|| {
            format!(
                "evidence file for {} does not exist: {}",
                truth.question_id,
                source.display()
            )
        })?;
        let line_count = contents.lines().count() as u64;
        if evidence.end_line > line_count {
            bail!(
                "evidence range for {} exceeds {} lines in {}",
                truth.question_id,
                line_count,
                evidence.file
            );
        }
    }
    Ok(())
}

/// Writes a pretty, newline-terminated JSON report.
///
/// # Errors
///
/// Returns an error if the report cannot be serialized or the destination
/// directory or file cannot be written.
pub fn write_report(path: &Path, report: &RunReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut serialized = serde_json::to_string_pretty(report)?;
    serialized.push('\n');
    fs::write(path, serialized).with_context(|| format!("failed to write {}", path.display()))
}

/// Validates a report against the checked-in run-result schema.
///
/// # Errors
///
/// Returns an error when the schema is unreadable or the report violates it.
pub fn validate_report(root: &Path, report: &RunReport) -> Result<()> {
    let schema = load_schema(&root.join("benchmarks/schema/run-result.schema.json"))?;
    let validator =
        jsonschema::validator_for(&schema).context("failed to compile run-result schema")?;
    let value = serde_json::to_value(report)?;
    validator
        .validate(&value)
        .map_err(|error| anyhow!("run-result schema validation failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(id: &str) -> Question {
        Question {
            id: id.to_owned(),
            fixture: "java-core".to_owned(),
            category: "class-discovery".to_owned(),
            question: "Which class?".to_owned(),
            difficulty: "easy".to_owned(),
            expected_capabilities: vec!["symbol-discovery".to_owned()],
            token_budget: 100,
            support: "phase-0".to_owned(),
        }
    }

    fn truth(id: &str) -> GroundTruth {
        GroundTruth {
            question_id: id.to_owned(),
            status: "answerable".to_owned(),
            expected: serde_json::json!({"symbol": "example.Sample"}),
            confidence_required: "COMPILER_RESOLVED".to_owned(),
            evidence: Vec::new(),
            allowed_ambiguities: Vec::new(),
            forbidden_claims: Vec::new(),
        }
    }

    #[test]
    fn rejects_duplicate_question_ids() {
        let corpus = Corpus {
            questions: vec![question("q-1"), question("q-1")],
            ground_truth: vec![truth("q-1")],
        };
        assert!(
            corpus
                .validate(Path::new("."))
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn rejects_missing_ground_truth() {
        let corpus = Corpus {
            questions: vec![question("q-1")],
            ground_truth: Vec::new(),
        };
        assert!(
            corpus
                .validate(Path::new("."))
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
    }

    #[test]
    fn rejects_invalid_evidence_ranges() {
        let mut invalid = truth("q-1");
        invalid.evidence.push(Evidence {
            file: "Sample.java".to_owned(),
            start_line: 9,
            end_line: 2,
        });
        let corpus = Corpus {
            questions: vec![question("q-1")],
            ground_truth: vec![invalid],
        };
        assert!(
            corpus
                .validate(Path::new("."))
                .unwrap_err()
                .to_string()
                .contains("range")
        );
    }

    #[test]
    fn rejects_unsupported_confidence() {
        let mut invalid = truth("q-1");
        invalid.confidence_required = "CERTAIN".to_owned();
        let corpus = Corpus {
            questions: vec![question("q-1")],
            ground_truth: vec![invalid],
        };
        assert!(
            corpus
                .validate(Path::new("."))
                .unwrap_err()
                .to_string()
                .contains("confidence")
        );
    }

    #[test]
    fn corpus_statistics_are_stable() {
        let corpus = Corpus {
            questions: vec![question("q-1"), question("q-2")],
            ground_truth: vec![truth("q-1"), truth("q-2")],
        };
        let stats = corpus.stats();
        assert_eq!(stats.question_count, 2);
        assert_eq!(stats.by_fixture["java-core"], 2);
        assert_eq!(stats.total_token_budget, 200);
    }

    #[test]
    fn report_output_is_deterministic() {
        let corpus = Corpus {
            questions: vec![question("q-1")],
            ground_truth: vec![truth("q-1")],
        };
        let first = serde_json::to_string_pretty(&corpus.report()).unwrap();
        let second = serde_json::to_string_pretty(&corpus.report()).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("\"input_tokens\": null"));
    }

    #[test]
    fn checked_in_corpus_and_report_pass_schema_validation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let corpus = Corpus::load(root).unwrap();
        validate_report(root, &corpus.report()).unwrap();
    }

    #[test]
    fn graph_evaluator_scores_supported_claims_and_preserves_unsupported_questions() {
        let expected = ExpectedGraphClaim {
            question_id: "q-1".to_owned(),
            category: "callers".to_owned(),
            claim: GraphClaim::Edge {
                source: "method:A#run()".to_owned(),
                target: "method:B#call()".to_owned(),
                kind: "CALLS".to_owned(),
            },
        };
        let forbidden = ExpectedGraphClaim {
            question_id: "q-1".to_owned(),
            category: "overloads".to_owned(),
            claim: GraphClaim::Edge {
                source: "method:A#run()".to_owned(),
                target: "method:B#call(int)".to_owned(),
                kind: "CALLS".to_owned(),
            },
        };
        let truth = GraphGroundTruth {
            fixture: "fixture".to_owned(),
            expected: vec![expected.clone()],
            forbidden: vec![forbidden],
            unsupported_questions: vec![UnsupportedQuestion {
                question_id: "q-2".to_owned(),
                category: "runtime".to_owned(),
                reason: "outside static graph contract".to_owned(),
            }],
        };
        let actual = BTreeSet::from([expected.claim]);
        let report = evaluate_graph_claims(&truth, &actual);
        assert!((report.precision - 1.0).abs() < f64::EPSILON);
        assert!((report.recall - 1.0).abs() < f64::EPSILON);
        assert_eq!(report.question_results[1].status, "unsupported");
    }

    #[test]
    fn medium_corpus_generation_is_deterministic() {
        let base =
            std::env::temp_dir().join(format!("graphine-medium-test-{}", std::process::id()));
        let first = base.join("first");
        let second = base.join("second");
        generate_medium_corpus(&first, 50).unwrap();
        generate_medium_corpus(&second, 50).unwrap();
        for index in 0..50 {
            let relative = format!("src/main/java/dev/graphine/medium/C{index:04}.java");
            assert_eq!(
                fs::read(first.join(&relative)).unwrap(),
                fs::read(second.join(relative)).unwrap()
            );
        }
        assert_eq!(
            fs::read(first.join("pom.xml")).unwrap(),
            fs::read(second.join("pom.xml")).unwrap()
        );
    }

    #[test]
    fn spring_medium_corpus_has_bounded_deterministic_routes() {
        let output = std::env::temp_dir().join(format!(
            "graphine-spring-medium-test-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        if output.exists() {
            fs::remove_dir_all(&output).unwrap();
        }
        generate_spring_medium_corpus(&output, 25).unwrap();
        let controller = fs::read_to_string(
            output.join("src/main/java/dev/graphine/springmedium/Controller024.java"),
        )
        .unwrap();
        assert!(controller.contains("@RequestMapping(\"/api/c024\")"));
        assert!(controller.contains("@GetMapping(\"/{id}\")"));
        assert!(
            output
                .join("src/main/java/org/springframework/stereotype/Service.java")
                .is_file()
        );
        fs::remove_dir_all(output).unwrap();
    }
}
