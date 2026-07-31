use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use graphine_analyzer_client::{AnalyzerClient, AnalyzerJarDiscovery, CancellationToken};
use graphine_index::Database;
use graphine_mcp::{McpServer, run_stdio};
use graphine_protocol::{
    ANALYZER_CAPABILITY_CARGO_SAFE_MODE, ANALYZER_CAPABILITY_CARGO_TRUSTED_MODE,
    ANALYZER_CAPABILITY_JAVA_SEMANTICS, ANALYZER_CAPABILITY_MAVEN_TRUSTED_MODE,
    ANALYZER_CAPABILITY_RUST_SEMANTICS, ANALYZER_CAPABILITY_SPRING_STATIC_SEMANTICS,
    ANALYZER_PROTOCOL_VERSION, AnalyzerHello, AnalyzerMode, CargoAnalysisOptions, GraphineConfig,
    GraphineError, ProjectLanguage, SyntheticGraph,
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
    about = "Local-first Java/Spring and Rust semantic graph"
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
        #[arg(long, value_enum, default_value_t = CliProjectLanguage::Auto)]
        language: CliProjectLanguage,
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
    /// Analyze a registered project with its persisted language backend.
    Analyze {
        project: String,
        #[arg(long, value_enum, default_value_t = CliAnalyzerMode::Safe)]
        mode: CliAnalyzerMode,
        #[arg(long)]
        allow_partial: bool,
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        #[arg(long)]
        all_features: bool,
        #[arg(long)]
        no_default_features: bool,
        #[arg(long)]
        target: Option<String>,
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

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliProjectLanguage {
    Auto,
    Java,
    Rust,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CliLanguage {
    Java,
    Rust,
}

impl From<CliLanguage> for ProjectLanguage {
    fn from(value: CliLanguage) -> Self {
        match value {
            CliLanguage::Java => Self::Java,
            CliLanguage::Rust => Self::Rust,
        }
    }
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
    Doctor {
        #[arg(long, value_enum, default_value_t = CliLanguage::Java)]
        language: CliLanguage,
    },
    Version {
        #[arg(long, value_enum, default_value_t = CliLanguage::Java)]
        language: CliLanguage,
    },
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
        Command::Register {
            path,
            name,
            language,
        } => {
            let language = detect_project_language(&path, language)?;
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let project = database.register_project(
                &path,
                name.as_deref(),
                language,
                &config.allowed_repository_roots,
            )?;
            print_json(&json!({
                "project_id": project.id,
                "display_name": project.display_name,
                "canonical_root": project.canonical_root,
                "language": project.language,
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
                        "language": project.language,
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
                "language": status.project.language,
                "analyzer_name": status.analyzer_name,
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
            let projects = database.list_projects()?;
            let java_in_use = projects
                .iter()
                .any(|project| project.language == ProjectLanguage::Java);
            let rust_in_use = projects
                .iter()
                .any(|project| project.language == ProjectLanguage::Rust);
            let java_version = executable_version(&config.java_executable, &["-version"]);
            let maven_available = executable_available(&config.maven_executable);
            let cargo_version = executable_version(&config.cargo_executable, &["--version"]);
            let rustc_version = executable_version(&config.rustc_executable, &["--version"]);
            let client = AnalyzerClient::from_config(&config);
            let java_discovery = client.discovery_for(ProjectLanguage::Java);
            let java_metadata = client.metadata_for(ProjectLanguage::Java);
            let java_analyzer_error = java_metadata.as_ref().err().map(GraphineError::code);
            let java_analyzer_status = analyzer_status(java_discovery, java_metadata.as_ref().ok());
            let java_protocol_compatible = java_metadata
                .as_ref()
                .is_ok_and(|value| value.protocol_version == ANALYZER_PROTOCOL_VERSION);
            let capabilities = derive_doctor_capabilities(
                java_version.is_some(),
                maven_available,
                java_protocol_compatible,
                java_metadata.as_ref().ok(),
            );
            let rust_discovery = client.discovery_for(ProjectLanguage::Rust);
            let rust_metadata = client.metadata_for(ProjectLanguage::Rust);
            let rust_protocol_compatible = rust_metadata
                .as_ref()
                .is_ok_and(|value| value.protocol_version == ANALYZER_PROTOCOL_VERSION);
            let rust_semantics = rustc_version.is_some()
                && cargo_version.is_some()
                && rust_protocol_compatible
                && rust_metadata.as_ref().ok().is_some_and(|metadata| {
                    metadata
                        .capabilities
                        .iter()
                        .any(|capability| capability == ANALYZER_CAPABILITY_RUST_SEMANTICS)
                });
            let mut degraded_reasons = Vec::<String>::new();
            if java_in_use {
                degraded_reasons.extend(
                    degraded_reasons_for_java(
                        java_version.is_some(),
                        java_discovery,
                        java_metadata.as_ref(),
                        capabilities,
                    )
                    .into_iter()
                    .map(str::to_owned),
                );
            }
            if rust_in_use {
                if rustc_version.is_none() {
                    degraded_reasons.push("rustc_unavailable".to_owned());
                }
                if cargo_version.is_none() {
                    degraded_reasons.push("cargo_unavailable".to_owned());
                }
                if !rust_discovery.available {
                    degraded_reasons.push("rust_analyzer_missing".to_owned());
                } else if rust_metadata.is_err() {
                    degraded_reasons.push("rust_analyzer_metadata_unavailable".to_owned());
                } else if !rust_protocol_compatible {
                    degraded_reasons.push("rust_analyzer_protocol_mismatch".to_owned());
                } else if !rust_semantics {
                    degraded_reasons.push("rust_semantics_capability_missing".to_owned());
                }
            }
            print_json(&json!({
                "status": if degraded_reasons.is_empty() { "ok" } else { "degraded" },
                "degraded_reasons": degraded_reasons,
                "database": {
                    "status": "ok",
                    "path": database_path,
                    "schema_version": graphine_protocol::SCHEMA_VERSION
                },
                "registered_projects": projects.len(),
                "transport": config.mcp_transport,
                "java": {"available": java_version.is_some(), "version": java_version},
                "analyzer": {
                    "status": java_analyzer_status,
                    "discovered": java_discovery.available,
                    "available": java_protocol_compatible,
                    "version": java_metadata.as_ref().ok().map(|value| &value.analyzer_version),
                    "protocol_version": java_metadata.as_ref().ok().map(|value| value.protocol_version),
                    "expected_protocol_version": ANALYZER_PROTOCOL_VERSION,
                    "protocol_compatible": java_protocol_compatible,
                    "reported_capabilities": java_metadata.as_ref().ok().map(|value| &value.capabilities),
                    "error": java_analyzer_error,
                    "jar": java_discovery.path,
                    "source": java_discovery.source,
                    "searched": java_discovery.searched,
                },
                "languages": {
                    "java": {
                        "in_use": java_in_use,
                        "toolchain_available": java_version.is_some(),
                        "worker_status": java_analyzer_status,
                        "worker": java_metadata.as_ref().ok(),
                        "worker_path": java_discovery.path,
                    },
                    "rust": {
                        "in_use": rust_in_use,
                        "cargo": {"available":cargo_version.is_some(),"version":cargo_version},
                        "rustc": {"available":rustc_version.is_some(),"version":rustc_version},
                        "worker_status": analyzer_status(rust_discovery, rust_metadata.as_ref().ok()),
                        "worker": rust_metadata.as_ref().ok(),
                        "worker_path": rust_discovery.path,
                        "rust_semantics": rust_semantics,
                        "safe_mode_supported": rust_metadata.as_ref().ok().is_some_and(|metadata| metadata.capabilities.iter().any(|capability| capability == ANALYZER_CAPABILITY_CARGO_SAFE_MODE)),
                        "trusted_mode_supported": rust_metadata.as_ref().ok().is_some_and(|metadata| metadata.capabilities.iter().any(|capability| capability == ANALYZER_CAPABILITY_CARGO_TRUSTED_MODE)),
                    }
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
                    "rust_semantics": rust_semantics,
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
            features,
            all_features,
            no_default_features,
            target,
        } => {
            let mut database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            let client = AnalyzerClient::from_config(&config);
            let outcome = client.analyze_and_ingest_with_options(
                &mut database,
                &project,
                mode.into(),
                allow_partial || config.allow_partial_activation,
                CargoAnalysisOptions {
                    features,
                    all_features,
                    no_default_features,
                    target,
                },
                &CancellationToken::default(),
            )?;
            print_json(&json!({
                "project": project,
                "generation": outcome.generation,
                "language": outcome.language,
                "analyzer_name": outcome.analyzer_name,
                "analyzer_version": outcome.analyzer_version,
                "mode": match mode { CliAnalyzerMode::Safe => "safe", CliAnalyzerMode::Trusted => "trusted" },
                "trust_warning": matches!(mode, CliAnalyzerMode::Trusted).then_some(
                    "Trusted analysis may execute build-tool extensions, build scripts, and procedural macros with the user's OS permissions."
                ),
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
                AnalyzerCommand::Doctor { language } => {
                    let language: ProjectLanguage = language.into();
                    let discovery = client.discovery_for(language);
                    let metadata = client.metadata_for(language);
                    let status = analyzer_status(discovery, metadata.as_ref().ok());
                    let protocol_compatible = metadata
                        .as_ref()
                        .is_ok_and(|value| value.protocol_version == ANALYZER_PROTOCOL_VERSION);
                    print_json(&json!({
                        "status": status,
                        "language": language,
                        "available": protocol_compatible,
                        "analyzer_name": metadata.as_ref().ok().map(|value| &value.analyzer_name),
                        "worker": metadata.as_ref().ok().map(|value| &value.analyzer_version),
                        "protocol_version": metadata.as_ref().ok().map(|value| value.protocol_version),
                        "expected_protocol_version": ANALYZER_PROTOCOL_VERSION,
                        "protocol_compatible": protocol_compatible,
                        "capabilities": metadata.as_ref().ok().map(|value| &value.capabilities),
                        "error": metadata.as_ref().err().map(GraphineError::code),
                        "worker_path": discovery.path,
                        "source": discovery.source,
                        "searched": discovery.searched,
                        "action": match status {
                            "ok" => Value::Null,
                            "protocol_mismatch" => json!("install an analyzer built for the expected protocol version"),
                            _ => match language {
                                ProjectLanguage::Java => json!("set java_analyzer_jar (analyzer_jar remains an alias) or GRAPHINE_ANALYZER_JAR"),
                                ProjectLanguage::Rust => json!("set rust_analyzer_worker or GRAPHINE_RUST_ANALYZER, or install graphine-rust-analyzer beside graphine"),
                            }
                        }
                    }))?;
                }
                AnalyzerCommand::Version { language } => {
                    let language: ProjectLanguage = language.into();
                    print_json(&json!({
                        "language":language,
                        "worker":client.version_for(language)?
                    }))?;
                }
            }
        }
        Command::Diagnostics { project } => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            print_json(&json!({"project":project,"diagnostics":database.diagnostics(&project)?}))?;
        }
    }
    Ok(())
}

fn detect_project_language(path: &Path, requested: CliProjectLanguage) -> Result<ProjectLanguage> {
    let has_java = path.join("pom.xml").is_file();
    let has_rust = path.join("Cargo.toml").is_file();
    match requested {
        CliProjectLanguage::Auto => match (has_java, has_rust) {
            (true, false) => Ok(ProjectLanguage::Java),
            (false, true) => Ok(ProjectLanguage::Rust),
            (true, true) => Err(GraphineError::InvalidArgument(
                "both pom.xml and Cargo.toml exist; pass --language java or --language rust"
                    .to_owned(),
            )
            .into()),
            (false, false) => Err(GraphineError::InvalidArgument(
                "project root must contain pom.xml or Cargo.toml".to_owned(),
            )
            .into()),
        },
        CliProjectLanguage::Java if has_java => Ok(ProjectLanguage::Java),
        CliProjectLanguage::Rust if has_rust => Ok(ProjectLanguage::Rust),
        CliProjectLanguage::Java => Err(GraphineError::InvalidArgument(
            "Java registration requires a root pom.xml".to_owned(),
        )
        .into()),
        CliProjectLanguage::Rust => Err(GraphineError::InvalidArgument(
            "Rust registration requires a root Cargo.toml".to_owned(),
        )
        .into()),
    }
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
        fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
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

fn degraded_reasons_for_java(
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
            analyzer_name: "graphine-java-jdt".to_owned(),
            analyzer_version: "test".to_owned(),
            language: ProjectLanguage::Java,
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

    #[test]
    fn registration_language_auto_detects_rust_and_requires_matching_manifests() {
        let rust = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rust-core");
        assert_eq!(
            detect_project_language(&rust, CliProjectLanguage::Auto).unwrap(),
            ProjectLanguage::Rust
        );
        assert!(detect_project_language(&rust, CliProjectLanguage::Java).is_err());
        let empty = std::env::temp_dir().join(format!(
            "graphine-language-detection-empty-{}",
            std::process::id()
        ));
        fs::create_dir_all(&empty).unwrap();
        assert!(detect_project_language(&empty, CliProjectLanguage::Auto).is_err());
        fs::remove_dir_all(empty).unwrap();
    }
}
