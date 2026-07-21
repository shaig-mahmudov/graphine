use graphine_index::{AnalysisIngestion, Database};
use graphine_protocol::{
    ANALYZER_PROTOCOL_VERSION, AnalysisCompletionStatus, AnalyzeProjectRequest, AnalyzerDiagnostic,
    AnalyzerEvent, AnalyzerMode, AnalyzerOptions, AnalyzerSummary, GraphineConfig, GraphineError,
    StableId, SyntheticEdge, SyntheticGraph, SyntheticNode,
};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::ffi::OsString;
use std::io::{Read, Write};
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
    timeout: Duration,
    output_limit: usize,
    #[cfg(test)]
    command_override: Option<(PathBuf, Vec<OsString>)>,
}

#[derive(Debug, Clone)]
pub struct AnalysisOutcome {
    pub generation: i64,
    pub summary: AnalyzerSummary,
    pub diagnostics: usize,
    pub partial: bool,
    pub analyzer_version: String,
    pub worker_round_trip_ms: u64,
    pub ingestion_ms: u64,
}

impl AnalyzerClient {
    #[must_use]
    pub fn from_config(config: &GraphineConfig, workspace_root: &Path) -> Self {
        let worker_jar = config.analyzer_jar.clone().unwrap_or_else(|| {
            workspace_root.join("analyzer-jdt/analyzer-cli/target/graphine-analyzer.jar")
        });
        Self {
            java_executable: config.java_executable.clone(),
            worker_jar,
            maven_executable: config.maven_executable.clone(),
            timeout: Duration::from_millis(config.analyzer_timeout_ms),
            output_limit: config.analyzer_output_limit_bytes,
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
        Self {
            java_executable,
            worker_jar,
            maven_executable,
            timeout,
            output_limit,
            #[cfg(test)]
            command_override: None,
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
        let project = database.project_by_ref(project_ref)?;
        let request_id = format!("index-{}-{}", now_ms(), std::process::id());
        let request = AnalyzeProjectRequest {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            operation: "analyze_project".to_owned(),
            project_root: java_compatible_path(&project.canonical_root),
            mode,
            source_sets: vec!["main".to_owned(), "test".to_owned()],
            options: AnalyzerOptions::default(),
            maven_executable: resolve_executable(&self.maven_executable),
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
            analyzer_version: analysis.analyzer_version.clone(),
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
            analyzer_version: analysis.analyzer_version,
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
        let mut child = self
            .command()
            .arg("--version")
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
                return String::from_utf8(bytes)
                    .map(|value| value.trim().to_owned())
                    .map_err(|_| GraphineError::AnalyzerProtocol);
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
        if !self.worker_jar.is_file() {
            return Err(GraphineError::AnalyzerUnavailable);
        }
        let mut child = self
            .command()
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
        let mut collector = EventCollector::new(request.request_id.clone());
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

    fn command(&self) -> Command {
        #[cfg(test)]
        let mut command = if let Some((program, arguments)) = &self.command_override {
            let mut override_command = Command::new(program);
            override_command.args(arguments);
            override_command
        } else {
            self.java_command()
        };
        #[cfg(not(test))]
        let mut command = self.java_command();
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
        ] {
            if let Some((key, value)) = inherited
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(allowed))
            {
                command.env(key, value);
            }
        }
        command
    }

    fn java_command(&self) -> Command {
        let mut command = Command::new(&self.java_executable);
        command.arg("-Xmx1024m").arg("-jar").arg(&self.worker_jar);
        command
    }
}

struct RawAnalysis {
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
    started: bool,
    analyzer_version: Option<String>,
    fingerprint: Option<String>,
    nodes: BTreeMap<String, SyntheticNode>,
    edges: Vec<SyntheticEdge>,
    edge_keys: BTreeSet<(String, String, String)>,
    diagnostics: Vec<AnalyzerDiagnostic>,
    summary: Option<AnalyzerSummary>,
    completion: Option<AnalysisCompletionStatus>,
}

impl EventCollector {
    fn new(request_id: String) -> Self {
        Self {
            request_id,
            started: false,
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
                analyzer_version,
            } => {
                if self.started
                    || protocol_version != ANALYZER_PROTOCOL_VERSION
                    || request_id != self.request_id
                {
                    return Err(GraphineError::AnalyzerProtocol);
                }
                self.started = true;
                self.analyzer_version = Some(analyzer_version);
            }
            AnalyzerEvent::ProjectMetadata { fingerprint, .. } if self.started => {
                self.fingerprint = Some(fingerprint);
            }
            AnalyzerEvent::ModuleDiscovered {
                root, source_roots, ..
            } if self.started => {
                if (root != "." && !safe_relative(&root))
                    || source_roots.iter().any(|path| !safe_relative(path))
                {
                    return Err(GraphineError::AnalyzerProtocol);
                }
            }
            AnalyzerEvent::Node { node } if self.started => {
                StableId::parse(node.stable_id.clone())?;
                if !node.metadata.is_object()
                    || self.nodes.insert(node.stable_id.clone(), node).is_some()
                {
                    return Err(GraphineError::AnalyzerProtocol);
                }
            }
            AnalyzerEvent::Edge {
                source,
                target,
                kind,
                confidence,
                provenance,
                metadata,
            } if self.started => {
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
                });
            }
            AnalyzerEvent::Diagnostic { diagnostic } if self.started => {
                self.diagnostics.push(diagnostic);
            }
            AnalyzerEvent::AnalysisSummary { summary } if self.started => {
                self.summary = Some(summary);
            }
            AnalyzerEvent::AnalysisCompleted { status } if self.started => {
                self.completion = Some(status);
            }
            AnalyzerEvent::AnalysisFailed { code, message } => {
                warn!(%code, %message, "analyzer reported failure");
                return Err(GraphineError::AnalyzerFailed);
            }
            _ => return Err(GraphineError::AnalyzerProtocol),
        }
        Ok(())
    }

    fn finish(self) -> Result<RawAnalysis, GraphineError> {
        let summary = self.summary.ok_or(GraphineError::AnalyzerProtocol)?;
        if summary.nodes_emitted != self.nodes.len() as u64
            || summary.edges_emitted != self.edges.len() as u64
        {
            return Err(GraphineError::AnalyzerProtocol);
        }
        if self.edges.iter().any(|edge| {
            !self.nodes.contains_key(&edge.source_stable_id)
                || !self.nodes.contains_key(&edge.target_stable_id)
        }) {
            return Err(GraphineError::AnalyzerProtocol);
        }
        Ok(RawAnalysis {
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

    #[test]
    fn protocol_rejects_malformed_version_duplicates_and_incomplete_streams() {
        let mut collector = EventCollector::new("id".to_owned());
        assert!(collector.accept("not json").is_err());
        assert!(collector.accept(&json!({"type":"analysis_started","protocol_version":2,"request_id":"id","analyzer_version":"x"}).to_string()).is_err());
        assert!(EventCollector::new("id".to_owned()).finish().is_err());
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
    fn analyzer_events_require_known_stable_ids() {
        let mut collector = EventCollector::new("id".to_owned());
        collector.accept(&json!({"type":"analysis_started","protocol_version":1,"request_id":"id","analyzer_version":"x"}).to_string()).unwrap();
        let invalid = json!({"type":"node","stable_id":"unknown:x","kind":"TYPE","qualified_name":"x","confidence":"COMPILER_RESOLVED","provenance":"test","metadata":{},"unresolved":[]});
        assert!(collector.accept(&invalid.to_string()).is_err());
    }

    #[test]
    fn successful_and_partial_streams_complete_only_with_matching_counts() {
        for (status, expected) in [
            ("complete", AnalysisCompletionStatus::Complete),
            ("partial", AnalysisCompletionStatus::Partial),
        ] {
            let mut collector = EventCollector::new("id".to_owned());
            for event in [
                json!({"type":"analysis_started","protocol_version":1,"request_id":"id","analyzer_version":"test"}),
                json!({"type":"project_metadata","fingerprint":"abc","java_release":"17","classpath_resolution_ms":0,"modules":["fixture"]}),
                json!({"type":"module_discovered","name":"fixture","root":".","source_roots":["src/main/java"]}),
                json!({"type":"node","stable_id":"type:sample.Subject","kind":"TYPE","qualified_name":"sample.Subject","file_path":"src/main/java/sample/Subject.java","start_line":1,"end_line":1,"confidence":"COMPILER_RESOLVED","provenance":"test","metadata":{},"unresolved":[]}),
                json!({"type":"analysis_summary","summary":{"files_discovered":1,"files_parsed":1,"files_failed":0,"bindings_resolved":1,"bindings_unresolved":0,"nodes_emitted":1,"edges_emitted":0,"duration_ms":1,"classpath_resolution_ms":0,"parsing_ms":1,"serialization_ms":0,"peak_java_memory_bytes":1,"status":status}}),
                json!({"type":"analysis_completed","status":status}),
            ] {
                collector.accept(&event.to_string()).unwrap();
            }
            assert_eq!(collector.finish().unwrap().completion, expected);
        }
    }

    fn request() -> AnalyzeProjectRequest {
        AnalyzeProjectRequest {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            request_id: "supervision-test".to_owned(),
            operation: "analyze_project".to_owned(),
            project_root: std::env::current_dir().unwrap(),
            mode: AnalyzerMode::Safe,
            source_sets: vec!["main".to_owned()],
            options: AnalyzerOptions::default(),
            maven_executable: PathBuf::from("mvn"),
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
    fn worker_crash_is_reported_without_activating_data() {
        #[cfg(windows)]
        let arguments = vec![
            OsString::from("-NoProfile"),
            OsString::from("-Command"),
            OsString::from("$input | Out-Null; exit 7"),
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
