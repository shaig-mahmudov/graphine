use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use graphine_analyzer_client::{AnalyzerClient, CancellationToken};
use graphine_index::Database;
use graphine_mcp::{McpServer, run_stdio};
use graphine_protocol::{AnalyzerMode, GraphineConfig, SyntheticGraph};
use serde_json::json;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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
                "diagnostic_count": status.diagnostic_count,
                "partial": status.partial,
                "analyzer_protocol_version": status.analyzer_protocol_version,
                "analysis_summary": status.analysis_summary,
                "failure_summary": status.failure_summary,
            }))?;
        }
        Command::Doctor => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            print_json(&json!({
                "status": "ok",
                "schema_version": graphine_protocol::SCHEMA_VERSION,
                "database": database_path,
                "registered_projects": database.list_projects()?.len(),
                "transport": config.mcp_transport,
                "real_java_indexing": false,
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
            let client = AnalyzerClient::from_config(&config, workspace_root());
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
            let client = AnalyzerClient::from_config(&config, workspace_root());
            let version = client.version()?;
            match command {
                AnalyzerCommand::Doctor => print_json(&json!({"status":"ok","worker":version,
                    "jar": config.analyzer_jar.clone().unwrap_or_else(|| workspace_root().join("analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar"))}))?,
                AnalyzerCommand::Version => print_json(&json!({"worker":version}))?,
            }
        }
        Command::Diagnostics { project } => {
            let database = Database::open(&database_path, config.sqlite_timeout_ms)?;
            print_json(&json!({"project":project,"diagnostics":database.diagnostics(&project)?}))?;
        }
    }
    Ok(())
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("graphine-cli must live at crates/graphine-cli")
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

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[allow(dead_code)]
fn _assert_local_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
}
