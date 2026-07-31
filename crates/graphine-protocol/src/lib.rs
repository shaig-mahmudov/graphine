use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

pub const SCHEMA_VERSION: i64 = 4;
pub const ANALYZER_PROTOCOL_VERSION: u32 = 2;
pub const ANALYZER_CAPABILITY_JAVA_SEMANTICS: &str = "java_semantics";
pub const ANALYZER_CAPABILITY_SPRING_STATIC_SEMANTICS: &str = "spring_static_semantics";
pub const ANALYZER_CAPABILITY_MAVEN_TRUSTED_MODE: &str = "maven_trusted_mode";
pub const ANALYZER_CAPABILITY_RUST_SEMANTICS: &str = "rust_semantics";
pub const ANALYZER_CAPABILITY_CARGO_SAFE_MODE: &str = "cargo_safe_mode";
pub const ANALYZER_CAPABILITY_CARGO_TRUSTED_MODE: &str = "cargo_trusted_mode";
pub const ANALYZER_PLACEHOLDER: &str = "synthetic-phase-1";
pub const PROJECT_NAMESPACE: Uuid = Uuid::from_u128(0x2bbd_0781_53ac_4e83_9e4b_87ec_ad76_bf89);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLanguage {
    #[default]
    Java,
    Rust,
}

impl fmt::Display for ProjectLanguage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Java => "java",
            Self::Rust => "rust",
        })
    }
}

