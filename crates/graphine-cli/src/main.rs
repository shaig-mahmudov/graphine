use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use graphine_analyzer_client::{AnalyzerClient, AnalyzerJarDiscovery, CancellationToken};
use graphine_index::Database;
use graphine_mcp::{McpServer, run_stdio};
use graphine_protocol::{
    ANALYZER_CAPABILITY_JAVA_SEMANTICS, ANALYZER_CAPABILITY_MAVEN_TRUSTED_MODE,
    ANALYZER_CAPABILITY_SPRING_STATIC_SEMANTICS, ANALYZER_PROTOCOL_VERSION, AnalyzerHello,
    AnalyzerMode, GraphineConfig, GraphineError, SyntheticGraph,
};
use serde_json::{Value, json};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "graphine",
    version,
    about = "Local-first Java/Spring graph foundation"
)]
struct Cli {
    /// JSON configuration file. Defaults are safe and local-only.
    #[arg(long, env = "GRAPHINE_CONFIG")]
    config: Option<PathBuf>,
    /// Override the configured local data directory.
    #[arg(long, env = "GRAPHINE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Register a local project directory.
    Register {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Unregister a project and remove its local graph data.
    Unregister { project: String },
    /// List registered projects.
    Projects,
    /// Inspect a project's index lifecycle state.
    Status { project: String },
    /// Check configuration and database health.
    Doctor,
    /// Show the effective configuration.
    Config,
    /// Database lifecycle commands.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Load a deterministic synthetic graph JSON file.
    SyntheticIndex {
        project: String,
        fixture_file: PathBuf,
    },
    /// Run the MCP server over STDIO.
    Serve,
    /// Explicitly analyze a registered Maven project with Eclipse JDT.
    Analyze {
        project: String,
        #[arg(long, value_enum, default_value_t = CliAnalyzerMode::Safe)]
        mode: CliAnalyzerMode,
        #[arg(long)]
        allow_partial: bool,
    },
    /// Inspect the standalone analyzer worker.
    Analyzer {
        #[command(subcommand)]
        command: AnalyzerCommand,
    },
    /// Show diagnostics persisted for the active generation.
    Diagnostics { project: String },
}

#[derive(Debug, Subcommand)]
enum DbCommand {
    /// Create the database and apply all migrations.
    Migrate,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliAnalyzerMode {
    Safe,
    Trusted,
}

impl From<CliAnalyzerMode> for AnalyzerMode {
    fn from(value: CliAnalyzerMode) -> Self {
        match value {
            CliAnalyzerMode::Safe => Self::Safe,
            CliAnalyzerMode::Trusted => Self::Trusted,
        }
    }
}

#[derive(Debug, Subcommand)]
enum AnalyzerCommand {
    Doctor,
    Version,
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = GraphineConfig::load(cli.config.as_deref())?;
    if let Some(data_dir) = cli.data_dir {
        config.data_dir = data_dir;
    }
    config.validate()?;
    init_tracing(&config.log_level)?;
    let database_path = config.data_dir.join("graphine.sqlite3");
    match cli.command {
        Command::Register { path, name } => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let project = database.register_project(
                &path,
                name.as_deref(),
                &config.allowed_repository_roots,
            )?;
            print_json(&json!({
                "project_id": project.id,
                "display_name": project.display_name,
                "canonical_root": project.canonical_root,
                "registered_at_ms": project.registered_at_ms,
            }))?;
        }
        Command::Unregister { project } => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            database.unregister_project(&project)?;
            print_json(&json!({"unregistered": project}))?;
        }
        Command::Projects => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let projects: Vec<_> = database
                .list_projects()?
                .into_iter()
                .map(|project| {
                    json!({
                        "project_id": project.id,
                        "display_name": project.display_name,
                        "canonical_root": project.canonical_root,
                        "schema_version": project.schema_version,
                        "analyzer_version": project.analyzer_version,
                    })
                })
                .collect();
            print_json(&json!({"projects": projects}))?;
        }
        Command::Status { project } => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let status = database.status(&project)?;
            print_json(&json!({
                "project": status.project.display_name,
                "project_id": status.project.id,
                "state": status.state,
                "active_generation": status.active_generation,
                "stale": status.stale,
                "last_successful_activation_ms": status.last_successful_activation_ms,
                "node_count": status.node_count,
                "edge_count": status.edge_count,
                "occurrence_count": status.occurrence_count,
                "diagnostic_count": status.diagnostic_count,
                "partial": status.partial,
                "analyzer_protocol_version": status.analyzer_protocol_version,
                "analysis_summary": status.analysis_summary,
                "failure_summary": status.failure_summary,
            }))?;
        }
        Command::Doctor => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let java_version = executable_version(&config.java_executable, &["-version"]);
            let maven_available = executable_available(&config.maven_executable);
            let client = AnalyzerClient::from_config(&config);
            let discovery = client.discovery();
            let metadata = client.metadata();
            let analyzer_error = metadata.as_ref().err().map(GraphineError::code);
            let analyzer_status = analyzer_status(discovery, metadata.as_ref().ok());
            let protocol_compatible = metadata
                .as_ref()
                .is_ok_and(|value| value.protocol_version == ANALYZER_PROTOCOL_VERSION);
            let capabilities = derive_doctor_capabilities(
                java_version.is_some(),
                maven_available,
                protocol_compatible,
                metadata.as_ref().ok(),
            );
            let degraded_reasons = degraded_reasons(
                java_version.is_some(),
                discovery,
                metadata.as_ref(),
                capabilities,
            );
            print_json(&json!({
                "status": if degraded_reasons.is_empty() { "ok" } else { "degraded" },
                "degraded_reasons": degraded_reasons,
                "database": {
                    "status": "ok",
                    "path": database_path,
                    "schema_version": graphine_protocol::SCHEMA_VERSION
                },
                "registered_projects": database.list_projects()?.len(),
                "transport": config.mcp_transport,
                "java": {"available": java_version.is_some(), "version": java_version},
                "analyzer": {
                    "status": analyzer_status,
                    "discovered": discovery.available,
                    "available": protocol_compatible,
                    "version": metadata.as_ref().ok().map(|value| &value.analyzer_version),
                    "protocol_version": metadata.as_ref().ok().map(|value| value.protocol_version),
                    "expected_protocol_version": ANALYZER_PROTOCOL_VERSION,
                    "protocol_compatible": protocol_compatible,
                    "reported_capabilities": metadata.as_ref().ok().map(|value| &value.capabilities),
                    "error": analyzer_error,
                    "jar": discovery.path,
                    "source": discovery.source,
                    "searched": discovery.searched,
                },
                "maven": {
                    "available": maven_available,
                    "version": Value::Null,
                    "inspection": "path_only_no_process",
                    "required_only_for_trusted_mode": true,
                    "supported_by_analyzer": capabilities.maven_trusted_mode.supported,
                    "trusted_mode_available": capabilities.maven_trusted_mode.available
                },
                "capabilities": {
                    "java_indexing": capabilities.java_indexing,
                    "spring_semantics": capabilities.spring_static_semantics,
                    "spring_static_semantics": capabilities.spring_static_semantics,
                    "maven_trusted_mode": capabilities.maven_trusted_mode.available,
                    "gradle": false
                },
                "unsupported": ["gradle"]
            }))?;
        }
        Command::Config => print_json(&serde_json::to_value(&config)?)?,
        Command::Db {
            command: DbCommand::Migrate,
        } => {
            let _database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            print_json(
                &json!({"migrated": true, "schema_version": graphine_protocol::SCHEMA_VERSION}),
            )?;
        }
        Command::SyntheticIndex {
            project,
            fixture_file,
        } => {
            let content = fs::read_to_string(&fixture_file).with_context(|| {
                format!("cannot read synthetic graph {}", fixture_file.display())
            })?;
            let graph: SyntheticGraph = serde_json::from_str(&content)
                .with_context(|| format!("invalid synthetic graph {}", fixture_file.display()))?;
            let mut database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let generation = database.load_synthetic(&project, &graph)?;
            let status = database.status(&project)?;
            print_json(&json!({
                "project": status.project.display_name,
                "generation": generation,
                "active": true,
                "nodes": status.node_count,
                "edges": status.edge_count,
                "occurrences": status.occurrence_count,
            }))?;
        }
        Command::Serve => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let server = McpServer::new(database, config);
            let stdin = io::stdin();
            let stdout = io::stdout();
            run_stdio(&server, stdin.lock(), stdout.lock())?;
        }
        Command::Analyze {
            project,
            mode,
            allow_partial,
        } => {
            let mut database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let client = AnalyzerClient::from_config(&config);
            let outcome = client.analyze_and_ingest(
                &mut database,
                &project,
                mode.into(),
                allow_partial || config.allow_partial_activation,
                &CancellationToken::default(),
            )?;
            print_json(&json!({
                "project": project,
                "generation": outcome.generation,
                "analyzer_version": outcome.analyzer_version,
                "partial": outcome.partial,
                "diagnostics": outcome.diagnostics,
                "worker_round_trip_ms": outcome.worker_round_trip_ms,
                "ingestion_ms": outcome.ingestion_ms,
                "summary": outcome.summary,
            }))?;
        }
        Command::Analyzer { command } => {
            let client = AnalyzerClient::from_config(&config);
            match command {
                AnalyzerCommand::Doctor => {
                    let discovery = client.discovery();
                    let metadata = client.metadata();
                    let status = analyzer_status(discovery, metadata.as_ref().ok());
                    let protocol_compatible = metadata
                        .as_ref()
                        .is_ok_and(|value| value.protocol_version == ANALYZER_PROTOCOL_VERSION);
                    print_json(&json!({
                        "status": status,
                        "available": protocol_compatible,
                        "worker": metadata.as_ref().ok().map(|value| &value.analyzer_version),
                        "protocol_version": metadata.as_ref().ok().map(|value| value.protocol_version),
                        "expected_protocol_version": ANALYZER_PROTOCOL_VERSION,
                        "protocol_compatible": protocol_compatible,
                        "capabilities": metadata.as_ref().ok().map(|value| &value.capabilities),
                        "error": metadata.as_ref().err().map(GraphineError::code),
                        "jar": discovery.path,
                        "source": discovery.source,
                        "searched": discovery.searched,
                        "action": match status {
                            "ok" => Value::Null,
                            "protocol_mismatch" => json!("install an analyzer built for the expected protocol version"),
                            _ => json!("set analyzer_jar/GRAPHINE_ANALYZER_JAR or install graphine-analyzer.jar beside the executable or under lib/")
                        }
                    }))?;
                }
                AnalyzerCommand::Version => print_json(&json!({"worker":client.version()?}))?,
            }
        }
        Command::Diagnostics { project } => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            print_json(&json!({"project":project,"diagnostics":database.diagnostics(&project)?}))?;
        }
    }
    Ok(())
}

