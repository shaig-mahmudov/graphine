use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

pub const SCHEMA_VERSION: i64 = 1;
pub const ANALYZER_PLACEHOLDER: &str = "synthetic-phase-1";
pub const PROJECT_NAMESPACE: Uuid = Uuid::from_u128(0x2bbd_0781_53ac_4e83_9e4b_87ec_ad76_bf89);

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
            "project" | "module" | "package" | "type" | "bean" | "entity" => {
                !body.trim().is_empty()
            }
            "method" => {
                let Some((owner, method)) = body.split_once('#') else {
                    return Err(GraphineError::InvalidStableId(value));
                };
                !owner.is_empty()
                    && method.contains('(')
                    && method.ends_with(')')
                    && !method.starts_with('(')
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
}

/// Reserved Phase 2 analyzer handshake. Phase 1 never launches an analyzer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerHello {
    pub protocol_version: u32,
    pub analyzer_version: String,
    pub capabilities: Vec<String>,
}

/// Reserved Phase 2 request boundary. Paths remain repository-relative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerRequest {
    pub project_id: ProjectId,
    pub generation: i64,
    pub changed_paths: Vec<String>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub project: String,
    pub generation: Option<i64>,
    pub stale: bool,
    pub summary: Value,
    pub facts: Vec<Value>,
    pub unresolved: Vec<String>,
    pub ambiguities: Vec<String>,
    pub evidence_refs: Vec<EvidenceRef>,
    pub pagination: Pagination,
    pub budget: Budget,
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
    pub allowed_repository_roots: Vec<PathBuf>,
    pub sqlite_timeout_ms: u64,
    pub mcp_transport: String,
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
            allowed_repository_roots: Vec::new(),
            sqlite_timeout_ms: 5_000,
            mcp_transport: "stdio".to_owned(),
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
            || self.sqlite_timeout_ms == 0
            || self.mcp_transport != "stdio"
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
            Self::Internal => "internal_error",
        }
    }

    #[must_use]
    pub fn safe_data(&self) -> Value {
        serde_json::json!({"code": self.code()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_validate_kind_specific_shapes() {
        let valid = [
            "type:example.Event",
            "method:example.EventService#create(java.lang.String)",
            "field:example.Event#name",
            "route:POST:/api/events",
            "bean:eventService",
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
        assert!(
            serde_json::from_str::<GraphineConfig>(r#"{"remote_url":"https://example.com"}"#)
                .is_err()
        );
    }
}
