use graphine_index::{AnalysisIngestion, Database};
use graphine_protocol::{
    ANALYZER_PROTOCOL_VERSION, AnalysisCompletionStatus, AnalyzeProjectRequest, AnalyzerDiagnostic,
    AnalyzerEvent, AnalyzerHello, AnalyzerMode, AnalyzerOptions, AnalyzerSummary,
    CargoAnalysisOptions, GraphineConfig, GraphineError, ProjectLanguage, StableId, SyntheticEdge,
    SyntheticGraph, SyntheticNode,
};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::ffi::OsString;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub struct AnalyzerClient {
    java_executable: PathBuf,
    worker_jar: PathBuf,
    maven_executable: PathBuf,
    rust_worker: PathBuf,
    cargo_executable: PathBuf,
    rustc_executable: PathBuf,
    timeout: Duration,
    output_limit: usize,
    java_discovery: AnalyzerJarDiscovery,
    rust_discovery: AnalyzerJarDiscovery,
    #[cfg(test)]
    command_override: Option<(PathBuf, Vec<OsString>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerJarDiscovery {
    pub path: PathBuf,
    pub source: String,
    pub searched: Vec<PathBuf>,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct AnalysisOutcome {
    pub generation: i64,
    pub summary: AnalyzerSummary,
    pub diagnostics: usize,
    pub partial: bool,
    pub analyzer_name: String,
    pub analyzer_version: String,
    pub language: ProjectLanguage,
    pub worker_round_trip_ms: u64,
    pub ingestion_ms: u64,
}

impl AnalyzerClient {
    #[must_use]
    pub fn from_config(config: &GraphineConfig) -> Self {
        let java_discovery = discover_analyzer_jar(config);
        let rust_discovery = discover_rust_analyzer_worker(config);
        Self {
            java_executable: config.java_executable.clone(),
            worker_jar: java_discovery.path.clone(),
            maven_executable: config.maven_executable.clone(),
            rust_worker: rust_discovery.path.clone(),
            cargo_executable: config.cargo_executable.clone(),
            rustc_executable: config.rustc_executable.clone(),
            timeout: Duration::from_millis(config.analyzer_timeout_ms),
            output_limit: config.analyzer_output_limit_bytes,
            java_discovery,
            rust_discovery,
            #[cfg(test)]
            command_override: None,
        }
    }

    #[must_use]
    pub fn new(
        java_executable: PathBuf,
        worker_jar: PathBuf,
        maven_executable: PathBuf,
        timeout: Duration,
        output_limit: usize,
    ) -> Self {
        let discovery = AnalyzerJarDiscovery {
            available: worker_jar.is_file(),
            path: worker_jar.clone(),
            source: "explicit_constructor".to_owned(),
            searched: vec![worker_jar.clone()],
        };
        Self {
            java_executable,
            worker_jar,
            maven_executable,
            rust_worker: PathBuf::from(if cfg!(windows) {
                "graphine-rust-analyzer.exe"
            } else {
                "graphine-rust-analyzer"
            }),
            cargo_executable: PathBuf::from(if cfg!(windows) { "cargo.exe" } else { "cargo" }),
            rustc_executable: PathBuf::from(if cfg!(windows) { "rustc.exe" } else { "rustc" }),
            timeout,
            output_limit,
            java_discovery: discovery,
            rust_discovery: AnalyzerJarDiscovery {
                path: PathBuf::from(if cfg!(windows) {
                    "graphine-rust-analyzer.exe"
                } else {
                    "graphine-rust-analyzer"
                }),
                source: "not_found".to_owned(),
                searched: Vec::new(),
                available: false,
            },
            #[cfg(test)]
            command_override: None,
        }
    }

    #[must_use]
    pub const fn discovery(&self) -> &AnalyzerJarDiscovery {
        &self.java_discovery
    }

    #[must_use]
    pub const fn discovery_for(&self, language: ProjectLanguage) -> &AnalyzerJarDiscovery {
        match language {
            ProjectLanguage::Java => &self.java_discovery,
            ProjectLanguage::Rust => &self.rust_discovery,
        }
    }

    /// Runs the analyzer and atomically activates its validated graph.
    ///
    /// # Errors
    ///
    /// Returns worker, protocol, cancellation, partial-policy, path, or database errors.
    pub fn analyze_and_ingest(
        &self,
        database: &mut Database,
        project_ref: &str,
        mode: AnalyzerMode,
        allow_partial: bool,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisOutcome, GraphineError> {
        self.analyze_and_ingest_with_options(
            database,
            project_ref,
            mode,
            allow_partial,
            CargoAnalysisOptions::default(),
            cancellation,
        )
    }

    /// Runs the selected project analyzer with language-specific Cargo options.
    ///
    /// # Errors
    ///
    /// Returns worker, protocol, cancellation, partial-policy, path, or database errors.
    pub fn analyze_and_ingest_with_options(
        &self,
        database: &mut Database,
        project_ref: &str,
        mode: AnalyzerMode,
        allow_partial: bool,
        cargo: CargoAnalysisOptions,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisOutcome, GraphineError> {
        let project = database.project_by_ref(project_ref)?;
        if project.language == ProjectLanguage::Java
            && (cargo.all_features
                || cargo.no_default_features
                || cargo.target.is_some()
                || !cargo.features.is_empty())
        {
            return Err(GraphineError::InvalidArgument(
                "Cargo feature and target options apply only to Rust projects".to_owned(),
            ));
        }
        validate_cargo_options(&cargo)?;
        let request_id = format!("index-{}-{}", now_ms(), std::process::id());
        let request = AnalyzeProjectRequest {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            operation: "analyze_project".to_owned(),
            language: project.language,
            project_root: if project.language == ProjectLanguage::Java {
                java_compatible_path(&project.canonical_root)
            } else {
                project.canonical_root.clone()
            },
            mode,
            source_sets: vec!["main".to_owned(), "test".to_owned()],
            options: AnalyzerOptions::default(),
            maven_executable: resolve_executable(&self.maven_executable),
            cargo_executable: resolve_executable(&self.cargo_executable),
            rustc_executable: resolve_executable(&self.rustc_executable),
            cargo,
            timeout_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
        };
        let worker_started = Instant::now();
        let analysis = self.run(&request, cancellation)?;
        let worker_round_trip_ms = elapsed_ms(worker_started);
        let partial = analysis.completion == AnalysisCompletionStatus::Partial;
        if partial && !allow_partial {
            return Err(GraphineError::AnalysisPartial);
        }
        let graph = SyntheticGraph {
            project: project.display_name,
            nodes: analysis.nodes,
            edges: analysis.edges,
        };
        let ingestion = AnalysisIngestion {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            analyzer_name: analysis.analyzer_name.clone(),
            analyzer_version: analysis.analyzer_version.clone(),
            language: project.language,
            source_fingerprint: analysis.fingerprint,
            partial,
            summary: analysis.summary.clone(),
            diagnostics: analysis.diagnostics.clone(),
        };
        let ingestion_started = Instant::now();
        let generation = database.load_analysis(project_ref, &graph, &ingestion)?;
        let ingestion_ms = elapsed_ms(ingestion_started);
        Ok(AnalysisOutcome {
            generation,
            summary: analysis.summary,
            diagnostics: analysis.diagnostics.len(),
            partial,
            analyzer_name: analysis.analyzer_name,
            analyzer_version: analysis.analyzer_version,
            language: project.language,
            worker_round_trip_ms,
            ingestion_ms,
        })
    }

    /// Returns the independently runnable worker version string.
    ///
    /// # Errors
    ///
    /// Returns unavailable, timeout, or worker errors.
    pub fn version(&self) -> Result<String, GraphineError> {
        self.version_for(ProjectLanguage::Java)
    }

    /// Returns the selected independently runnable worker version.
    ///
    /// # Errors
    ///
    /// Returns unavailable, timeout, or worker errors.
    pub fn version_for(&self, language: ProjectLanguage) -> Result<String, GraphineError> {
        self.inspect(language, "--version")
    }

    /// Returns analyzer-reported protocol and packaged capability metadata.
    ///
    /// This inspection does not analyze a repository or start Maven.
    ///
    /// # Errors
    ///
    /// Returns unavailable, timeout, worker, or malformed-metadata errors.
    pub fn metadata(&self) -> Result<AnalyzerHello, GraphineError> {
        self.metadata_for(ProjectLanguage::Java)
    }

    /// Returns metadata for one selected worker.
    ///
    /// # Errors
    ///
    /// Returns unavailable, timeout, worker, or malformed-metadata errors.
    pub fn metadata_for(&self, language: ProjectLanguage) -> Result<AnalyzerHello, GraphineError> {
        let output = self.inspect(language, "--metadata")?;
        let metadata = parse_analyzer_metadata(&output)?;
        if metadata.language != language {
            return Err(GraphineError::AnalyzerProtocol);
        }
        Ok(metadata)
    }

    fn inspect(&self, language: ProjectLanguage, argument: &str) -> Result<String, GraphineError> {
        if !self.discovery_for(language).available {
            return Err(GraphineError::AnalyzerUnavailable);
        }
        let mut command = self.command(language);
        let mut child = command
            .arg(argument)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| GraphineError::AnalyzerUnavailable)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(status) = child
                .try_wait()
                .map_err(|_| GraphineError::AnalyzerFailed)?
            {
                if !status.success() {
                    return Err(GraphineError::AnalyzerFailed);
                }
                let mut bytes = Vec::new();
                child
                    .stdout
                    .take()
                    .ok_or(GraphineError::AnalyzerFailed)?
                    .take(4096)
                    .read_to_end(&mut bytes)
                    .map_err(|_| GraphineError::AnalyzerFailed)?;
                let value =
                    String::from_utf8(bytes).map_err(|_| GraphineError::AnalyzerProtocol)?;
                let value = value.trim();
                if value.is_empty() {
                    return Err(GraphineError::AnalyzerProtocol);
                }
                return Ok(value.to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        }
        terminate(&mut child);
        Err(GraphineError::AnalyzerTimeout)
    }

    fn run(
        &self,
        request: &AnalyzeProjectRequest,
        cancellation: &CancellationToken,
    ) -> Result<RawAnalysis, GraphineError> {
        if !self.discovery_for(request.language).available {
            return Err(GraphineError::AnalyzerUnavailable);
        }
        let mut child = self
            .command(request.language)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| GraphineError::AnalyzerUnavailable)?;
        let serialized = serde_json::to_vec(request).map_err(|_| GraphineError::Internal)?;
        let mut stdin = child.stdin.take().ok_or(GraphineError::AnalyzerFailed)?;
        stdin
            .write_all(&serialized)
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|_| GraphineError::AnalyzerFailed)?;
        drop(stdin);

        let stdout = child.stdout.take().ok_or(GraphineError::AnalyzerFailed)?;
        let stderr = child.stderr.take().ok_or(GraphineError::AnalyzerFailed)?;
        let (sender, receiver) = mpsc::channel();
        let limit = self.output_limit;
        let stdout_thread = thread::spawn(move || stream_lines(stdout, limit, &sender));
        let stderr_thread = thread::spawn(move || read_bounded(stderr, 64 * 1024));
        let deadline = Instant::now() + self.timeout;
        let mut collector = EventCollector::new(request.request_id.clone(), request.language);
        let mut disconnected = false;
        loop {
            if cancellation.is_cancelled() {
                terminate(&mut child);
                return Err(GraphineError::AnalysisCancelled);
            }
            if Instant::now() >= deadline {
                terminate(&mut child);
                return Err(GraphineError::AnalyzerTimeout);
            }
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(StreamItem::Line(line)) => {
                    if let Err(error) = collector.accept(&line) {
                        terminate(&mut child);
                        return Err(error);
                    }
                }
                Ok(StreamItem::LimitExceeded | StreamItem::IoFailure) => {
                    terminate(&mut child);
                    return Err(GraphineError::AnalyzerProtocol);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if disconnected {
                let status = child.wait().map_err(|_| GraphineError::AnalyzerFailed)?;
                let _ = stdout_thread.join();
                let stderr_bytes = stderr_thread.join().unwrap_or_default();
                debug!(
                    stderr_bytes = stderr_bytes.len(),
                    "analyzer worker stderr captured"
                );
                if !status.success() {
                    warn!(
                        diagnostic = %String::from_utf8_lossy(&stderr_bytes)
                            .chars().take(1000).collect::<String>(),
                        "analyzer worker failed"
                    );
                    return Err(GraphineError::AnalyzerFailed);
                }
                break;
            }
        }
        let result = collector.finish()?;
        info!(
            nodes = result.nodes.len(),
            edges = result.edges.len(),
            diagnostics = result.diagnostics.len(),
            "analyzer stream validated"
        );
        Ok(result)
    }

    fn command(&self, language: ProjectLanguage) -> Command {
        #[cfg(test)]
        let mut command = if let Some((program, arguments)) = &self.command_override {
            let mut override_command = Command::new(program);
            override_command.args(arguments);
            override_command
        } else {
            self.language_command(language)
        };
        #[cfg(not(test))]
        let mut command = self.language_command(language);
        let inherited = std::env::vars().collect::<BTreeMap<_, _>>();
        command.env_clear();
        for allowed in [
            "PATH",
            "PATHEXT",
            "JAVA_HOME",
            "HOME",
            "USERPROFILE",
            "TEMP",
            "TMP",
            "SystemRoot",
            "WINDIR",
            "ComSpec",
            "APPDATA",
            "LOCALAPPDATA",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "RUSTUP_TOOLCHAIN",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
        ] {
            if let Some((key, value)) = inherited
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(allowed))
            {
                command.env(key, value);
            }
        }
        #[cfg(unix)]
        command.process_group(0);
        command
    }

    fn language_command(&self, language: ProjectLanguage) -> Command {
        match language {
            ProjectLanguage::Java => self.java_command(),
            ProjectLanguage::Rust => Command::new(&self.rust_worker),
        }
    }

    fn java_command(&self) -> Command {
        let mut command = Command::new(&self.java_executable);
        command.arg("-Xmx1024m").arg("-jar").arg(&self.worker_jar);
        command
    }
}

fn parse_analyzer_metadata(value: &str) -> Result<AnalyzerHello, GraphineError> {
    let metadata: AnalyzerHello =
        serde_json::from_str(value).map_err(|_| GraphineError::AnalyzerProtocol)?;
    if metadata.analyzer_name.trim().is_empty()
        || metadata.analyzer_version.trim().is_empty()
        || metadata.capabilities.iter().any(|capability| {
            capability.trim().is_empty()
                || capability.len() > 100
                || capability.contains(['\0', '\n', '\r'])
        })
        || metadata.capabilities.iter().collect::<BTreeSet<_>>().len()
            != metadata.capabilities.len()
    {
        return Err(GraphineError::AnalyzerProtocol);
    }
    Ok(metadata)
}

#[must_use]
pub fn discover_analyzer_jar(config: &GraphineConfig) -> AnalyzerJarDiscovery {
    let executable = std::env::current_exe().ok();
    let environment = std::env::var_os("GRAPHINE_ANALYZER_JAR").map(PathBuf::from);
    let development = if cfg!(debug_assertions) {
        find_development_root()
    } else {
        None
    };
    discover_analyzer_jar_from(
        config,
        environment,
        executable.as_deref(),
        development.as_deref(),
    )
}

fn discover_analyzer_jar_from(
    config: &GraphineConfig,
    environment: Option<PathBuf>,
    executable: Option<&Path>,
    development_root: Option<&Path>,
) -> AnalyzerJarDiscovery {
    let mut candidates: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(path) = &config.java_analyzer_jar {
        candidates.push((path.clone(), "config"));
    }
    if let Some(path) = environment {
        candidates.push((path, "environment"));
    }
    if let Some(directory) = executable.and_then(Path::parent) {
        candidates.push((
            directory.join("lib/graphine-analyzer.jar"),
            "executable_lib",
        ));
        candidates.push((
            directory.join("graphine-analyzer.jar"),
            "executable_sibling",
        ));
    }
    if let Some(root) = development_root {
        candidates.push((
            root.join("analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar"),
            "development_workspace",
        ));
    }
    let searched: Vec<_> = candidates.iter().map(|(path, _)| path.clone()).collect();
    if let Some((path, source)) = candidates.iter().find(|(path, _)| path.is_file()) {
        return AnalyzerJarDiscovery {
            path: path.clone(),
            source: (*source).to_owned(),
            searched,
            available: true,
        };
    }
    AnalyzerJarDiscovery {
        path: candidates.first().map_or_else(
            || PathBuf::from("graphine-analyzer.jar"),
            |(path, _)| path.clone(),
        ),
        source: "not_found".to_owned(),
        searched,
        available: false,
    }
}

#[must_use]
pub fn discover_rust_analyzer_worker(config: &GraphineConfig) -> AnalyzerJarDiscovery {
    let executable = std::env::current_exe().ok();
    let environment = std::env::var_os("GRAPHINE_RUST_ANALYZER").map(PathBuf::from);
    let development = if cfg!(debug_assertions) {
        find_development_root()
    } else {
        None
    };
    discover_rust_analyzer_worker_from(
        config,
        environment,
        executable.as_deref(),
        development.as_deref(),
    )
}

fn discover_rust_analyzer_worker_from(
    config: &GraphineConfig,
    environment: Option<PathBuf>,
    executable: Option<&Path>,
    development_root: Option<&Path>,
) -> AnalyzerJarDiscovery {
    let file_name = if cfg!(windows) {
        "graphine-rust-analyzer.exe"
    } else {
        "graphine-rust-analyzer"
    };
    let mut candidates: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(path) = &config.rust_analyzer_worker {
        candidates.push((path.clone(), "config"));
    }
    if let Some(path) = environment {
        candidates.push((path, "environment"));
    }
    if let Some(directory) = executable.and_then(Path::parent) {
        candidates.push((directory.join("lib").join(file_name), "executable_lib"));
        candidates.push((directory.join(file_name), "executable_sibling"));
    }
    if let Some(root) = development_root {
        candidates.push((
            root.join("target/debug").join(file_name),
            "development_workspace",
        ));
    }
    let searched: Vec<_> = candidates.iter().map(|(path, _)| path.clone()).collect();
    if let Some((path, source)) = candidates.iter().find(|(path, _)| path.is_file()) {
        return AnalyzerJarDiscovery {
            path: path.clone(),
            source: (*source).to_owned(),
            searched,
            available: true,
        };
    }
    AnalyzerJarDiscovery {
        path: candidates
            .first()
            .map_or_else(|| PathBuf::from(file_name), |(path, _)| path.clone()),
        source: "not_found".to_owned(),
        searched,
        available: false,
    }
}

fn find_development_root() -> Option<PathBuf> {
    let current = std::env::current_dir().ok()?;
    current.ancestors().find_map(|candidate| {
        candidate
            .join("analyzer-jdt/pom.xml")
            .is_file()
            .then(|| candidate.to_path_buf())
    })
}

struct RawAnalysis {
    analyzer_name: String,
    analyzer_version: String,
    fingerprint: String,
    nodes: Vec<SyntheticNode>,
    edges: Vec<SyntheticEdge>,
    diagnostics: Vec<AnalyzerDiagnostic>,
    summary: AnalyzerSummary,
    completion: AnalysisCompletionStatus,
}

struct EventCollector {
    request_id: String,
    language: ProjectLanguage,
    phase: EventPhase,
    analyzer_name: Option<String>,
    analyzer_version: Option<String>,
    fingerprint: Option<String>,
    nodes: BTreeMap<String, SyntheticNode>,
    edges: Vec<SyntheticEdge>,
    edge_keys: BTreeSet<(String, String, String)>,
    diagnostics: Vec<AnalyzerDiagnostic>,
    summary: Option<AnalyzerSummary>,
    completion: Option<AnalysisCompletionStatus>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EventPhase {
    AwaitingStart,
    AwaitingMetadata,
    Modules,
    Nodes,
    Edges,
    Diagnostics,
    AwaitingCompletion,
    Complete,
}

impl EventCollector {
    fn new(request_id: String, language: ProjectLanguage) -> Self {
        Self {
            request_id,
            language,
            phase: EventPhase::AwaitingStart,
            analyzer_name: None,
            analyzer_version: None,
            fingerprint: None,
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            edge_keys: BTreeSet::new(),
            diagnostics: Vec::new(),
            summary: None,
            completion: None,
        }
    }

    fn accept(&mut self, line: &str) -> Result<(), GraphineError> {
        if self.completion.is_some() {
            return Err(GraphineError::AnalyzerProtocol);
        }
        let event: AnalyzerEvent =
            serde_json::from_str(line).map_err(|_| GraphineError::AnalyzerProtocol)?;
        match event {
            AnalyzerEvent::AnalysisStarted {
                protocol_version,
                request_id,
                analyzer_name,
                language,
                analyzer_version,
            } => self.accept_started(
                protocol_version,
                &request_id,
                analyzer_name,
                language,
                analyzer_version,
            )?,
            AnalyzerEvent::ProjectMetadata {
                language,
                fingerprint,
                configuration,
                ..
            } => self.accept_metadata(language, fingerprint, &configuration)?,
            AnalyzerEvent::ModuleDiscovered {
                root, source_roots, ..
            } if self.phase == EventPhase::Modules => {
                if (root != "." && !safe_relative(&root))
                    || source_roots.iter().any(|path| !safe_relative(path))
                {
                    return Err(GraphineError::AnalyzerProtocol);
                }
            }
            AnalyzerEvent::Node { node }
                if matches!(self.phase, EventPhase::Modules | EventPhase::Nodes) =>
            {
                StableId::parse(node.stable_id.clone())?;
                if !node.metadata.is_object()
                    || self.nodes.insert(node.stable_id.clone(), node).is_some()
                {
                    return Err(GraphineError::AnalyzerProtocol);
                }
                self.phase = EventPhase::Nodes;
            }
            AnalyzerEvent::Edge {
                source,
                target,
                kind,
                confidence,
                provenance,
                metadata,
                occurrences,
            } if matches!(
                self.phase,
                EventPhase::Modules | EventPhase::Nodes | EventPhase::Edges
            ) =>
            {
                if kind.is_empty()
                    || !metadata.is_object()
                    || !self
                        .edge_keys
                        .insert((source.clone(), target.clone(), kind.clone()))
                {
                    return Err(GraphineError::AnalyzerProtocol);
                }
                self.edges.push(SyntheticEdge {
                    source_stable_id: source,
                    target_stable_id: target,
                    kind,
                    confidence,
                    provenance,
                    metadata,
                    occurrences,
                });
                self.phase = EventPhase::Edges;
            }
            AnalyzerEvent::Diagnostic { diagnostic }
                if matches!(
                    self.phase,
                    EventPhase::Modules
                        | EventPhase::Nodes
                        | EventPhase::Edges
                        | EventPhase::Diagnostics
                ) =>
            {
                self.diagnostics.push(diagnostic);
                self.phase = EventPhase::Diagnostics;
            }
            AnalyzerEvent::AnalysisSummary { summary } => self.accept_summary(summary)?,
            AnalyzerEvent::AnalysisCompleted { status } => self.accept_completed(status)?,
            AnalyzerEvent::AnalysisFailed { code, message } => {
                warn!(%code, %message, "analyzer reported failure");
                return Err(GraphineError::AnalyzerFailed);
            }
            _ => return Err(GraphineError::AnalyzerProtocol),
        }
        Ok(())
    }

    fn accept_started(
        &mut self,
        protocol_version: u32,
        request_id: &str,
        analyzer_name: String,
        language: ProjectLanguage,
        analyzer_version: String,
    ) -> Result<(), GraphineError> {
        let expected_name = match language {
            ProjectLanguage::Java => "graphine-java-jdt",
            ProjectLanguage::Rust => "graphine-rust-analyzer",
        };
        if self.phase != EventPhase::AwaitingStart
            || protocol_version != ANALYZER_PROTOCOL_VERSION
            || request_id != self.request_id
            || language != self.language
            || analyzer_name != expected_name
        {
            return Err(GraphineError::AnalyzerProtocol);
        }
        self.phase = EventPhase::AwaitingMetadata;
        self.analyzer_name = Some(analyzer_name);
        self.analyzer_version = Some(analyzer_version);
        Ok(())
    }

    fn accept_metadata(
        &mut self,
        language: ProjectLanguage,
        fingerprint: String,
        configuration: &serde_json::Value,
    ) -> Result<(), GraphineError> {
        if self.phase != EventPhase::AwaitingMetadata
            || language != self.language
            || fingerprint.is_empty()
            || !configuration.is_object()
        {
            return Err(GraphineError::AnalyzerProtocol);
        }
        self.fingerprint = Some(fingerprint);
        self.phase = EventPhase::Modules;
        Ok(())
    }

    fn accept_summary(&mut self, summary: AnalyzerSummary) -> Result<(), GraphineError> {
        if !matches!(
            self.phase,
            EventPhase::Modules | EventPhase::Nodes | EventPhase::Edges | EventPhase::Diagnostics
        ) || summary.language != self.language
            || !summary.configuration.is_object()
        {
            return Err(GraphineError::AnalyzerProtocol);
        }
        self.summary = Some(summary);
        self.phase = EventPhase::AwaitingCompletion;
        Ok(())
    }

    fn accept_completed(&mut self, status: AnalysisCompletionStatus) -> Result<(), GraphineError> {
        let expected = match status {
            AnalysisCompletionStatus::Complete => "complete",
            AnalysisCompletionStatus::Partial => "partial",
        };
        if self.phase != EventPhase::AwaitingCompletion
            || self.summary.as_ref().map(|summary| summary.status.as_str()) != Some(expected)
        {
            return Err(GraphineError::AnalyzerProtocol);
        }
        self.completion = Some(status);
        self.phase = EventPhase::Complete;
        Ok(())
    }

    /// Finalizes the collected analyzer events into a raw analysis result.
    ///
    /// Validates that the event stream is complete, its emitted counts match the
    /// collected nodes and edges, and every edge references existing nodes.
    ///
    /// # Errors
    ///
    /// Returns an analyzer protocol error when required data is missing or counts
    /// do not match. Returns a dangling-edge error when an edge references a
    /// missing source or target node.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// # fn example(collector: EventCollector) -> Result<RawAnalysis, GraphineError> {
    /// let analysis = collector.finish()?;
    /// # Ok(analysis)
    /// # }
    /// ```
    fn finish(self) -> Result<RawAnalysis, GraphineError> {
        let summary = self.summary.ok_or(GraphineError::AnalyzerProtocol)?;
        if summary.nodes_emitted != self.nodes.len() as u64
            || summary.edges_emitted != self.edges.len() as u64
        {
            return Err(GraphineError::AnalyzerProtocol);
        }
        for edge in &self.edges {
            let missing = if !self.nodes.contains_key(&edge.source_stable_id) {
                Some(("source", &edge.source_stable_id))
            } else if !self.nodes.contains_key(&edge.target_stable_id) {
                Some(("target", &edge.target_stable_id))
            } else {
                None
            };
            if let Some((endpoint, stable_id)) = missing {
                return Err(GraphineError::AnalyzerProtocolDanglingEdge {
                    endpoint: endpoint.to_owned(),
                    stable_id: stable_id.clone(),
                    edge_kind: edge.kind.clone(),
                });
            }
        }
        Ok(RawAnalysis {
            analyzer_name: self.analyzer_name.ok_or(GraphineError::AnalyzerProtocol)?,
            analyzer_version: self
                .analyzer_version
                .ok_or(GraphineError::AnalyzerProtocol)?,
            fingerprint: self.fingerprint.ok_or(GraphineError::AnalyzerProtocol)?,
            nodes: self.nodes.into_values().collect(),
            edges: self.edges,
            diagnostics: self.diagnostics,
            summary,
            completion: self.completion.ok_or(GraphineError::AnalyzerProtocol)?,
        })
    }
}

enum StreamItem {
    Line(String),
    LimitExceeded,
    IoFailure,
}

fn stream_lines(mut reader: impl Read, limit: usize, sender: &mpsc::Sender<StreamItem>) {
    let mut total = 0_usize;
    let mut line = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => {
                let _ = sender.send(StreamItem::IoFailure);
                return;
            }
        };
        total = total.saturating_add(read);
        if total > limit {
            let _ = sender.send(StreamItem::LimitExceeded);
            return;
        }
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                if let Ok(value) = String::from_utf8(std::mem::take(&mut line)) {
                    if sender.send(StreamItem::Line(value)).is_err() {
                        return;
                    }
                } else {
                    let _ = sender.send(StreamItem::IoFailure);
                    return;
                }
            } else {
                line.push(*byte);
            }
        }
    }
    if !line.is_empty() {
        if let Ok(value) = String::from_utf8(line) {
            let _ = sender.send(StreamItem::Line(value));
        } else {
            let _ = sender.send(StreamItem::IoFailure);
        }
    }
}