fn init_tracing(level: &str) -> Result<()> {
    let filter = EnvFilter::try_new(level).context("invalid log level")?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow!("failed to initialize tracing: {error}"))
}

fn executable_version(executable: &Path, arguments: &[&str]) -> Option<String> {
    let output = ProcessCommand::new(executable)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = if output.stdout.is_empty() {
        String::from_utf8(output.stderr).ok()?
    } else {
        String::from_utf8(output.stdout).ok()?
    };
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().chars().take(200).collect())
}

fn executable_available(executable: &Path) -> bool {
    if executable.is_absolute() || executable.components().count() > 1 {
        return candidate_is_executable(executable);
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let extensions: Vec<String> = if cfg!(windows) && executable.extension().is_none() {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).any(|directory| {
        extensions.iter().any(|extension| {
            let candidate = if extension.is_empty() {
                directory.join(executable)
            } else {
                directory.join(format!("{}{extension}", executable.to_string_lossy()))
            };
            candidate_is_executable(&candidate)
        })
    })
}

fn candidate_is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
    }
    #[cfg(not(unix))]
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DoctorCapabilities {
    java_indexing: bool,
    spring_static_semantics: bool,
    maven_trusted_mode: MavenTrustedModeCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MavenTrustedModeCapability {
    supported: bool,
    available: bool,
}

fn derive_doctor_capabilities(
    java_available: bool,
    maven_available: bool,
    protocol_compatible: bool,
    metadata: Option<&AnalyzerHello>,
) -> DoctorCapabilities {
    let has = |capability: &str| {
        metadata.is_some_and(|value| {
            value
                .capabilities
                .iter()
                .any(|reported| reported == capability)
        })
    };
    let analyzer_usable = java_available && protocol_compatible;
    let java_indexing = analyzer_usable && has(ANALYZER_CAPABILITY_JAVA_SEMANTICS);
    let spring_static_semantics = java_indexing && has(ANALYZER_CAPABILITY_SPRING_STATIC_SEMANTICS);
    let maven_trusted_mode_supported =
        analyzer_usable && has(ANALYZER_CAPABILITY_MAVEN_TRUSTED_MODE);
    DoctorCapabilities {
        java_indexing,
        spring_static_semantics,
        maven_trusted_mode: MavenTrustedModeCapability {
            supported: maven_trusted_mode_supported,
            available: maven_trusted_mode_supported && maven_available,
        },
    }
}

fn analyzer_status(
    discovery: &AnalyzerJarDiscovery,
    metadata: Option<&AnalyzerHello>,
) -> &'static str {
    if !discovery.available {
        "missing"
    } else if metadata.is_none() {
        "unavailable"
    } else if metadata.is_some_and(|value| value.protocol_version != ANALYZER_PROTOCOL_VERSION) {
        "protocol_mismatch"
    } else {
        "ok"
    }
}

