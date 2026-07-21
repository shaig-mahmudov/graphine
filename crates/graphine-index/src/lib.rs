use graphine_protocol::{
    ANALYZER_PLACEHOLDER, AnalyzerDiagnostic, AnalyzerSummary, Confidence, GenerationState,
    GraphineError, ProjectId, SCHEMA_VERSION, StableId, SyntheticEdge, SyntheticGraph,
    SyntheticNode,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, instrument};

const MIGRATION_1: &str = r"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
    project_id TEXT PRIMARY KEY,
    canonical_root TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    registered_at_ms INTEGER NOT NULL,
    source_fingerprint TEXT,
    analyzer_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS project_generations (
    project_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    state TEXT NOT NULL CHECK (state IN ('CREATED','BUILDING','READY','FAILED','STALE')),
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    failure_summary TEXT,
    PRIMARY KEY (project_id, generation),
    FOREIGN KEY (project_id) REFERENCES projects(project_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS project_state (
    project_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('CREATED','BUILDING','READY','FAILED','STALE')),
    active_generation INTEGER,
    stale INTEGER NOT NULL DEFAULT 0 CHECK (stale IN (0,1)),
    last_successful_activation_ms INTEGER,
    failure_summary TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(project_id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, active_generation) REFERENCES project_generations(project_id, generation)
);
CREATE TABLE IF NOT EXISTS nodes (
    project_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    stable_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    simple_name TEXT NOT NULL,
    module_name TEXT,
    package_name TEXT,
    file_path TEXT,
    start_line INTEGER,
    end_line INTEGER,
    confidence TEXT NOT NULL,
    provenance TEXT NOT NULL,
    metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json)),
    unresolved_json TEXT NOT NULL CHECK (json_valid(unresolved_json)),
    PRIMARY KEY (project_id, generation, stable_id),
    FOREIGN KEY (project_id, generation) REFERENCES project_generations(project_id, generation) ON DELETE CASCADE,
    CHECK ((file_path IS NULL AND start_line IS NULL AND end_line IS NULL) OR
           (file_path IS NOT NULL AND start_line > 0 AND end_line >= start_line))
);
CREATE TABLE IF NOT EXISTS edges (
    project_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    source_stable_id TEXT NOT NULL,
    target_stable_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    confidence TEXT NOT NULL,
    provenance TEXT NOT NULL,
    metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json)),
    PRIMARY KEY (project_id, generation, source_stable_id, target_stable_id, kind),
    FOREIGN KEY (project_id, generation, source_stable_id) REFERENCES nodes(project_id, generation, stable_id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, generation, target_stable_id) REFERENCES nodes(project_id, generation, stable_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS evidence (
    project_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    stable_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    file_path TEXT NOT NULL,
    start_line INTEGER NOT NULL CHECK (start_line > 0),
    end_line INTEGER NOT NULL CHECK (end_line >= start_line),
    PRIMARY KEY (project_id, generation, stable_id, ordinal),
    FOREIGN KEY (project_id, generation, stable_id) REFERENCES nodes(project_id, generation, stable_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_generations_project_state ON project_generations(project_id, state, generation);
CREATE INDEX IF NOT EXISTS idx_nodes_project_generation ON nodes(project_id, generation);
CREATE INDEX IF NOT EXISTS idx_nodes_stable_id ON nodes(stable_id);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(project_id, generation, kind);
CREATE INDEX IF NOT EXISTS idx_nodes_qualified_name ON nodes(project_id, generation, qualified_name);
CREATE INDEX IF NOT EXISTS idx_nodes_simple_name ON nodes(project_id, generation, simple_name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(project_id, generation, source_stable_id);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(project_id, generation, target_stable_id);
CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(project_id, generation, kind);
";

const MIGRATION_2: &str = r"
ALTER TABLE project_generations ADD COLUMN analyzer_protocol_version INTEGER;
ALTER TABLE project_generations ADD COLUMN analyzer_version TEXT;
ALTER TABLE project_generations ADD COLUMN source_fingerprint TEXT;
ALTER TABLE project_generations ADD COLUMN partial INTEGER NOT NULL DEFAULT 0 CHECK (partial IN (0,1));
ALTER TABLE project_generations ADD COLUMN summary_json TEXT CHECK (summary_json IS NULL OR json_valid(summary_json));
CREATE TABLE analyzer_diagnostics (
    project_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    kind TEXT NOT NULL,
    file_path TEXT,
    start_line INTEGER,
    end_line INTEGER,
    symbol_text TEXT,
    reason TEXT NOT NULL,
    severity TEXT NOT NULL,
    PRIMARY KEY (project_id, generation, ordinal),
    FOREIGN KEY (project_id, generation) REFERENCES project_generations(project_id, generation) ON DELETE CASCADE,
    CHECK ((file_path IS NULL AND start_line IS NULL AND end_line IS NULL) OR
           (file_path IS NOT NULL AND start_line > 0 AND end_line >= start_line))
);
CREATE INDEX idx_diagnostics_project_generation ON analyzer_diagnostics(project_id, generation);
UPDATE projects SET schema_version=2;
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub canonical_root: PathBuf,
    pub display_name: String,
    pub registered_at_ms: i64,
    pub source_fingerprint: Option<String>,
    pub analyzer_version: String,
    pub schema_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStatus {
    pub project: ProjectRecord,
    pub active_generation: Option<i64>,
    pub state: GenerationState,
    pub stale: bool,
    pub last_successful_activation_ms: Option<i64>,
    pub failure_summary: Option<String>,
    pub node_count: u64,
    pub edge_count: u64,
    pub diagnostic_count: u64,
    pub partial: bool,
    pub analyzer_protocol_version: Option<u32>,
    pub analysis_summary: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeRecord {
    pub stable_id: String,
    pub kind: String,
    pub qualified_name: String,
    pub simple_name: String,
    pub module_name: Option<String>,
    pub package_name: Option<String>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub confidence: Confidence,
    pub provenance: String,
    pub metadata: Value,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRecord {
    pub source_stable_id: String,
    pub target_stable_id: String,
    pub kind: String,
    pub confidence: Confidence,
    pub provenance: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSummary {
    pub status: IndexStatus,
    pub node_counts: Vec<(String, u64)>,
    pub edge_counts: Vec<(String, u64)>,
    pub modules: Vec<String>,
    pub packages: Vec<String>,
    pub unresolved_count: u64,
}

#[derive(Debug, Clone)]
pub struct AnalysisIngestion {
    pub protocol_version: u32,
    pub analyzer_version: String,
    pub source_fingerprint: String,
    pub partial: bool,
    pub summary: AnalyzerSummary,
    pub diagnostics: Vec<AnalyzerDiagnostic>,
}

pub struct Database {
    connection: Connection,
}

impl Database {
    /// Opens or creates a Graphine `SQLite` database and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns `database_error` if the database cannot be opened or migrated.
    pub fn open(path: &Path, timeout_ms: u64) -> Result<Self, GraphineError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| GraphineError::Database)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_millis(timeout_ms))
            .map_err(db_error)?;
        let mut database = Self { connection };
        database.migrate()?;
        Ok(database)
    }

    /// Creates a migrated in-memory database.
    ///
    /// # Errors
    ///
    /// Returns `database_error` if `SQLite` initialization fails.
    pub fn open_in_memory() -> Result<Self, GraphineError> {
        let connection = Connection::open_in_memory().map_err(db_error)?;
        let mut database = Self { connection };
        database.migrate()?;
        Ok(database)
    }

    /// Applies all known idempotent migrations.
    ///
    /// # Errors
    ///
    /// Returns `database_error` if a migration cannot commit.
    pub fn migrate(&mut self) -> Result<(), GraphineError> {
        self.connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
            .map_err(db_error)?;
        let transaction = self.connection.transaction().map_err(db_error)?;
        transaction.execute_batch(MIGRATION_1).map_err(db_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at_ms) VALUES (?1, ?2)",
                params![1, now_ms()],
            )
            .map_err(db_error)?;
        let has_version_2: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=2)",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !has_version_2 {
            transaction.execute_batch(MIGRATION_2).map_err(db_error)?;
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at_ms) VALUES (2, ?1)",
                    [now_ms()],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    /// Registers a canonical directory as a Graphine project.
    ///
    /// # Errors
    ///
    /// Returns `invalid_path` for files, missing paths, escaped roots, or disallowed roots.
    pub fn register_project(
        &self,
        root: &Path,
        display_name: Option<&str>,
        allowed_roots: &[PathBuf],
    ) -> Result<ProjectRecord, GraphineError> {
        let canonical = fs::canonicalize(root).map_err(|_| GraphineError::InvalidPath)?;
        if !canonical.is_dir() {
            return Err(GraphineError::InvalidPath);
        }
        if !allowed_roots.is_empty() {
            let mut permitted = false;
            for allowed in allowed_roots {
                let allowed = fs::canonicalize(allowed).map_err(|_| GraphineError::InvalidPath)?;
                if canonical.starts_with(allowed) {
                    permitted = true;
                    break;
                }
            }
            if !permitted {
                return Err(GraphineError::InvalidPath);
            }
        }
        let name = display_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                canonical
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
            })
            .ok_or(GraphineError::InvalidPath)?;
        if name.len() > 200 || name.contains(['\0', '\n', '\r']) {
            return Err(GraphineError::InvalidArgument(
                "invalid display name".to_owned(),
            ));
        }
        let id = ProjectId::from_canonical_root(&canonical);
        self.connection
            .execute(
                "INSERT INTO projects(project_id, canonical_root, display_name, registered_at_ms, analyzer_version, schema_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(canonical_root) DO UPDATE SET display_name=excluded.display_name",
                params![
                    id.as_str(),
                    canonical.to_string_lossy(),
                    name,
                    now_ms(),
                    ANALYZER_PLACEHOLDER,
                    SCHEMA_VERSION
                ],
            )
            .map_err(db_error)?;
        self.connection
            .execute(
                "INSERT OR IGNORE INTO project_state(project_id, state) VALUES (?1, 'CREATED')",
                [id.as_str()],
            )
            .map_err(db_error)?;
        self.project_by_ref(id.as_str())
    }

    /// Removes a registered project and all of its graph generations.
    ///
    /// # Errors
    ///
    /// Returns `project_not_found` when no ID or display name matches.
    pub fn unregister_project(&self, reference: &str) -> Result<(), GraphineError> {
        let project = self.project_by_ref(reference)?;
        let changed = self
            .connection
            .execute(
                "DELETE FROM projects WHERE project_id=?1",
                [project.id.as_str()],
            )
            .map_err(db_error)?;
        if changed == 0 {
            return Err(GraphineError::ProjectNotFound);
        }
        Ok(())
    }

    /// Lists projects in deterministic display-name order.
    ///
    /// # Errors
    ///
    /// Returns `database_error` if the query fails.
    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>, GraphineError> {
        let mut statement = self
            .connection
            .prepare("SELECT project_id, canonical_root, display_name, registered_at_ms, source_fingerprint, analyzer_version, schema_version FROM projects ORDER BY display_name, project_id")
            .map_err(db_error)?;
        statement
            .query_map([], project_from_row)
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    /// Resolves a project by stable ID or case-insensitive display name.
    ///
    /// # Errors
    ///
    /// Returns `project_not_found` when no project matches.
    pub fn project_by_ref(&self, reference: &str) -> Result<ProjectRecord, GraphineError> {
        self.connection
            .query_row(
                "SELECT project_id, canonical_root, display_name, registered_at_ms, source_fingerprint, analyzer_version, schema_version
                 FROM projects WHERE project_id=?1 OR display_name=?1 COLLATE NOCASE",
                [reference],
                project_from_row,
            )
            .optional()
            .map_err(db_error)?
            .ok_or(GraphineError::ProjectNotFound)
    }

    /// Starts a durable BUILDING generation without changing the active graph.
    ///
    /// # Errors
    ///
    /// Returns a generation conflict if a previous build is still BUILDING.
    pub fn begin_generation(&mut self, project: &ProjectId) -> Result<i64, GraphineError> {
        let transaction = self.connection.transaction().map_err(db_error)?;
        let building: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM project_generations WHERE project_id=?1 AND state='BUILDING')",
                [project.as_str()],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if building {
            return Err(GraphineError::GenerationConflict);
        }
        let generation: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(generation), 0) + 1 FROM project_generations WHERE project_id=?1",
                [project.as_str()],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "INSERT INTO project_generations(project_id, generation, state, created_at_ms) VALUES (?1, ?2, 'CREATED', ?3)",
                params![project.as_str(), generation, now_ms()],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE project_generations SET state='BUILDING' WHERE project_id=?1 AND generation=?2",
                params![project.as_str(), generation],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE project_state SET state='BUILDING', failure_summary=NULL WHERE project_id=?1",
                [project.as_str()],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(generation)
    }

    /// Marks an incomplete generation failed while preserving the active generation.
    ///
    /// # Errors
    ///
    /// Returns `database_error` if the failure record cannot commit.
    pub fn fail_generation(
        &mut self,
        project: &ProjectId,
        generation: i64,
        summary: &str,
    ) -> Result<(), GraphineError> {
        let safe_summary: String = summary.chars().take(500).collect();
        let transaction = self.connection.transaction().map_err(db_error)?;
        let changed = transaction
            .execute(
                "UPDATE project_generations SET state='FAILED', completed_at_ms=?3, failure_summary=?4
                 WHERE project_id=?1 AND generation=?2 AND state IN ('CREATED','BUILDING')",
                params![project.as_str(), generation, now_ms(), safe_summary],
            )
            .map_err(db_error)?;
        if changed == 0 {
            return Err(GraphineError::GenerationConflict);
        }
        transaction
            .execute(
                "UPDATE project_state SET state='FAILED', failure_summary=?2 WHERE project_id=?1",
                params![project.as_str(), safe_summary],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)
    }

    /// Validates, loads, and atomically activates a synthetic graph generation.
    ///
    /// # Errors
    ///
    /// Returns a validation, path, generation, or database error. Failed loads are recorded.
    #[instrument(skip_all, fields(project = project_ref))]
    pub fn load_synthetic(
        &mut self,
        project_ref: &str,
        graph: &SyntheticGraph,
    ) -> Result<i64, GraphineError> {
        let project = self.project_by_ref(project_ref)?;
        let generation = self.begin_generation(&project.id)?;
        let result = validate_synthetic(&project.canonical_root, graph)
            .and_then(|()| self.write_and_activate(&project, generation, graph, None));
        if let Err(error) = &result {
            let _ = self.fail_generation(&project.id, generation, error.code());
        }
        result.map(|()| generation)
    }

    /// Loads a validated analyzer graph, diagnostics, and analyzer metadata atomically.
    ///
    /// # Errors
    ///
    /// Returns a validation, path, generation, or database error. Failed loads are recorded.
    pub fn load_analysis(
        &mut self,
        project_ref: &str,
        graph: &SyntheticGraph,
        analysis: &AnalysisIngestion,
    ) -> Result<i64, GraphineError> {
        let project = self.project_by_ref(project_ref)?;
        let generation = self.begin_generation(&project.id)?;
        let result = validate_synthetic(&project.canonical_root, graph).and_then(|()| {
            for diagnostic in &analysis.diagnostics {
                validate_diagnostic(&project.canonical_root, diagnostic)?;
            }
            self.write_and_activate(&project, generation, graph, Some(analysis))
        });
        if let Err(error) = &result {
            let _ = self.fail_generation(&project.id, generation, error.code());
        }
        result.map(|()| generation)
    }

    fn write_and_activate(
        &mut self,
        project: &ProjectRecord,
        generation: i64,
        graph: &SyntheticGraph,
        analysis: Option<&AnalysisIngestion>,
    ) -> Result<(), GraphineError> {
        let transaction = self.connection.transaction().map_err(db_error)?;
        for node in &graph.nodes {
            insert_node(&transaction, project, generation, node)?;
        }
        for edge in &graph.edges {
            insert_edge(&transaction, &project.id, generation, edge)?;
        }
        if let Some(analysis) = analysis {
            insert_analysis_metadata(&transaction, &project.id, generation, analysis)?;
        }
        let node_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE project_id=?1 AND generation=?2",
                params![project.id.as_str(), generation],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if node_count == 0 {
            return Err(GraphineError::InvalidArgument(
                "a generation must contain at least one node".to_owned(),
            ));
        }
        let activated = now_ms();
        transaction
            .execute(
                "UPDATE project_generations SET state='READY', completed_at_ms=?3 WHERE project_id=?1 AND generation=?2 AND state='BUILDING'",
                params![project.id.as_str(), generation, activated],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE project_state SET state='READY', active_generation=?2, stale=0, last_successful_activation_ms=?3, failure_summary=NULL WHERE project_id=?1",
                params![project.id.as_str(), generation, activated],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)
    }

    /// Returns current project/index state and active graph counts.
    ///
    /// # Errors
    ///
    /// Returns `project_not_found` or `database_error`.
    pub fn status(&self, reference: &str) -> Result<IndexStatus, GraphineError> {
        let project = self.project_by_ref(reference)?;
        let (state, active_generation, is_stale, last_success, failure): (
            String,
            Option<i64>,
            bool,
            Option<i64>,
            Option<String>,
        ) = self
            .connection
            .query_row(
                "SELECT state, active_generation, stale, last_successful_activation_ms, failure_summary FROM project_state WHERE project_id=?1",
                [project.id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(db_error)?;
        let (node_count, edge_count, diagnostic_count, partial, protocol, summary) = if let Some(
            generation,
        ) =
            active_generation
        {
            self.connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM nodes WHERE project_id=?1 AND generation=?2),
                            (SELECT COUNT(*) FROM edges WHERE project_id=?1 AND generation=?2),
                            (SELECT COUNT(*) FROM analyzer_diagnostics WHERE project_id=?1 AND generation=?2),
                            partial, analyzer_protocol_version, summary_json
                     FROM project_generations WHERE project_id=?1 AND generation=?2",
                    params![project.id.as_str(), generation],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?, row.get::<_, u64>(2)?, row.get::<_, bool>(3)?, row.get::<_, Option<u32>>(4)?, row.get::<_, Option<String>>(5)?)),
                )
                .map_err(db_error)?
        } else {
            (0, 0, 0, false, None, None)
        };
        Ok(IndexStatus {
            project,
            active_generation,
            state: parse_state(&state)?,
            stale: is_stale,
            last_successful_activation_ms: last_success,
            failure_summary: failure,
            node_count,
            edge_count,
            diagnostic_count,
            partial,
            analyzer_protocol_version: protocol,
            analysis_summary: summary
                .map(|value| serde_json::from_str(&value).map_err(|_| GraphineError::Database))
                .transpose()?,
        })
    }

    /// Returns diagnostics from the active generation in stable ordinal order.
    ///
    /// # Errors
    ///
    /// Returns project, index, or database errors.
    pub fn diagnostics(&self, reference: &str) -> Result<Vec<AnalyzerDiagnostic>, GraphineError> {
        let status = self.ready_status(reference)?;
        let generation = status
            .active_generation
            .ok_or(GraphineError::IndexNotReady)?;
        let mut statement = self.connection.prepare(
            "SELECT kind,file_path,start_line,end_line,symbol_text,reason,severity FROM analyzer_diagnostics
             WHERE project_id=?1 AND generation=?2 ORDER BY ordinal"
        ).map_err(db_error)?;
        statement
            .query_map(params![status.project.id.as_str(), generation], |row| {
                Ok(AnalyzerDiagnostic {
                    kind: row.get(0)?,
                    file_path: row.get(1)?,
                    start_line: row.get(2)?,
                    end_line: row.get(3)?,
                    symbol_text: row.get(4)?,
                    reason: row.get(5)?,
                    severity: row.get(6)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    /// Marks the active index stale without deleting it.
    ///
    /// # Errors
    ///
    /// Returns `project_not_found` or `database_error`.
    pub fn mark_stale(&self, reference: &str) -> Result<(), GraphineError> {
        let project = self.project_by_ref(reference)?;
        self.connection
            .execute(
                "UPDATE project_state SET state='STALE', stale=1 WHERE project_id=?1",
                [project.id.as_str()],
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// Returns active node and edge counts grouped by kind.
    ///
    /// # Errors
    ///
    /// Returns an index or database error.
    pub fn graph_summary(&self, reference: &str) -> Result<GraphSummary, GraphineError> {
        let status = self.ready_status(reference)?;
        let generation = status
            .active_generation
            .ok_or(GraphineError::IndexNotReady)?;
        let nodes = grouped_counts(
            &self.connection,
            "SELECT kind, COUNT(*) FROM nodes WHERE project_id=?1 AND generation=?2 GROUP BY kind ORDER BY kind",
            status.project.id.as_str(),
            generation,
        )?;
        let edges = grouped_counts(
            &self.connection,
            "SELECT kind, COUNT(*) FROM edges WHERE project_id=?1 AND generation=?2 GROUP BY kind ORDER BY kind",
            status.project.id.as_str(),
            generation,
        )?;
        let modules = distinct_strings(&self.connection, "module_name", &status, generation)?;
        let packages = distinct_strings(&self.connection, "package_name", &status, generation)?;
        let unresolved = self.connection.query_row(
            "SELECT (SELECT COUNT(*) FROM nodes WHERE project_id=?1 AND generation=?2 AND unresolved_json <> '[]') +
                    (SELECT COUNT(*) FROM analyzer_diagnostics WHERE project_id=?1 AND generation=?2)",
            params![status.project.id.as_str(), generation],
            |row| row.get(0),
        ).map_err(db_error)?;
        Ok(GraphSummary {
            status,
            node_counts: nodes,
            edge_counts: edges,
            modules,
            packages,
            unresolved_count: unresolved,
        })
    }

    /// Fetches candidate active nodes using bounded `SQLite` lexical filtering.
    ///
    /// # Errors
    ///
    /// Returns an index or database error.
    pub fn search_candidates(
        &self,
        reference: &str,
        query: &str,
        kinds: &[String],
        limit: u32,
    ) -> Result<(IndexStatus, Vec<NodeRecord>), GraphineError> {
        let status = self.ready_status(reference)?;
        let generation = status
            .active_generation
            .ok_or(GraphineError::IndexNotReady)?;
        let escaped_query = query.replace(['%', '_'], "");
        let exact_pattern = format!("%{escaped_query}%");
        let token = escaped_query
            .split(|character: char| !character.is_alphanumeric())
            .find(|value| !value.is_empty())
            .unwrap_or(&escaped_query);
        let token_pattern = format!("%{token}%");
        let mut statement = self.connection.prepare(
            "SELECT stable_id, kind, qualified_name, simple_name, module_name, package_name, file_path, start_line, end_line, confidence, provenance, metadata_json, unresolved_json
             FROM nodes WHERE project_id=?1 AND generation=?2 AND
                (qualified_name LIKE ?3 OR simple_name LIKE ?3 OR stable_id LIKE ?3 OR qualified_name LIKE ?4 OR simple_name LIKE ?4)
             ORDER BY CASE WHEN qualified_name=?5 OR simple_name=?5 THEN 0 ELSE 1 END, stable_id LIMIT ?6"
        ).map_err(db_error)?;
        let rows = statement
            .query_map(
                params![
                    status.project.id.as_str(),
                    generation,
                    exact_pattern,
                    token_pattern,
                    query,
                    limit
                ],
                node_from_row,
            )
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let filtered = if kinds.is_empty() {
            rows
        } else {
            rows.into_iter()
                .filter(|node| {
                    kinds
                        .iter()
                        .any(|kind| kind.eq_ignore_ascii_case(&node.kind))
                })
                .collect()
        };
        Ok((status, filtered))
    }

    /// Fetches one node from the active generation.
    ///
    /// # Errors
    ///
    /// Returns `symbol_not_found`, an index error, or a database error.
    pub fn node(
        &self,
        reference: &str,
        stable_id: &StableId,
    ) -> Result<(IndexStatus, NodeRecord), GraphineError> {
        let status = self.ready_status(reference)?;
        let generation = status
            .active_generation
            .ok_or(GraphineError::IndexNotReady)?;
        let node = self.connection.query_row(
            "SELECT stable_id, kind, qualified_name, simple_name, module_name, package_name, file_path, start_line, end_line, confidence, provenance, metadata_json, unresolved_json
             FROM nodes WHERE project_id=?1 AND generation=?2 AND stable_id=?3",
            params![status.project.id.as_str(), generation, stable_id.as_str()],
            node_from_row,
        ).optional().map_err(db_error)?.ok_or(GraphineError::SymbolNotFound)?;
        Ok((status, node))
    }

    /// Fetches deterministic direct edges for a node.
    ///
    /// # Errors
    ///
    /// Returns an index or database error.
    pub fn direct_edges(
        &self,
        status: &IndexStatus,
        stable_id: &str,
        inbound: bool,
        kinds: &[String],
        limit: u32,
    ) -> Result<Vec<EdgeRecord>, GraphineError> {
        let generation = status
            .active_generation
            .ok_or(GraphineError::IndexNotReady)?;
        let column = if inbound {
            "target_stable_id"
        } else {
            "source_stable_id"
        };
        let sql = format!(
            "SELECT source_stable_id, target_stable_id, kind, confidence, provenance, metadata_json FROM edges
             WHERE project_id=?1 AND generation=?2 AND {column}=?3 ORDER BY kind, source_stable_id, target_stable_id LIMIT ?4"
        );
        let mut statement = self.connection.prepare(&sql).map_err(db_error)?;
        let rows = statement
            .query_map(
                params![status.project.id.as_str(), generation, stable_id, limit],
                edge_from_row,
            )
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(if kinds.is_empty() {
            rows
        } else {
            rows.into_iter()
                .filter(|edge| {
                    kinds
                        .iter()
                        .any(|kind| kind.eq_ignore_ascii_case(&edge.kind))
                })
                .collect()
        })
    }

    fn ready_status(&self, reference: &str) -> Result<IndexStatus, GraphineError> {
        let status = self.status(reference)?;
        if status.active_generation.is_none() {
            return Err(GraphineError::IndexNotReady);
        }
        Ok(status)
    }
}

fn insert_node(
    transaction: &Transaction<'_>,
    project: &ProjectRecord,
    generation: i64,
    node: &SyntheticNode,
) -> Result<(), GraphineError> {
    let simple = node
        .simple_name
        .clone()
        .unwrap_or_else(|| derive_simple_name(&node.qualified_name));
    let metadata = serde_json::to_string(&node.metadata)
        .map_err(|_| GraphineError::InvalidArgument("malformed metadata".to_owned()))?;
    let unresolved =
        serde_json::to_string(&node.unresolved).map_err(|_| GraphineError::Internal)?;
    transaction.execute(
        "INSERT INTO nodes(project_id,generation,stable_id,kind,qualified_name,simple_name,module_name,package_name,file_path,start_line,end_line,confidence,provenance,metadata_json,unresolved_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![project.id.as_str(),generation,node.stable_id,node.kind,node.qualified_name,simple,node.module_name,node.package_name,node.file_path,node.start_line,node.end_line,node.confidence.to_string(),node.provenance,metadata,unresolved],
    ).map_err(db_error)?;
    if let (Some(file), Some(start), Some(end)) = (&node.file_path, node.start_line, node.end_line)
    {
        transaction.execute(
            "INSERT INTO evidence(project_id,generation,stable_id,ordinal,file_path,start_line,end_line) VALUES (?1,?2,?3,0,?4,?5,?6)",
            params![project.id.as_str(), generation, node.stable_id, file, start, end],
        ).map_err(db_error)?;
    }
    Ok(())
}

fn insert_analysis_metadata(
    transaction: &Transaction<'_>,
    project: &ProjectId,
    generation: i64,
    analysis: &AnalysisIngestion,
) -> Result<(), GraphineError> {
    let summary = serde_json::to_string(&analysis.summary).map_err(|_| GraphineError::Internal)?;
    transaction
        .execute(
            "UPDATE project_generations SET analyzer_protocol_version=?3, analyzer_version=?4,
         source_fingerprint=?5, partial=?6, summary_json=?7 WHERE project_id=?1 AND generation=?2",
            params![
                project.as_str(),
                generation,
                analysis.protocol_version,
                analysis.analyzer_version,
                analysis.source_fingerprint,
                analysis.partial,
                summary
            ],
        )
        .map_err(db_error)?;
    transaction.execute(
        "UPDATE projects SET source_fingerprint=?2, analyzer_version=?3, schema_version=?4 WHERE project_id=?1",
        params![project.as_str(), analysis.source_fingerprint, analysis.analyzer_version, SCHEMA_VERSION],
    ).map_err(db_error)?;
    for (ordinal, diagnostic) in analysis.diagnostics.iter().enumerate() {
        transaction.execute(
            "INSERT INTO analyzer_diagnostics(project_id,generation,ordinal,kind,file_path,start_line,end_line,symbol_text,reason,severity)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![project.as_str(), generation, ordinal, diagnostic.kind, diagnostic.file_path,
                diagnostic.start_line, diagnostic.end_line, diagnostic.symbol_text, diagnostic.reason, diagnostic.severity],
        ).map_err(db_error)?;
    }
    Ok(())
}

fn insert_edge(
    transaction: &Transaction<'_>,
    project: &ProjectId,
    generation: i64,
    edge: &SyntheticEdge,
) -> Result<(), GraphineError> {
    let metadata = serde_json::to_string(&edge.metadata)
        .map_err(|_| GraphineError::InvalidArgument("malformed metadata".to_owned()))?;
    transaction.execute(
        "INSERT INTO edges(project_id,generation,source_stable_id,target_stable_id,kind,confidence,provenance,metadata_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![project.as_str(),generation,edge.source_stable_id,edge.target_stable_id,edge.kind,edge.confidence.to_string(),edge.provenance,metadata],
    ).map_err(db_error)?;
    Ok(())
}

/// Validates a repository-relative path and resolves it without allowing root escape.
///
/// # Errors
///
/// Returns `invalid_path` for traversal, absolute paths, missing files, or escaping symlinks.
pub fn validate_evidence_path(root: &Path, value: &str) -> Result<String, GraphineError> {
    if value.is_empty()
        || value.contains(['\0', '\u{fffd}'])
        || value.starts_with(['/', '\\'])
        || value.as_bytes().get(1) == Some(&b':')
    {
        return Err(GraphineError::InvalidPath);
    }
    let parts: Vec<_> = value.split(['/', '\\']).collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        return Err(GraphineError::InvalidPath);
    }
    let relative: PathBuf = parts.iter().collect();
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GraphineError::InvalidPath);
    }
    let canonical_root = fs::canonicalize(root).map_err(|_| GraphineError::InvalidPath)?;
    let target =
        fs::canonicalize(canonical_root.join(&relative)).map_err(|_| GraphineError::InvalidPath)?;
    if !target.starts_with(&canonical_root) || !target.is_file() {
        return Err(GraphineError::InvalidPath);
    }
    Ok(parts.join("/"))
}

fn validate_synthetic(root: &Path, graph: &SyntheticGraph) -> Result<(), GraphineError> {
    if graph.project.trim().is_empty() {
        return Err(GraphineError::InvalidArgument(
            "synthetic project label is empty".to_owned(),
        ));
    }
    let mut ids = BTreeSet::new();
    for node in &graph.nodes {
        StableId::parse(node.stable_id.clone())?;
        if !ids.insert(node.stable_id.as_str()) {
            return Err(GraphineError::InvalidArgument(
                "duplicate stable ID".to_owned(),
            ));
        }
        if !node.metadata.is_object() {
            return Err(GraphineError::InvalidArgument(
                "malformed metadata".to_owned(),
            ));
        }
        match (&node.file_path, node.start_line, node.end_line) {
            (None, None, None) => {}
            (Some(path), Some(start), Some(end)) if start > 0 && end >= start => {
                validate_evidence_path(root, path)?;
            }
            _ => {
                return Err(GraphineError::InvalidArgument(
                    "invalid evidence range".to_owned(),
                ));
            }
        }
    }
    let mut edges = BTreeSet::new();
    for edge in &graph.edges {
        if !ids.contains(edge.source_stable_id.as_str())
            || !ids.contains(edge.target_stable_id.as_str())
        {
            return Err(GraphineError::InvalidArgument(
                "foreign edge endpoint".to_owned(),
            ));
        }
        if edge.kind.trim().is_empty() || !edge.metadata.is_object() {
            return Err(GraphineError::InvalidArgument("malformed edge".to_owned()));
        }
        if !edges.insert((&edge.source_stable_id, &edge.target_stable_id, &edge.kind)) {
            return Err(GraphineError::InvalidArgument("duplicate edge".to_owned()));
        }
    }
    Ok(())
}

fn validate_diagnostic(root: &Path, diagnostic: &AnalyzerDiagnostic) -> Result<(), GraphineError> {
    match (
        &diagnostic.file_path,
        diagnostic.start_line,
        diagnostic.end_line,
    ) {
        (None, None, None) => Ok(()),
        (Some(path), Some(start), Some(end)) if start > 0 && end >= start => {
            validate_evidence_path(root, path).map(|_| ())
        }
        _ => Err(GraphineError::InvalidArgument(
            "invalid diagnostic range".to_owned(),
        )),
    }
}

fn derive_simple_name(qualified: &str) -> String {
    qualified
        .rsplit(['.', '#'])
        .next()
        .unwrap_or(qualified)
        .split('(')
        .next()
        .unwrap_or(qualified)
        .to_owned()
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    let id: String = row.get(0)?;
    Ok(ProjectRecord {
        id: ProjectId::parse(id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        canonical_root: PathBuf::from(row.get::<_, String>(1)?),
        display_name: row.get(2)?,
        registered_at_ms: row.get(3)?,
        source_fingerprint: row.get(4)?,
        analyzer_version: row.get(5)?,
        schema_version: row.get(6)?,
    })
}

fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRecord> {
    let confidence: String = row.get(9)?;
    let metadata: String = row.get(11)?;
    let unresolved: String = row.get(12)?;
    Ok(NodeRecord {
        stable_id: row.get(0)?,
        kind: row.get(1)?,
        qualified_name: row.get(2)?,
        simple_name: row.get(3)?,
        module_name: row.get(4)?,
        package_name: row.get(5)?,
        file_path: row.get(6)?,
        start_line: row.get(7)?,
        end_line: row.get(8)?,
        confidence: confidence
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        provenance: row.get(10)?,
        metadata: serde_json::from_str(&metadata).map_err(|_| rusqlite::Error::InvalidQuery)?,
        unresolved: serde_json::from_str(&unresolved).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EdgeRecord> {
    let confidence: String = row.get(3)?;
    let metadata: String = row.get(5)?;
    Ok(EdgeRecord {
        source_stable_id: row.get(0)?,
        target_stable_id: row.get(1)?,
        kind: row.get(2)?,
        confidence: confidence
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        provenance: row.get(4)?,
        metadata: serde_json::from_str(&metadata).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn grouped_counts(
    connection: &Connection,
    sql: &str,
    project: &str,
    generation: i64,
) -> Result<Vec<(String, u64)>, GraphineError> {
    let mut statement = connection.prepare(sql).map_err(db_error)?;
    statement
        .query_map(params![project, generation], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
}

fn distinct_strings(
    connection: &Connection,
    column: &str,
    status: &IndexStatus,
    generation: i64,
) -> Result<Vec<String>, GraphineError> {
    let sql = format!(
        "SELECT DISTINCT {column} FROM nodes WHERE project_id=?1 AND generation=?2 AND {column} IS NOT NULL ORDER BY {column}"
    );
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    statement
        .query_map(params![status.project.id.as_str(), generation], |row| {
            row.get(0)
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
}

fn parse_state(value: &str) -> Result<GenerationState, GraphineError> {
    match value {
        "CREATED" => Ok(GenerationState::Created),
        "BUILDING" => Ok(GenerationState::Building),
        "READY" => Ok(GenerationState::Ready),
        "FAILED" => Ok(GenerationState::Failed),
        "STALE" => Ok(GenerationState::Stale),
        _ => Err(GraphineError::Database),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[allow(clippy::needless_pass_by_value)]
fn db_error(error: rusqlite::Error) -> GraphineError {
    debug!(?error, "sqlite operation failed");
    GraphineError::Database
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_project(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("graphine-{label}-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        let mut file = fs::File::create(root.join("src/Sample.java")).unwrap();
        writeln!(file, "class Sample {{}}").unwrap();
        root
    }

    fn graph(name: &str) -> SyntheticGraph {
        SyntheticGraph {
            project: name.to_owned(),
            nodes: vec![SyntheticNode {
                stable_id: "type:example.Sample".to_owned(),
                kind: "TYPE".to_owned(),
                qualified_name: "example.Sample".to_owned(),
                simple_name: None,
                module_name: Some("app".to_owned()),
                package_name: Some("example".to_owned()),
                file_path: Some("src/Sample.java".to_owned()),
                start_line: Some(1),
                end_line: Some(1),
                confidence: Confidence::CompilerResolved,
                provenance: "test".to_owned(),
                metadata: serde_json::json!({"annotation": "Fixture"}),
                unresolved: Vec::new(),
            }],
            edges: Vec::new(),
        }
    }

    #[test]
    fn migrations_create_required_tables_and_indexes() {
        let database = Database::open_in_memory().unwrap();
        for name in [
            "projects",
            "project_generations",
            "project_state",
            "nodes",
            "edges",
            "evidence",
            "analyzer_diagnostics",
            "idx_nodes_stable_id",
            "idx_edges_source",
            "idx_diagnostics_project_generation",
        ] {
            let exists: bool = database
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "{name}");
        }
    }

    #[test]
    fn activation_and_failure_preserve_previous_generation() {
        let root = temp_project("lifecycle");
        let mut database = Database::open_in_memory().unwrap();
        let project = database
            .register_project(&root, Some("sample"), &[])
            .unwrap();
        let first = database.load_synthetic("sample", &graph("sample")).unwrap();
        assert_eq!(
            database.status("sample").unwrap().active_generation,
            Some(first)
        );
        let mut invalid = graph("sample");
        invalid.edges.push(SyntheticEdge {
            source_stable_id: "type:example.Sample".to_owned(),
            target_stable_id: "type:missing.Other".to_owned(),
            kind: "CALLS".to_owned(),
            confidence: Confidence::CompilerResolved,
            provenance: "test".to_owned(),
            metadata: serde_json::json!({}),
        });
        assert!(database.load_synthetic("sample", &invalid).is_err());
        let status = database.status("sample").unwrap();
        assert_eq!(status.active_generation, Some(first));
        assert_eq!(status.state, GenerationState::Failed);
        assert_eq!(status.node_count, 1);
        drop(project);
    }

    #[test]
    fn analyzer_metadata_and_diagnostics_activate_atomically() {
        let root = temp_project("analysis-ingestion");
        let mut database = Database::open_in_memory().unwrap();
        database
            .register_project(&root, Some("analysis"), &[])
            .unwrap();
        let diagnostic = AnalyzerDiagnostic {
            kind: "unresolved_binding".to_owned(),
            file_path: Some("src/Sample.java".to_owned()),
            start_line: Some(1),
            end_line: Some(1),
            symbol_text: Some("Missing".to_owned()),
            reason: "type binding was not resolved".to_owned(),
            severity: "warning".to_owned(),
        };
        let ingestion = AnalysisIngestion {
            protocol_version: 1,
            analyzer_version: "test-analyzer".to_owned(),
            source_fingerprint: "fixture-fingerprint".to_owned(),
            partial: true,
            summary: AnalyzerSummary {
                files_discovered: 1,
                files_parsed: 1,
                nodes_emitted: 1,
                status: "partial".to_owned(),
                ..AnalyzerSummary::default()
            },
            diagnostics: vec![diagnostic.clone()],
        };
        let generation = database
            .load_analysis("analysis", &graph("analysis"), &ingestion)
            .unwrap();
        let status = database.status("analysis").unwrap();
        assert_eq!(status.active_generation, Some(generation));
        assert_eq!(status.analyzer_protocol_version, Some(1));
        assert_eq!(status.diagnostic_count, 1);
        assert!(status.partial);
        assert_eq!(
            status.analysis_summary,
            Some(serde_json::to_value(&ingestion.summary).unwrap())
        );
        assert_eq!(database.diagnostics("analysis").unwrap(), vec![diagnostic]);

        let mut invalid = ingestion;
        invalid.diagnostics[0].file_path = Some("../outside.java".to_owned());
        assert!(
            database
                .load_analysis("analysis", &graph("analysis"), &invalid)
                .is_err()
        );
        let after_failure = database.status("analysis").unwrap();
        assert_eq!(after_failure.active_generation, Some(generation));
        assert_eq!(database.diagnostics("analysis").unwrap().len(), 1);
    }

    #[test]
    fn synthetic_validation_rejects_malformed_graph_shapes() {
        let root = temp_project("malformed");
        let base = graph("malformed");
        let mut duplicate_node = base.clone();
        duplicate_node.nodes.push(duplicate_node.nodes[0].clone());
        let mut invalid_range = base.clone();
        invalid_range.nodes[0].start_line = Some(0);
        let mut invalid_metadata = base.clone();
        invalid_metadata.nodes[0].metadata = serde_json::json!("not-an-object");
        let mut invalid_id = base.clone();
        invalid_id.nodes[0].stable_id = "method:create".to_owned();
        let mut duplicate_edge = base.clone();
        let edge = SyntheticEdge {
            source_stable_id: "type:example.Sample".to_owned(),
            target_stable_id: "type:example.Sample".to_owned(),
            kind: "SELF".to_owned(),
            confidence: Confidence::CompilerResolved,
            provenance: "test".to_owned(),
            metadata: serde_json::json!({}),
        };
        duplicate_edge.edges = vec![edge.clone(), edge];
        for (label, invalid) in [
            ("duplicate node", duplicate_node),
            ("invalid range", invalid_range),
            ("invalid metadata", invalid_metadata),
            ("invalid stable ID", invalid_id),
            ("duplicate edge", duplicate_edge),
        ] {
            assert!(validate_synthetic(&root, &invalid).is_err(), "{label}");
        }
    }

    #[test]
    fn interrupted_build_does_not_replace_active_generation() {
        let root = temp_project("interrupted");
        let mut database = Database::open_in_memory().unwrap();
        let project = database
            .register_project(&root, Some("interrupted"), &[])
            .unwrap();
        let active = database
            .load_synthetic("interrupted", &graph("interrupted"))
            .unwrap();
        let building = database.begin_generation(&project.id).unwrap();
        assert!(building > active);
        assert_eq!(
            database.status("interrupted").unwrap().active_generation,
            Some(active)
        );
        database
            .fail_generation(&project.id, building, "interrupted")
            .unwrap();
        database.mark_stale("interrupted").unwrap();
        let status = database.status("interrupted").unwrap();
        assert!(status.stale);
        assert_eq!(status.active_generation, Some(active));
    }

    #[test]
    fn paths_reject_traversal_absolute_and_missing_targets() {
        let root = temp_project("paths");
        fs::write(root.join("src/Sp ace-Δ.java"), "class Unicode {}\n").unwrap();
        assert_eq!(
            validate_evidence_path(&root, "src/Sample.java").unwrap(),
            "src/Sample.java"
        );
        assert_eq!(
            validate_evidence_path(&root, "src/Sp ace-Δ.java").unwrap(),
            "src/Sp ace-Δ.java"
        );
        for bad in [
            "../secret",
            "/etc/passwd",
            "C:\\Windows\\win.ini",
            "src/missing.java",
            "src\\..\\secret",
        ] {
            assert!(validate_evidence_path(&root, bad).is_err(), "{bad}");
        }
        assert!(root.to_string_lossy().contains("graphine-paths"));
    }

    #[test]
    fn symlink_escape_is_rejected_when_symlinks_are_available() {
        let root = temp_project("symlink-root");
        let outside = temp_project("symlink-outside");
        let link = root.join("src/Escape.java");
        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(outside.join("src/Sample.java"), &link);
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(outside.join("src/Sample.java"), &link);
        if created.is_ok() {
            assert!(validate_evidence_path(&root, "src/Escape.java").is_err());
        }
    }

    #[test]
    fn registration_rejects_files_and_honors_allowed_roots() {
        let root = temp_project("registration");
        let database = Database::open_in_memory().unwrap();
        assert!(
            database
                .register_project(&root.join("src/Sample.java"), None, &[])
                .is_err()
        );
        assert!(
            database
                .register_project(&root, Some("unicode-Δ path"), std::slice::from_ref(&root))
                .is_ok()
        );
        let other = temp_project("other-root");
        assert!(
            database
                .register_project(&other, Some("other"), &[root])
                .is_err()
        );
    }
}
