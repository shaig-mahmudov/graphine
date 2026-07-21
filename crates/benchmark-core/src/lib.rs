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
}
