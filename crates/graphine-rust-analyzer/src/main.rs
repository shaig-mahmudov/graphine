mod analysis;

use anyhow::{Context, Result, bail};
use graphine_protocol::{
    ANALYZER_CAPABILITY_CARGO_SAFE_MODE, ANALYZER_CAPABILITY_CARGO_TRUSTED_MODE,
    ANALYZER_CAPABILITY_RUST_SEMANTICS, ANALYZER_PROTOCOL_VERSION, AnalysisCompletionStatus,
    AnalyzeProjectRequest, AnalyzerEvent, AnalyzerHello, ProjectLanguage,
};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

const ANALYZER_NAME: &str = "graphine-rust-analyzer";
const ANALYZER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_REQUEST_BYTES: u64 = 8 * 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        let _ = emit(&AnalyzerEvent::AnalysisFailed {
            code: "analysis_failed".to_owned(),
            message: format!("{error:#}"),
        });
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--version") => {
            println!("{ANALYZER_VERSION}");
            return Ok(());
        }
        Some("--metadata") => {
            println!("{}", serde_json::to_string(&hello())?);
            return Ok(());
        }
        Some(argument) => bail!("unsupported argument: {argument}"),
        None => {}
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock()).take(MAX_REQUEST_BYTES + 1);
    let mut request_json = String::new();
    reader
        .read_line(&mut request_json)
        .context("failed to read analyzer request")?;
    if request_json.len() as u64 > MAX_REQUEST_BYTES {
        bail!("analyzer request exceeds {MAX_REQUEST_BYTES} bytes");
    }
    if request_json.trim().is_empty() {
        bail!("analyzer request is empty");
    }

    let request: AnalyzeProjectRequest =
        serde_json::from_str(&request_json).context("invalid analyzer request JSON")?;
    validate_request(&request)?;

    emit(&AnalyzerEvent::AnalysisStarted {
        protocol_version: ANALYZER_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        analyzer_name: ANALYZER_NAME.to_owned(),
        language: ProjectLanguage::Rust,
        analyzer_version: ANALYZER_VERSION.to_owned(),
    })?;

    let output = analysis::analyze(&request, ANALYZER_VERSION)?;
    emit(&AnalyzerEvent::ProjectMetadata {
        language: ProjectLanguage::Rust,
        fingerprint: output.fingerprint,
        modules: output
            .modules
            .iter()
            .map(|module| module.name.clone())
            .collect(),
        configuration: serde_json::json!({
            "cargo": request.cargo,
            "mode": request.mode,
        }),
    })?;
    for module in output.modules {
        emit(&AnalyzerEvent::ModuleDiscovered {
            name: module.name,
            root: module.root,
            source_roots: module.source_roots,
        })?;
    }
    for node in output.nodes {
        emit(&AnalyzerEvent::Node { node })?;
    }
    for edge in output.edges {
        emit(&AnalyzerEvent::Edge {
            source: edge.source_stable_id,
            target: edge.target_stable_id,
            kind: edge.kind,
            confidence: edge.confidence,
            provenance: edge.provenance,
            metadata: edge.metadata,
            occurrences: edge.occurrences,
        })?;
    }
    for diagnostic in output.diagnostics {
        emit(&AnalyzerEvent::Diagnostic { diagnostic })?;
    }
    emit(&AnalyzerEvent::AnalysisSummary {
        summary: output.summary,
    })?;
    emit(&AnalyzerEvent::AnalysisCompleted {
        status: if output.partial {
            AnalysisCompletionStatus::Partial
        } else {
            AnalysisCompletionStatus::Complete
        },
    })?;
    Ok(())
}

fn validate_request(request: &AnalyzeProjectRequest) -> Result<()> {
    if request.protocol_version != ANALYZER_PROTOCOL_VERSION {
        bail!(
            "unsupported analyzer protocol {}; expected {}",
            request.protocol_version,
            ANALYZER_PROTOCOL_VERSION
        );
    }
    if request.language != ProjectLanguage::Rust {
        bail!(
            "wrong-language request for Rust worker: {}",
            request.language
        );
    }
    if request.operation != "analyze_project" {
        bail!("unsupported operation: {}", request.operation);
    }
    if request.cargo.all_features
        && (request.cargo.no_default_features || !request.cargo.features.is_empty())
    {
        bail!("--all-features conflicts with --features and --no-default-features");
    }
    if !request.project_root.join("Cargo.toml").is_file() {
        bail!(
            "Rust project root has no Cargo.toml: {}",
            request.project_root.display()
        );
    }
    Ok(())
}

fn hello() -> AnalyzerHello {
    AnalyzerHello {
        protocol_version: ANALYZER_PROTOCOL_VERSION,
        analyzer_name: ANALYZER_NAME.to_owned(),
        analyzer_version: ANALYZER_VERSION.to_owned(),
        language: ProjectLanguage::Rust,
        capabilities: vec![
            ANALYZER_CAPABILITY_RUST_SEMANTICS.to_owned(),
            ANALYZER_CAPABILITY_CARGO_SAFE_MODE.to_owned(),
            ANALYZER_CAPABILITY_CARGO_TRUSTED_MODE.to_owned(),
        ],
    }
}

fn emit(event: &AnalyzerEvent) -> Result<()> {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    serde_json::to_writer(&mut writer, event)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphine_protocol::{AnalyzerMode, AnalyzerOptions, CargoAnalysisOptions};
    use std::path::PathBuf;

    #[test]
    fn wrong_language_and_conflicting_features_are_rejected() {
        let mut request = AnalyzeProjectRequest {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            request_id: "validation".to_owned(),
            operation: "analyze_project".to_owned(),
            language: ProjectLanguage::Java,
            project_root: PathBuf::from("."),
            mode: AnalyzerMode::Safe,
            source_sets: Vec::new(),
            options: AnalyzerOptions::default(),
            maven_executable: PathBuf::from("mvn"),
            cargo_executable: PathBuf::from("cargo"),
            rustc_executable: PathBuf::from("rustc"),
            cargo: CargoAnalysisOptions::default(),
            timeout_ms: 1,
        };
        assert!(validate_request(&request).is_err());
        request.language = ProjectLanguage::Rust;
        request.project_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rust-core");
        request.cargo.all_features = true;
        request.cargo.no_default_features = true;
        assert!(validate_request(&request).is_err());
    }
}