fn read_bounded(reader: impl Read, limit: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes);
    bytes
}

fn terminate(child: &mut Child) {
    let process_id = child.id();
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = status {
            warn!(
                ?error,
                process_id, "failed to terminate analyzer process tree"
            );
        }
    }
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-KILL", "--", &format!("-{process_id}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = status {
            warn!(
                ?error,
                process_id, "failed to terminate analyzer process group"
            );
        }
    }
    if let Err(error) = child.kill() {
        warn!(?error, "failed to kill analyzer worker");
    }
    let _ = child.wait();
}

fn safe_relative(value: &str) -> bool {
    !value.starts_with(['/', '\\'])
        && value.as_bytes().get(1) != Some(&b':')
        && !value
            .split(['/', '\\'])
            .any(|part| matches!(part, "" | "." | ".."))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn validate_cargo_options(options: &CargoAnalysisOptions) -> Result<(), GraphineError> {
    if options.all_features && (!options.features.is_empty() || options.no_default_features) {
        return Err(GraphineError::InvalidArgument(
            "--all-features conflicts with --features and --no-default-features".to_owned(),
        ));
    }
    if options.features.len() > 256
        || options.features.iter().any(|feature| {
            feature.is_empty()
                || feature.len() > 200
                || feature.contains(['\0', '\n', '\r', ' ', ','])
        })
        || options
            .target
            .as_ref()
            .is_some_and(|target| target.is_empty() || target.len() > 200)
    {
        return Err(GraphineError::InvalidArgument(
            "invalid Cargo feature or target selection".to_owned(),
        ));
    }
    Ok(())
}

fn java_compatible_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    value
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn resolve_executable(executable: &Path) -> PathBuf {
    if executable.is_absolute() {
        return executable.to_path_buf();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return executable.to_path_buf();
    };
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return candidate;
        }
    }
    executable.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn protocol_rejects_malformed_version_duplicates_and_incomplete_streams() {
        let mut collector = EventCollector::new("id".to_owned(), ProjectLanguage::Java);
        assert!(collector.accept("not json").is_err());
        assert!(collector.accept(&json!({"type":"analysis_started","protocol_version":2,"request_id":"id","analyzer_name":"graphine-rust-analyzer","language":"rust","analyzer_version":"x"}).to_string()).is_err());
        assert!(
            EventCollector::new("id".to_owned(), ProjectLanguage::Java)
                .finish()
                .is_err()
        );
    }

    #[test]
    fn bounded_stream_rejects_oversized_output() {
        let (sender, receiver) = mpsc::channel();
        stream_lines("123456789".as_bytes(), 4, &sender);
        assert!(matches!(
            receiver.recv().unwrap(),
            StreamItem::LimitExceeded
        ));
    }

    #[test]
    fn cancellation_token_is_explicit() {
        let token = CancellationToken::default();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn packaged_analyzer_discovery_is_executable_relative_and_ordered() {
        let root = std::env::temp_dir().join(format!("graphine-discovery-{}", std::process::id()));
        let binary = root.join(if cfg!(windows) {
            "graphine.exe"
        } else {
            "graphine"
        });
        let packaged = root.join("lib/graphine-analyzer.jar");
        fs::create_dir_all(packaged.parent().unwrap()).unwrap();
        fs::write(&packaged, b"jar").unwrap();
        let config = GraphineConfig::default();
        let discovery = discover_analyzer_jar_from(&config, None, Some(&binary), None);
        assert_eq!(discovery.path, packaged);
        assert_eq!(discovery.source, "executable_lib");
        assert!(discovery.available);
        assert_eq!(discovery.searched[0], packaged);
    }

    #[test]
    fn missing_analyzer_discovery_reports_every_searched_location() {
        let root =
            std::env::temp_dir().join(format!("graphine-missing-discovery-{}", std::process::id()));
        let binary = root.join(if cfg!(windows) {
            "graphine.exe"
        } else {
            "graphine"
        });
        let discovery =
            discover_analyzer_jar_from(&GraphineConfig::default(), None, Some(&binary), None);
        assert!(!discovery.available);
        assert_eq!(discovery.source, "not_found");
        assert_eq!(discovery.searched.len(), 2);
    }

    #[test]
    fn analyzer_metadata_preserves_reported_capabilities_and_protocol() {
        let metadata = parse_analyzer_metadata(
            r#"{"protocol_version":2,"analyzer_name":"graphine-java-jdt","analyzer_version":"0.1.0","language":"java","capabilities":["java_semantics","spring_static_semantics","maven_trusted_mode"]}"#,
        )
        .unwrap();
        assert_eq!(metadata.protocol_version, ANALYZER_PROTOCOL_VERSION);
        assert!(
            metadata
                .capabilities
                .contains(&"spring_static_semantics".to_owned())
        );

        let mismatched = parse_analyzer_metadata(
            r#"{"protocol_version":1,"analyzer_name":"graphine-java-jdt","analyzer_version":"old","language":"java","capabilities":["java_semantics"]}"#,
        )
        .unwrap();
        assert_ne!(mismatched.protocol_version, ANALYZER_PROTOCOL_VERSION);
    }

    #[test]
    fn analyzer_metadata_rejects_malformed_or_duplicate_capabilities() {
        assert!(parse_analyzer_metadata("not json").is_err());
        assert!(
            parse_analyzer_metadata(
                r#"{"protocol_version":2,"analyzer_name":"graphine-java-jdt","analyzer_version":"0.1.0","language":"java","capabilities":["java_semantics","java_semantics"]}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn analyzer_events_require_known_stable_ids() {
        let mut collector = EventCollector::new("id".to_owned(), ProjectLanguage::Java);
        collector.accept(&json!({"type":"analysis_started","protocol_version":2,"request_id":"id","analyzer_name":"graphine-java-jdt","language":"java","analyzer_version":"x"}).to_string()).unwrap();
        collector.accept(&json!({"type":"project_metadata","language":"java","fingerprint":"abc","modules":[],"configuration":{}}).to_string()).unwrap();
        let invalid = json!({"type":"node","stable_id":"unknown:x","kind":"TYPE","qualified_name":"x","confidence":"COMPILER_RESOLVED","provenance":"test","metadata":{},"unresolved":[]});
        assert!(collector.accept(&invalid.to_string()).is_err());
    }

    #[test]
    fn analyzer_events_enforce_protocol_v2_order() {
        let mut collector = EventCollector::new("id".to_owned(), ProjectLanguage::Java);
        collector.accept(&json!({"type":"analysis_started","protocol_version":2,"request_id":"id","analyzer_name":"graphine-java-jdt","language":"java","analyzer_version":"x"}).to_string()).unwrap();
        assert!(
            collector
                .accept(
                    &json!({"type":"analysis_summary","summary":{"language":"java","files_discovered":0,"files_parsed":0,"files_failed":0,"bindings_resolved":0,"bindings_unresolved":0,"nodes_emitted":0,"edges_emitted":0,"duration_ms":0,"status":"complete"}})
                        .to_string()
                )
                .is_err()
        );
    }

    #[test]
    fn successful_and_partial_streams_complete_only_with_matching_counts() {
        for (status, expected) in [
            ("complete", AnalysisCompletionStatus::Complete),
            ("partial", AnalysisCompletionStatus::Partial),
        ] {
            let mut collector = EventCollector::new("id".to_owned(), ProjectLanguage::Java);
            for event in [
                json!({"type":"analysis_started","protocol_version":2,"request_id":"id","analyzer_name":"graphine-java-jdt","language":"java","analyzer_version":"test"}),
                json!({"type":"project_metadata","language":"java","fingerprint":"abc","modules":["fixture"],"configuration":{"java_release":"17","classpath_resolution_ms":0}}),
                json!({"type":"module_discovered","name":"fixture","root":".","source_roots":["src/main/java"]}),
                json!({"type":"node","stable_id":"type:sample.Subject","kind":"TYPE","qualified_name":"sample.Subject","file_path":"src/main/java/sample/Subject.java","start_line":1,"end_line":1,"confidence":"COMPILER_RESOLVED","provenance":"test","metadata":{},"unresolved":[]}),
                json!({"type":"analysis_summary","summary":{"language":"java","files_discovered":1,"files_parsed":1,"files_failed":0,"bindings_resolved":1,"bindings_unresolved":0,"nodes_emitted":1,"edges_emitted":0,"duration_ms":1,"timings_ms":{"parsing":1,"total":1},"resources":{"peak_memory_bytes":1},"status":status}}),
                json!({"type":"analysis_completed","status":status}),
            ] {
                collector.accept(&event.to_string()).unwrap();
            }
            assert_eq!(collector.finish().unwrap().completion, expected);
        }
    }

    #[test]
    fn dangling_edge_error_identifies_missing_endpoint_and_kind() {
        for (source, target, endpoint, missing) in [
            (
                "method:sample.Missing#run()",
                "type:sample.Subject",
                "source",
                "method:sample.Missing#run()",
            ),
            (
                "type:sample.Subject",
                "bean:missing",
                "target",
                "bean:missing",
            ),
        ] {
            let mut collector = EventCollector::new("id".to_owned(), ProjectLanguage::Java);
            for event in [
                json!({"type":"analysis_started","protocol_version":2,"request_id":"id","analyzer_name":"graphine-java-jdt","language":"java","analyzer_version":"test"}),
                json!({"type":"project_metadata","language":"java","fingerprint":"abc","modules":[],"configuration":{}}),
                json!({"type":"node","stable_id":"type:sample.Subject","kind":"TYPE","qualified_name":"sample.Subject","confidence":"COMPILER_RESOLVED","provenance":"test","metadata":{},"unresolved":[]}),
                json!({"type":"edge","source":source,"target":target,"kind":"DECLARES_BEAN","confidence":"FRAMEWORK_RESOLVED","provenance":"test","metadata":{},"occurrences":[]}),
                json!({"type":"analysis_summary","summary":{"language":"java","files_discovered":1,"files_parsed":1,"files_failed":0,"bindings_resolved":1,"bindings_unresolved":0,"nodes_emitted":1,"edges_emitted":1,"duration_ms":1,"timings_ms":{},"resources":{},"configuration":{},"status":"complete"}}),
                json!({"type":"analysis_completed","status":"complete"}),
            ] {
                collector.accept(&event.to_string()).unwrap();
            }

            let Err(error) = collector.finish() else {
                panic!("dangling edge was accepted");
            };

            assert!(matches!(
                &error,
                GraphineError::AnalyzerProtocolDanglingEdge {
                    endpoint: actual_endpoint,
                    stable_id,
                    edge_kind,
                } if actual_endpoint == endpoint && stable_id == missing && edge_kind == "DECLARES_BEAN"
            ));
            assert_eq!(
                error.to_string(),
                format!(
                    "analyzer protocol failed validation: dangling DECLARES_BEAN edge is missing {endpoint} node {missing}"
                )
            );
        }
    }

    fn request() -> AnalyzeProjectRequest {
        AnalyzeProjectRequest {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            request_id: "supervision-test".to_owned(),
            operation: "analyze_project".to_owned(),
            language: ProjectLanguage::Java,
            project_root: std::env::current_dir().unwrap(),
            mode: AnalyzerMode::Safe,
            source_sets: vec!["main".to_owned()],
            options: AnalyzerOptions::default(),
            maven_executable: PathBuf::from("mvn"),
            cargo_executable: PathBuf::from("cargo"),
            rustc_executable: PathBuf::from("rustc"),
            cargo: CargoAnalysisOptions::default(),
            timeout_ms: 25,
        }
    }

    fn client_with_override(arguments: Vec<OsString>, timeout: Duration) -> AnalyzerClient {
        let mut client = AnalyzerClient::new(
            PathBuf::from("unused-java"),
            std::env::current_exe().unwrap(),
            PathBuf::from("mvn"),
            timeout,
            4096,
        );
        #[cfg(windows)]
        let program = PathBuf::from("powershell");
        #[cfg(not(windows))]
        let program = PathBuf::from("sh");
        client.command_override = Some((program, arguments));
        client
    }

    #[test]
    fn worker_timeout_is_enforced() {
        #[cfg(windows)]
        let arguments = vec![
            OsString::from("-NoProfile"),
            OsString::from("-Command"),
            OsString::from("while ($true) {}"),
        ];
        #[cfg(not(windows))]
        let arguments = vec![OsString::from("-c"), OsString::from("while :; do :; done")];
        let client = client_with_override(arguments, Duration::from_millis(25));
        assert!(matches!(
            client.run(&request(), &CancellationToken::default()),
            Err(GraphineError::AnalyzerTimeout)
        ));
    }

    #[test]
    fn worker_timeout_terminates_descendant_processes() {
        let root = std::env::temp_dir().join(format!(
            "graphine-worker-tree-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let sentinel = root.join("descendant-survived");
        #[cfg(windows)]
        let arguments = {
            let child_script = root.join("delayed-write.ps1");
            let escaped_sentinel = sentinel.to_string_lossy().replace('\'', "''");
            std::fs::write(
                &child_script,
                format!(
                    "Start-Sleep -Milliseconds 750\nSet-Content -LiteralPath '{escaped_sentinel}' -Value survived\n"
                ),
            )
            .unwrap();
            let escaped_script = child_script.to_string_lossy().replace('\'', "''");
            vec![
                OsString::from("-NoProfile"),
                OsString::from("-Command"),
                OsString::from(format!(
                    r#"$scriptPath = '{escaped_script}'; Start-Process -FilePath powershell -WindowStyle Hidden -ArgumentList @('-NoProfile','-File',('"' + $scriptPath + '"')); while ($true) {{}}"#
                )),
            ]
        };
        #[cfg(not(windows))]
        let arguments = vec![
            OsString::from("-c"),
            OsString::from(format!(
                "(sleep 0.75; touch '{}') & while :; do :; done",
                sentinel.to_string_lossy().replace('\'', "'\\''")
            )),
        ];
        let client = client_with_override(arguments, Duration::from_millis(100));
        assert!(matches!(
            client.run(&request(), &CancellationToken::default()),
            Err(GraphineError::AnalyzerTimeout)
        ));
        thread::sleep(Duration::from_secs(1));
        assert!(
            !sentinel.exists(),
            "a descendant of the timed-out analyzer survived"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_crash_is_reported_without_activating_data() {
        #[cfg(windows)]
        let arguments = vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from("$null = [Console]::In.ReadToEnd(); exit 7"),
        ];
        #[cfg(not(windows))]
        let arguments = vec![
            OsString::from("-c"),
            OsString::from("cat >/dev/null; exit 7"),
        ];
        let client = client_with_override(arguments, Duration::from_secs(5));
        assert!(matches!(
            client.run(&request(), &CancellationToken::default()),
            Err(GraphineError::AnalyzerFailed)
        ));
    }

    #[test]
    fn worker_cancellation_kills_the_process() {
        #[cfg(windows)]
        let arguments = vec![
            OsString::from("-NoProfile"),
            OsString::from("-Command"),
            OsString::from("while ($true) {}"),
        ];
        #[cfg(not(windows))]
        let arguments = vec![OsString::from("-c"), OsString::from("while :; do :; done")];
        let client = client_with_override(arguments, Duration::from_secs(5));
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        assert!(matches!(
            client.run(&request(), &cancellation),
            Err(GraphineError::AnalysisCancelled)
        ));
    }
}