impl FromStr for ProjectLanguage {
    type Err = GraphineError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "java" => Ok(Self::Java),
            "rust" => Ok(Self::Rust),
            _ => Err(GraphineError::InvalidArgument(format!(
                "unsupported project language: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Confidence {
    RuntimeConfirmed,
    BytecodeConfirmed,
    CompilerResolved,
    FrameworkResolved,
    StaticInferred,
    Ambiguous,
    Unresolved,
}

impl fmt::Display for Confidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let serialized = serde_json::to_value(self).map_err(|_| fmt::Error)?;
        formatter.write_str(serialized.as_str().ok_or(fmt::Error)?)
    }
}

impl FromStr for Confidence {
    type Err = GraphineError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(Value::String(value.to_owned())).map_err(|_| {
            GraphineError::InvalidArgument(format!("unsupported confidence value: {value}"))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    #[must_use]
    pub fn from_canonical_root(root: &Path) -> Self {
        let normalized = root.to_string_lossy().replace('\\', "/");
        Self(format!(
            "project:{}",
            Uuid::new_v5(&PROJECT_NAMESPACE, normalized.as_bytes())
        ))
    }

    /// Parses an externally visible project identifier.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` when the value is not `project:<uuid>`.
    pub fn parse(value: impl Into<String>) -> Result<Self, GraphineError> {
        let value = value.into();
        let suffix = value
            .strip_prefix("project:")
            .ok_or_else(|| GraphineError::InvalidArgument("invalid project ID".to_owned()))?;
        Uuid::parse_str(suffix)
            .map_err(|_| GraphineError::InvalidArgument("invalid project ID".to_owned()))?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StableId(String);

impl StableId {
    /// Parses and validates a stable graph identifier.
    ///
    /// # Errors
    ///
    /// Returns `invalid_stable_id` for an unknown prefix or malformed payload.
    pub fn parse(value: impl Into<String>) -> Result<Self, GraphineError> {
        let value = value.into();
        if value.len() > 1_024 || value.contains(['\0', '\n', '\r']) {
            return Err(GraphineError::InvalidStableId(value));
        }
        let (kind, body) = value
            .split_once(':')
            .ok_or_else(|| GraphineError::InvalidStableId(value.clone()))?;
        let valid = match kind {
            "project" | "crate" | "module" | "package" | "type" | "trait" | "function"
            | "variant" | "const" | "static" | "macro" | "associated_type" | "bean" | "entity"
            | "config" => !body.trim().is_empty(),
            "method" | "constructor" => {
                let Some((owner, method)) = body.split_once('#') else {
                    return Err(GraphineError::InvalidStableId(value));
                };
                !owner.is_empty()
                    && method.contains('(')
                    && method.ends_with(')')
                    && !method.starts_with('(')
                    && (kind != "constructor" || method.starts_with("<init>("))
            }
            "field" => body
                .split_once('#')
                .is_some_and(|(owner, field)| !owner.is_empty() && !field.is_empty()),
            "route" => body
                .split_once(':')
                .is_some_and(|(method, path)| is_http_method(method) && path.starts_with('/')),
            _ => false,
        };
        if !valid {
            return Err(GraphineError::InvalidStableId(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn kind_prefix(&self) -> &str {
        self.0.split_once(':').map_or("", |(kind, _)| kind)
    }
}

impl fmt::Display for StableId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_http_method(value: &str) -> bool {
    matches!(
        value,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GenerationState {
    Created,
    Building,
    Ready,
    Failed,
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailLevel {
    Summary,
    #[default]
    Standard,
    Detailed,
    Evidence,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Inbound,
    #[default]
    Outbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticGraph {
    pub project: String,
    pub nodes: Vec<SyntheticNode>,
    #[serde(default)]
    pub edges: Vec<SyntheticEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticNode {
    pub stable_id: String,
    pub kind: String,
    pub qualified_name: String,
    #[serde(default)]
    pub simple_name: Option<String>,
    #[serde(default)]
    pub module_name: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub namespace_path: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
    pub confidence: Confidence,
    pub provenance: String,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    #[serde(default)]
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyntheticEdge {
    pub source_stable_id: String,
    pub target_stable_id: String,
    pub kind: String,
    pub confidence: Confidence,
    pub provenance: String,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    #[serde(default)]
    pub occurrences: Vec<EdgeOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeOccurrence {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

/// Install-time analyzer metadata shared by the Rust supervisor and JVM worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerHello {
    pub protocol_version: u32,
    pub analyzer_name: String,
    pub analyzer_version: String,
    pub language: ProjectLanguage,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzerMode {
    Safe,
    Trusted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerOptions {
    pub include_method_bodies: bool,
    pub include_field_access: bool,
    pub include_tests: bool,
    #[serde(default)]
    pub explicit_classpath: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoAnalysisOptions {
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub all_features: bool,
    #[serde(default)]
    pub no_default_features: bool,
    #[serde(default)]
    pub target: Option<String>,
}

impl Default for AnalyzerOptions {
    fn default() -> Self {
        Self {
            include_method_bodies: true,
            include_field_access: true,
            include_tests: true,
            explicit_classpath: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzeProjectRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub operation: String,
    #[serde(default)]
    pub language: ProjectLanguage,
    pub project_root: PathBuf,
    pub mode: AnalyzerMode,
    pub source_sets: Vec<String>,
    pub options: AnalyzerOptions,
    pub maven_executable: PathBuf,
    #[serde(default = "default_cargo_executable")]
    pub cargo_executable: PathBuf,
    #[serde(default = "default_rustc_executable")]
    pub rustc_executable: PathBuf,
    #[serde(default)]
    pub cargo: CargoAnalysisOptions,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnalyzerEvent {
    AnalysisStarted {
        protocol_version: u32,
        request_id: String,
        analyzer_name: String,
        language: ProjectLanguage,
        analyzer_version: String,
    },
    ProjectMetadata {
        language: ProjectLanguage,
        fingerprint: String,
        modules: Vec<String>,
        #[serde(default = "empty_object")]
        configuration: Value,
    },
    ModuleDiscovered {
        name: String,
        root: String,
        source_roots: Vec<String>,
    },
    Node {
        #[serde(flatten)]
        node: SyntheticNode,
    },
    Edge {
        source: String,
        target: String,
        kind: String,
        confidence: Confidence,
        provenance: String,
        #[serde(default = "empty_object")]
        metadata: Value,
        #[serde(default)]
        occurrences: Vec<EdgeOccurrence>,
    },
    Diagnostic {
        diagnostic: AnalyzerDiagnostic,
    },
    AnalysisSummary {
        summary: AnalyzerSummary,
    },
    AnalysisFailed {
        code: String,
        message: String,
    },
    AnalysisCompleted {
        status: AnalysisCompletionStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerDiagnostic {
    pub kind: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
    #[serde(default)]
    pub symbol_text: Option<String>,
    pub reason: String,
    #[serde(default = "default_warning_severity")]
    pub severity: String,
}

fn default_warning_severity() -> String {
    "warning".to_owned()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerSummary {
    #[serde(default)]
    pub language: ProjectLanguage,
    pub files_discovered: u64,
    pub files_parsed: u64,
    pub files_failed: u64,
    pub bindings_resolved: u64,
    pub bindings_unresolved: u64,
    pub nodes_emitted: u64,
    pub edges_emitted: u64,
    pub duration_ms: u64,
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<String, bool>,
    #[serde(default)]
    pub timings_ms: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, u64>,
    #[serde(default = "empty_object")]
    pub configuration: Value,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisCompletionStatus {
    Complete,
    Partial,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub project: String,
    pub generation: Option<i64>,
    pub stale: bool,
    pub partial: bool,
    #[serde(default)]
    pub completeness: Completeness,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    pub complete: bool,
    #[serde(default = "empty_object", skip_serializing_if = "Value::is_null")]
    pub summary: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguities: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default_uncertainty")]
    pub uncertainty: UncertaintyState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<EvidenceRef>,
    pub pagination: Pagination,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<Value>,
    pub budget: Budget,
    #[serde(default, skip_serializing_if = "is_default_truncation")]
    pub truncation: TruncationState,
}

fn is_default_uncertainty(value: &UncertaintyState) -> bool {
    value == &UncertaintyState::default()
}

fn is_default_truncation(value: &TruncationState) -> bool {
    value == &TruncationState::default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completeness {
    pub status: String,
    pub scope: String,
    pub facts_truncated: bool,
    pub evidence_truncated: bool,
    pub uncertainty_truncated: bool,
}

impl Default for Completeness {
    fn default() -> Self {
        Self {
            status: "complete".to_owned(),
            scope: "requested scope".to_owned(),
            facts_truncated: false,
            evidence_truncated: false,
            uncertainty_truncated: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncertaintyState {
    pub unresolved_count: u64,
    pub ambiguity_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguities_truncated: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncationState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguities_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_truncated: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    pub has_more: bool,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub requested_tokens: u32,
    pub estimated_tokens: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GraphineConfig {
    pub data_dir: PathBuf,
    pub log_level: String,
    pub default_token_budget: u32,
    pub maximum_token_budget: u32,
    pub maximum_query_depth: u32,
    pub maximum_result_count: u32,
    pub maximum_evidence_lines: u32,
    pub maximum_evidence_bytes: usize,
    pub evidence_excluded_paths: Vec<String>,
    pub allowed_repository_roots: Vec<PathBuf>,
    pub sqlite_timeout_ms: u64,
    pub mcp_transport: String,
    #[serde(alias = "analyzer_jar")]
    pub java_analyzer_jar: Option<PathBuf>,
    pub rust_analyzer_worker: Option<PathBuf>,
    pub java_executable: PathBuf,
    pub maven_executable: PathBuf,
    pub cargo_executable: PathBuf,
    pub rustc_executable: PathBuf,
    pub analyzer_timeout_ms: u64,
    pub analyzer_output_limit_bytes: usize,
    pub allow_partial_activation: bool,
}

impl Default for GraphineConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(".graphine"),
            log_level: "info".to_owned(),
            default_token_budget: 800,
            maximum_token_budget: 4_000,
            maximum_query_depth: 5,
            maximum_result_count: 100,
            maximum_evidence_lines: 200,
            maximum_evidence_bytes: 64 * 1024,
            evidence_excluded_paths: Vec::new(),
            allowed_repository_roots: Vec::new(),
            sqlite_timeout_ms: 5_000,
            mcp_transport: "stdio".to_owned(),
            java_analyzer_jar: None,
            rust_analyzer_worker: None,
            java_executable: PathBuf::from("java"),
            maven_executable: PathBuf::from(if cfg!(windows) { "mvn.cmd" } else { "mvn" }),
            cargo_executable: default_cargo_executable(),
            rustc_executable: default_rustc_executable(),
            analyzer_timeout_ms: 120_000,
            analyzer_output_limit_bytes: 64 * 1024 * 1024,
            allow_partial_activation: false,
        }
    }
}

impl GraphineConfig {
    /// Loads configuration from JSON, or returns safe defaults when no path is supplied.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` when the file is unreadable or malformed.
    pub fn load(path: Option<&Path>) -> Result<Self, GraphineError> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let content = fs::read_to_string(path).map_err(|error| {
            GraphineError::InvalidArgument(format!("cannot read config: {error}"))
        })?;
        serde_json::from_str(&content)
            .map_err(|error| GraphineError::InvalidArgument(format!("invalid config: {error}")))
    }

    /// Validates budget and operational limits.
    ///
    /// # Errors
    ///
    /// Returns `invalid_argument` for unsafe or contradictory values.
    pub fn validate(&self) -> Result<(), GraphineError> {
        if self.default_token_budget < 128
            || self.default_token_budget > self.maximum_token_budget
            || self.maximum_query_depth == 0
            || self.maximum_result_count == 0
            || self.maximum_evidence_lines == 0
            || self.maximum_evidence_bytes < 1024
            || self.sqlite_timeout_ms == 0
            || self.mcp_transport != "stdio"
            || self.analyzer_timeout_ms == 0
            || self.analyzer_output_limit_bytes < 1024 * 1024
        {
            return Err(GraphineError::InvalidArgument(
                "unsafe or unsupported configuration".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum GraphineError {
    #[error("project was not found")]
    ProjectNotFound,
    #[error("project is not registered")]
    ProjectNotRegistered,
    #[error("index is not ready")]
    IndexNotReady,
    #[error("index is stale")]
    IndexStale,
    #[error("symbol was not found")]
    SymbolNotFound,
    #[error("symbol is ambiguous")]
    AmbiguousSymbol,
    #[error("invalid stable ID: {0}")]
    InvalidStableId(String),
    #[error("invalid repository-relative path")]
    InvalidPath,
    #[error("invalid token budget")]
    InvalidTokenBudget,
    #[error("invalid cursor")]
    InvalidCursor,
    #[error("graph generation conflict")]
    GenerationConflict,
    #[error("database operation failed")]
    Database,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("analyzer worker is unavailable")]
    AnalyzerUnavailable,
    #[error("analyzer protocol failed validation")]
    AnalyzerProtocol,
    #[error("analyzer timed out")]
    AnalyzerTimeout,
    #[error("analyzer process failed")]
    AnalyzerFailed,
    #[error("capability is not supported for this project")]
    CapabilityNotSupported,
    #[error("partial analysis activation is disabled")]
    AnalysisPartial,
    #[error("analysis was cancelled")]
    AnalysisCancelled,
    #[error("internal error")]
    Internal,
}

impl GraphineError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProjectNotFound => "project_not_found",
            Self::ProjectNotRegistered => "project_not_registered",
            Self::IndexNotReady => "index_not_ready",
            Self::IndexStale => "index_stale",
            Self::SymbolNotFound => "symbol_not_found",
            Self::AmbiguousSymbol => "ambiguous_symbol",
            Self::InvalidStableId(_) => "invalid_stable_id",
            Self::InvalidPath => "invalid_path",
            Self::InvalidTokenBudget => "invalid_token_budget",
            Self::InvalidCursor => "invalid_cursor",
            Self::GenerationConflict => "generation_conflict",
            Self::Database => "database_error",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::AnalyzerUnavailable => "analyzer_unavailable",
            Self::AnalyzerProtocol => "analyzer_protocol_error",
            Self::AnalyzerTimeout => "analyzer_timeout",
            Self::AnalyzerFailed => "analyzer_failed",
            Self::CapabilityNotSupported => "capability_not_supported",
            Self::AnalysisPartial => "analysis_partial",
            Self::AnalysisCancelled => "analysis_cancelled",
            Self::Internal => "internal_error",
        }
    }

    #[must_use]
    pub fn safe_data(&self) -> Value {
        if matches!(self, Self::CapabilityNotSupported) {
            serde_json::json!({
                "code": self.code(),
                "suggested_tools": ["get_project_map", "search_symbol", "get_symbol_context", "trace_flow"]
            })
        } else {
            serde_json::json!({"code": self.code()})
        }
    }
}

fn default_cargo_executable() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "cargo.exe" } else { "cargo" })
}

fn default_rustc_executable() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "rustc.exe" } else { "rustc" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_validate_kind_specific_shapes() {
        let valid = [
            "type:example.Event",
            "method:example.EventService#create(java.lang.String)",
            "constructor:example.EventService#<init>(example.Repository)",
            "field:example.Event#name",
            "route:POST:/api/events",
            "bean:eventService",
            "config:mail.api-key",
            "crate:app/lib/app",
            "module:crate:app/lib/app::crate",
            "trait:crate:app/lib/app::crate::Store",
            "function:crate:app/lib/app::crate::process()",
            "method:crate:app/lib/app::crate::MemoryStore as crate:app/lib/app::crate::Store#save()",
            "associated_type:crate:app/lib/app::crate::Store::Error",
        ];
        for value in valid {
            assert!(StableId::parse(value).is_ok(), "{value}");
        }
        for value in ["method:create", "route:FETCH:/x", "unknown:value", "type:"] {
            assert!(StableId::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn confidence_uses_contract_spelling() {
        let value = serde_json::to_string(&Confidence::FrameworkResolved).unwrap();
        assert_eq!(value, "\"FRAMEWORK_RESOLVED\"");
        assert_eq!(
            "COMPILER_RESOLVED".parse::<Confidence>().unwrap(),
            Confidence::CompilerResolved
        );
        assert!(serde_json::from_str::<Confidence>("\"CERTAIN\"").is_err());
    }

    #[test]
    fn project_identity_is_stable_for_a_root() {
        let root = Path::new("C:/work/example");
        assert_eq!(
            ProjectId::from_canonical_root(root),
            ProjectId::from_canonical_root(root)
        );
    }

    #[test]
    fn configuration_accepts_safe_partial_overrides_and_rejects_unknown_keys() {
        let config: GraphineConfig =
            serde_json::from_str(r#"{"default_token_budget":900}"#).unwrap();
        assert_eq!(config.default_token_budget, 900);
        assert_eq!(config.maximum_token_budget, 4_000);
        assert!(config.validate().is_ok());
        let aliased: GraphineConfig =
            serde_json::from_str(r#"{"analyzer_jar":"legacy.jar"}"#).unwrap();
        assert_eq!(aliased.java_analyzer_jar, Some(PathBuf::from("legacy.jar")));
        assert!(
            serde_json::from_str::<GraphineConfig>(r#"{"remote_url":"https://example.com"}"#)
                .is_err()
        );
    }
}