fn degraded_reasons(
    java_available: bool,
    discovery: &AnalyzerJarDiscovery,
    metadata: Result<&AnalyzerHello, &GraphineError>,
    capabilities: DoctorCapabilities,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if !java_available {
        reasons.push("java_unavailable");
    }
    if !discovery.available {
        reasons.push("analyzer_missing");
    } else if metadata.is_err() {
        reasons.push("analyzer_metadata_unavailable");
    } else if metadata.is_ok_and(|value| value.protocol_version != ANALYZER_PROTOCOL_VERSION) {
        reasons.push("analyzer_protocol_mismatch");
    }
    if metadata.is_ok_and(|value| value.protocol_version == ANALYZER_PROTOCOL_VERSION) {
        if !capabilities.java_indexing {
            reasons.push("java_indexing_capability_missing");
        }
        if !capabilities.spring_static_semantics {
            reasons.push("spring_static_semantics_capability_missing");
        }
    }
    reasons
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[allow(dead_code)]
fn _assert_local_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(protocol_version: u32, capabilities: &[&str]) -> AnalyzerHello {
        AnalyzerHello {
            protocol_version,
            analyzer_version: "test".to_owned(),
            capabilities: capabilities
                .iter()
                .map(|capability| (*capability).to_owned())
                .collect(),
        }
    }

    #[test]
    fn doctor_derives_installed_java_spring_and_maven_capabilities() {
        let metadata = metadata(
            ANALYZER_PROTOCOL_VERSION,
            &[
                ANALYZER_CAPABILITY_JAVA_SEMANTICS,
                ANALYZER_CAPABILITY_SPRING_STATIC_SEMANTICS,
                ANALYZER_CAPABILITY_MAVEN_TRUSTED_MODE,
            ],
        );
        let capabilities = derive_doctor_capabilities(true, true, true, Some(&metadata));
        assert!(capabilities.java_indexing);
        assert!(capabilities.spring_static_semantics);
        assert!(capabilities.maven_trusted_mode.supported);
        assert!(capabilities.maven_trusted_mode.available);
    }

    #[test]
    fn doctor_does_not_invent_spring_or_maven_availability() {
        let java_only = metadata(
            ANALYZER_PROTOCOL_VERSION,
            &[ANALYZER_CAPABILITY_JAVA_SEMANTICS],
        );
        let capabilities = derive_doctor_capabilities(true, true, true, Some(&java_only));
        assert!(capabilities.java_indexing);
        assert!(!capabilities.spring_static_semantics);
        assert!(!capabilities.maven_trusted_mode.supported);
        assert!(!capabilities.maven_trusted_mode.available);

        let complete = metadata(
            ANALYZER_PROTOCOL_VERSION,
            &[
                ANALYZER_CAPABILITY_JAVA_SEMANTICS,
                ANALYZER_CAPABILITY_MAVEN_TRUSTED_MODE,
            ],
        );
        assert!(
            !derive_doctor_capabilities(true, false, true, Some(&complete))
                .maven_trusted_mode
                .available
        );
    }

    #[test]
    fn doctor_rejects_protocol_mismatch_and_missing_analyzer_metadata() {
        let mismatched = metadata(
            ANALYZER_PROTOCOL_VERSION + 1,
            &[
                ANALYZER_CAPABILITY_JAVA_SEMANTICS,
                ANALYZER_CAPABILITY_SPRING_STATIC_SEMANTICS,
            ],
        );
        let capabilities = derive_doctor_capabilities(true, true, false, Some(&mismatched));
        assert!(!capabilities.java_indexing);
        assert!(!capabilities.spring_static_semantics);
        assert_eq!(
            derive_doctor_capabilities(true, true, false, None),
            DoctorCapabilities {
                java_indexing: false,
                spring_static_semantics: false,
                maven_trusted_mode: MavenTrustedModeCapability {
                    supported: false,
                    available: false,
                },
            }
        );
    }
}
