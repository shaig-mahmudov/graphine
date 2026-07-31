use anyhow::{Context, Result, bail};
use cargo_metadata::{CargoOpt, MetadataCommand, PackageId};
use graphine_protocol::{
    AnalyzeProjectRequest, AnalyzerDiagnostic, AnalyzerMode, AnalyzerSummary, Confidence,
    EdgeOccurrence, ProjectLanguage, SyntheticEdge, SyntheticNode,
};
use ra_ap_cfg::CfgExpr;
use ra_ap_hir::{
    Adt, AsAssocItem, AssocItemContainer, Function, Module, ModuleDef, PathResolution, Semantics,
    Trait, Variant,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_proc_macro_api::ProcMacroClient;
use ra_ap_project_model::{CargoConfig, CargoFeatures, TargetDirectoryConfig};
use ra_ap_syntax::{
    AstNode, Edition, SourceFile,
    ast::{self, HasAttrs, HasName},
};
use ra_ap_vfs::Vfs;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use walkdir::{DirEntry, WalkDir};

pub(crate) struct AnalysisOutput {
    pub fingerprint: String,
    pub modules: Vec<ModuleInfo>,
    pub nodes: Vec<SyntheticNode>,
    pub edges: Vec<SyntheticEdge>,
    pub diagnostics: Vec<AnalyzerDiagnostic>,
    pub summary: AnalyzerSummary,
    pub partial: bool,
}

pub(crate) struct ModuleInfo {
    pub name: String,
    pub root: String,
    pub source_roots: Vec<String>,
}

#[derive(Clone, Debug)]
struct TargetInfo {
    package: String,
    kind: String,
    name: String,
    source: PathBuf,
    crate_key: String,
}

#[derive(Clone)]
struct SourceUnit {
    target: TargetInfo,
    hir_module: Option<Module>,
    path: PathBuf,
    relative_path: String,
    module_path: String,
    text: String,
}

#[derive(Clone)]
struct Definition {
    stable_id: String,
    simple_name: String,
}

#[derive(Default)]
struct GraphBuilder {
    nodes: BTreeMap<String, SyntheticNode>,
    edges: BTreeMap<(String, String, String), SyntheticEdge>,
    definitions: Vec<Definition>,
    names: HashMap<String, Vec<String>>,
    diagnostics: Vec<AnalyzerDiagnostic>,
    bindings_resolved: u64,
    bindings_unresolved: u64,
}

struct SemanticWorkspace {
    database: RootDatabase,
    vfs: Vfs,
    proc_macro_client: Option<ProcMacroClient>,
    crate_targets: HashMap<ra_ap_hir::Crate, TargetInfo>,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn analyze(
    request: &AnalyzeProjectRequest,
    analyzer_version: &str,
) -> Result<AnalysisOutput> {
    let started = Instant::now();
    let root = request
        .project_root
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", request.project_root.display()))?;

    let mut diagnostics = Vec::new();
    let safe_manifest_fallback = request.mode == AnalyzerMode::Safe
        && !root.join("Cargo.lock").is_file()
        && has_non_path_dependencies(&root)?;
    let (targets, metadata_complete) = if safe_manifest_fallback {
        diagnostics.push(project_diagnostic(
            "cargo_metadata_unavailable",
            "safe mode found registry dependencies without a lockfile; using a local manifest/path-dependency model",
        ));
        (manifest_targets(&root)?, false)
    } else {
        match cargo_targets(&root, request) {
            Ok(targets) if !targets.is_empty() => (targets, true),
            Ok(_) => {
                diagnostics.push(project_diagnostic(
                    "cargo_metadata_unavailable",
                    "Cargo reported no workspace targets; using manifest-only discovery",
                ));
                (manifest_targets(&root)?, false)
            }
            Err(error) => {
                diagnostics.push(project_diagnostic(
                    "cargo_metadata_unavailable",
                    &format!(
                        "Cargo metadata was unavailable; using a local manifest-only model: {error:#}"
                    ),
                ));
                (manifest_targets(&root)?, false)
            }
        }
    };
    if targets.is_empty() {
        bail!("Cargo workspace contains no discoverable Rust targets");
    }

    if request.mode == AnalyzerMode::Safe
        && targets.iter().any(|target| target.kind == "custom-build")
    {
        diagnostics.push(project_diagnostic(
            "build_script_disabled",
            "safe mode indexed build-script source without executing it or consuming generated output",
        ));
    }
    if request.mode == AnalyzerMode::Safe
        && targets.iter().any(|target| target.kind == "proc-macro")
    {
        diagnostics.push(project_diagnostic(
            "procedural_macro_disabled",
            "safe mode indexed procedural-macro source without executing macro expansion",
        ));
    }
    let trusted_check_started = Instant::now();
    let trusted_build_data = if request.mode == AnalyzerMode::Trusted {
        match run_trusted_cargo_check(&root, request) {
            Ok(()) => true,
            Err(error) => {
                diagnostics.push(project_diagnostic(
                    "trusted_cargo_check_failed",
                    &format!(
                        "trusted Cargo check did not complete; semantic recovery will continue without build data or procedural macros: {error:#}"
                    ),
                ));
                false
            }
        }
    } else {
        false
    };
    let trusted_check_ms = if request.mode == AnalyzerMode::Trusted {
        elapsed_ms(trusted_check_started)
    } else {
        0
    };
    let load_started = Instant::now();
    let semantics = if safe_manifest_fallback {
        diagnostics.push(project_diagnostic(
            "semantic_workspace_unavailable",
            "safe manifest-only fallback cannot load unavailable registry dependencies",
        ));
        None
    } else {
        match load_semantics(&root, request, &targets, trusted_build_data) {
            Ok(workspace) => Some(workspace),
            Err(error) => {
                diagnostics.push(project_diagnostic(
                    "semantic_workspace_unavailable",
                    &format!(
                        "rust-analyzer semantic loading was incomplete; syntax evidence is retained: {error:#}"
                    ),
                ));
                None
            }
        }
    };
    let semantic_load_ms = elapsed_ms(load_started);
    let units = if let Some(workspace) = semantics.as_ref() {
        discover_semantic_sources(&root, &targets, workspace, request.options.include_tests)?
    } else {
        discover_sources(&root, &targets, request.options.include_tests)?
    };
    let cfg_audit = audit_cfg_items(&units, semantics.as_ref());
    if cfg_audit.excluded {
        diagnostics.push(project_diagnostic(
            "cfg_branch_excluded",
            "one or more declarations were excluded by rust-analyzer's active Cargo configuration",
        ));
    }
    diagnostics.extend(cfg_audit.diagnostics);
    if trusted_build_data
        && semantics
            .as_ref()
            .is_some_and(|workspace| workspace.proc_macro_client.is_none())
    {
        diagnostics.push(project_diagnostic(
            "procedural_macro_server_unavailable",
            "trusted Cargo check executed procedural macros, but rust-analyzer could not retain a procedural-macro server for semantic expansion",
        ));
    }
    let trusted_proc_macro_expansions =
        if trusted_build_data && let Some(workspace) = semantics.as_ref() {
            ra_ap_hir::attach_db(&workspace.database, || {
                expand_trusted_attribute_macros(&units, workspace)
            })
        } else {
            0
        };
    let parsing_started = Instant::now();

    let mut graph = GraphBuilder::default();
    emit_crates_and_modules(&mut graph, &targets, &units);
    if let Some(workspace) = semantics.as_ref() {
        ra_ap_hir::attach_db(&workspace.database, || {
            for unit in &units {
                collect_type_definitions(&mut graph, unit, semantics.as_ref(), request);
            }
            for unit in &units {
                collect_callable_definitions(&mut graph, unit, semantics.as_ref(), request);
            }
            for unit in &units {
                collect_relationships(&mut graph, unit, semantics.as_ref(), request);
            }
        });
    } else {
        for unit in &units {
            collect_type_definitions(&mut graph, unit, None, request);
        }
        for unit in &units {
            collect_callable_definitions(&mut graph, unit, None, request);
        }
        for unit in &units {
            collect_relationships(&mut graph, unit, None, request);
        }
    }
    graph.diagnostics.extend(diagnostics);

    let parsing_ms = elapsed_ms(parsing_started);
    let fingerprint = fingerprint(&root, &units, request, analyzer_version, metadata_complete)?;
    let modules = targets
        .iter()
        .map(|target| ModuleInfo {
            name: target.crate_key.clone(),
            root: relative_slash(&root, &target.source),
            source_roots: vec![relative_slash(
                &root,
                target.source.parent().unwrap_or(&target.source),
            )],
        })
        .collect::<Vec<_>>();
    let partial = !metadata_complete
        || semantics.is_none()
        || graph
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == "error");
    let mut capabilities = BTreeMap::new();
    capabilities.insert("rust_semantics".to_owned(), semantics.is_some());
    capabilities.insert(
        "cargo_safe_mode".to_owned(),
        request.mode == AnalyzerMode::Safe,
    );
    capabilities.insert(
        "cargo_trusted_mode".to_owned(),
        request.mode == AnalyzerMode::Trusted,
    );
    let mut timings_ms = BTreeMap::new();
    timings_ms.insert("project_loading".to_owned(), semantic_load_ms);
    timings_ms.insert("trusted_cargo_check".to_owned(), trusted_check_ms);
    timings_ms.insert("parsing_and_extraction".to_owned(), parsing_ms);
    let total_ms = elapsed_ms(started);
    timings_ms.insert("total".to_owned(), total_ms);
    let node_count = graph.nodes.len() as u64;
    let edge_count = graph.edges.len() as u64;
    let diagnostic_count = graph.diagnostics.len() as u64;
    let summary = AnalyzerSummary {
        language: ProjectLanguage::Rust,
        files_discovered: units.len() as u64,
        files_parsed: units.len() as u64,
        files_failed: 0,
        bindings_resolved: graph.bindings_resolved,
        bindings_unresolved: graph.bindings_unresolved,
        nodes_emitted: node_count,
        edges_emitted: edge_count,
        duration_ms: total_ms,
        capabilities,
        timings_ms,
        resources: BTreeMap::from([
            (
                "source_bytes".to_owned(),
                units.iter().map(|unit| unit.text.len() as u64).sum(),
            ),
            ("diagnostics".to_owned(), diagnostic_count),
            (
                "trusted_proc_macro_expansions".to_owned(),
                trusted_proc_macro_expansions,
            ),
        ]),
        configuration: json!({
            "mode":request.mode,
            "features":request.cargo.features,
            "all_features":request.cargo.all_features,
            "no_default_features":request.cargo.no_default_features,
            "target":request.cargo.target,
        }),
        status: if partial { "partial" } else { "complete" }.to_owned(),
    };

    Ok(AnalysisOutput {
        fingerprint,
        modules,
        nodes: graph.nodes.into_values().collect(),
        edges: graph.edges.into_values().collect(),
        diagnostics: graph.diagnostics,
        summary,
        partial,
    })
}

fn has_non_path_dependencies(root: &Path) -> Result<bool> {
    for manifest in WalkDir::new(root)
        .into_iter()
        .filter_entry(include_entry)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "Cargo.toml")
    {
        let text = fs::read_to_string(manifest.path())
            .with_context(|| format!("cannot read {}", manifest.path().display()))?;
        let value: toml::Value = toml::from_str(&text)
            .with_context(|| format!("cannot parse {}", manifest.path().display()))?;
        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if value
                .get(section)
                .and_then(toml::Value::as_table)
                .is_some_and(|dependencies| {
                    dependencies.values().any(|dependency| match dependency {
                        toml::Value::String(_) => true,
                        toml::Value::Table(details) => !details.contains_key("path"),
                        _ => false,
                    })
                })
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn run_trusted_cargo_check(root: &Path, request: &AnalyzeProjectRequest) -> Result<()> {
    let target_dir = trusted_target_directory(root);
    let mut command = Command::new(&request.cargo_executable);
    command
        .current_dir(root)
        .arg("check")
        .arg("--all-targets")
        .arg("--target-dir")
        .arg(target_dir)
        .env("RUSTC", &request.rustc_executable)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if request.cargo.all_features {
        command.arg("--all-features");
    } else {
        if request.cargo.no_default_features {
            command.arg("--no-default-features");
        }
        if !request.cargo.features.is_empty() {
            command
                .arg("--features")
                .arg(request.cargo.features.join(","));
        }
    }
    if let Some(target) = &request.cargo.target {
        command.arg("--target").arg(target);
    }
    let mut child = command
        .spawn()
        .context("failed to start trusted Cargo check")?;
    let timeout = trusted_cargo_timeout(request);
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect trusted Cargo check")?
        {
            if !status.success() {
                bail!("trusted Cargo check exited with {status}");
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            terminate_descendant_tree(&mut child);
            bail!(
                "trusted Cargo check exceeded its dedicated {} ms timeout",
                timeout.as_millis()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn trusted_cargo_timeout(request: &AnalyzeProjectRequest) -> Duration {
    Duration::from_millis((request.timeout_ms / 2).clamp(100, 120_000))
}

#[cfg(windows)]
fn terminate_descendant_tree(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_descendant_tree(child: &mut Child) {
    let root = child.id();
    let descendants = Command::new("ps")
        .args(["-eo", "pid=,ppid="])
        .output()
        .ok()
        .map(|output| descendant_processes(root, &String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    let _ = child.kill();
    if !descendants.is_empty() {
        let mut command = Command::new("kill");
        command.arg("-KILL").arg("--");
        for process_id in descendants.into_iter().rev() {
            command.arg(process_id.to_string());
        }
        let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.wait();
}

#[cfg(unix)]
fn descendant_processes(root: u32, process_table: &str) -> Vec<u32> {
    let mut children = HashMap::<u32, Vec<u32>>::new();
    for line in process_table.lines() {
        let mut fields = line.split_whitespace();
        let (Some(process_id), Some(parent_id)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let (Ok(process_id), Ok(parent_id)) = (process_id.parse(), parent_id.parse()) {
            children.entry(parent_id).or_default().push(process_id);
        }
    }
    let mut descendants = Vec::new();
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        if let Some(processes) = children.get(&parent) {
            descendants.extend(processes);
            pending.extend(processes);
        }
    }
    descendants
}

fn cargo_targets(root: &Path, request: &AnalyzeProjectRequest) -> Result<Vec<TargetInfo>> {
    let mut command = MetadataCommand::new();
    command
        .cargo_path(&request.cargo_executable)
        .manifest_path(root.join("Cargo.toml"))
        .current_dir(root)
        .no_deps();
    if request.cargo.all_features {
        command.features(CargoOpt::AllFeatures);
    } else {
        if request.cargo.no_default_features {
            command.features(CargoOpt::NoDefaultFeatures);
        }
        if !request.cargo.features.is_empty() {
            command.features(CargoOpt::SomeFeatures(request.cargo.features.clone()));
        }
    }
    if request.mode == AnalyzerMode::Safe {
        command.other_options(vec!["--offline".to_owned(), "--locked".to_owned()]);
        command.env("CARGO_NET_OFFLINE", "true");
    }
    command.env("RUSTC", request.rustc_executable.as_os_str());
    let metadata = command.exec().context("cargo metadata failed")?;
    let members = metadata
        .workspace_members
        .iter()
        .cloned()
        .collect::<BTreeSet<PackageId>>();
    let mut targets = Vec::new();
    for package in metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
    {
        for target in &package.targets {
            let kind = target
                .kind
                .first()
                .map_or_else(|| "unknown".to_owned(), ToString::to_string);
            let source = target.src_path.clone().into_std_path_buf();
            targets.push(TargetInfo {
                crate_key: crate_key(package.name.as_ref(), &kind, &target.name),
                package: package.name.to_string(),
                kind,
                name: target.name.clone(),
                source,
            });
        }
    }
    targets.sort_by(|left, right| left.crate_key.cmp(&right.crate_key));
    Ok(targets)
}

fn manifest_targets(root: &Path) -> Result<Vec<TargetInfo>> {
    let manifests = WalkDir::new(root)
        .into_iter()
        .filter_entry(include_entry)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "Cargo.toml")
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    let mut targets = Vec::new();
    for manifest in manifests {
        let text = fs::read_to_string(&manifest)
            .with_context(|| format!("cannot read {}", manifest.display()))?;
        let value: toml::Value = toml::from_str(&text)
            .with_context(|| format!("cannot parse {}", manifest.display()))?;
        let Some(package) = value.get("package") else {
            continue;
        };
        let Some(package_name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let package_root = manifest.parent().unwrap_or(root);
        add_standard_target(
            &mut targets,
            package_name,
            "lib",
            package_name,
            package_root.join("src/lib.rs"),
        );
        add_standard_target(
            &mut targets,
            package_name,
            "bin",
            package_name,
            package_root.join("src/main.rs"),
        );
        add_standard_target(
            &mut targets,
            package_name,
            "custom-build",
            "build-script-build",
            package_root.join("build.rs"),
        );
        for (directory, kind) in [
            ("src/bin", "bin"),
            ("tests", "test"),
            ("examples", "example"),
            ("benches", "bench"),
        ] {
            let path = package_root.join(directory);
            let Ok(entries) = fs::read_dir(path) else {
                continue;
            };
            for entry in entries.flatten() {
                let source = entry.path();
                if source
                    .extension()
                    .is_some_and(|extension| extension == "rs")
                {
                    let name = source
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or(kind)
                        .to_owned();
                    add_standard_target(&mut targets, package_name, kind, &name, source);
                }
            }
        }
    }
    targets.sort_by(|left, right| left.crate_key.cmp(&right.crate_key));
    targets.dedup_by(|left, right| left.crate_key == right.crate_key);
    Ok(targets)
}

fn add_standard_target(
    targets: &mut Vec<TargetInfo>,
    package: &str,
    kind: &str,
    name: &str,
    source: PathBuf,
) {
    if source.is_file() {
        targets.push(TargetInfo {
            package: package.to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            source,
            crate_key: crate_key(package, kind, name),
        });
    }
}

fn discover_sources(
    root: &Path,
    targets: &[TargetInfo],
    include_tests: bool,
) -> Result<Vec<SourceUnit>> {
    let rust_files = WalkDir::new(root)
        .into_iter()
        .filter_entry(include_entry)
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rs")
        })
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    let mut units = Vec::new();
    for path in rust_files {
        let Some(target) = select_target(&path, targets, include_tests) else {
            continue;
        };
        let text = fs::read_to_string(&path)
            .with_context(|| format!("cannot read Rust source {}", path.display()))?;
        units.push(SourceUnit {
            relative_path: relative_slash(root, &path),
            module_path: module_path(target, &path),
            target: target.clone(),
            hir_module: None,
            path,
            text,
        });
    }
    units.sort_by(|left, right| {
        (&left.target.crate_key, &left.relative_path)
            .cmp(&(&right.target.crate_key, &right.relative_path))
    });
    units.dedup_by(|left, right| left.path == right.path);
    Ok(units)
}

fn discover_semantic_sources(
    root: &Path,
    targets: &[TargetInfo],
    workspace: &SemanticWorkspace,
    include_tests: bool,
) -> Result<Vec<SourceUnit>> {
    let sema = Semantics::new(&workspace.database);
    let mut units = Vec::new();
    for (krate, target) in &workspace.crate_targets {
        if !include_tests && matches!(target.kind.as_str(), "test" | "bench") {
            continue;
        }
        for module in krate.modules(&workspace.database) {
            let definition = sema.module_definition_node(module);
            if SourceFile::cast(definition.value.clone()).is_none() {
                continue;
            }
            let file_id = sema
                .original_range(&definition.value)
                .into_file_id(&workspace.database)
                .file_id;
            let Some(absolute_path) = workspace.vfs.file_path(file_id).as_path() else {
                continue;
            };
            let path: &Path = absolute_path.as_ref();
            if !path_is_within(path, root)
                || path.extension().is_none_or(|extension| extension != "rs")
                || !path.is_file()
            {
                continue;
            }
            let text = fs::read_to_string(path)
                .with_context(|| format!("cannot read Rust source {}", path.display()))?;
            units.push(SourceUnit {
                target: target.clone(),
                hir_module: Some(module),
                path: path.to_path_buf(),
                relative_path: relative_slash(root, path),
                module_path: hir_module_path(module, &workspace.database),
                text: text.clone(),
            });
        }
    }
    units.sort_by(|left, right| {
        (
            &left.target.crate_key,
            &left.module_path,
            &left.relative_path,
        )
            .cmp(&(
                &right.target.crate_key,
                &right.module_path,
                &right.relative_path,
            ))
    });
    units.dedup_by(|left, right| {
        left.target.crate_key == right.target.crate_key
            && left.module_path == right.module_path
            && left.path == right.path
    });
    if units.is_empty() {
        return discover_sources(root, targets, include_tests);
    }
    Ok(units)
}

fn select_target<'a>(
    path: &Path,
    targets: &'a [TargetInfo],
    include_tests: bool,
) -> Option<&'a TargetInfo> {
    let eligible =
        |target: &&TargetInfo| include_tests || !matches!(target.kind.as_str(), "test" | "bench");
    if let Some(exact) = targets
        .iter()
        .filter(eligible)
        .find(|target| same_path(&target.source, path))
    {
        return Some(exact);
    }
    targets
        .iter()
        .filter(eligible)
        .filter(|target| {
            target
                .source
                .parent()
                .is_some_and(|directory| path.starts_with(directory))
        })
        .max_by_key(|target| {
            let depth = target
                .source
                .parent()
                .map_or(0, |path| path.components().count());
            let primary = usize::from(matches!(
                target.kind.as_str(),
                "lib" | "rlib" | "proc-macro"
            ));
            (depth, primary)
        })
}

fn load_semantics(
    root: &Path,
    request: &AnalyzeProjectRequest,
    targets: &[TargetInfo],
    trusted_build_data: bool,
) -> Result<SemanticWorkspace> {
    let trusted_target_dir = (request.mode == AnalyzerMode::Trusted)
        .then(|| {
            camino::Utf8PathBuf::from_path_buf(trusted_target_directory(root))
                .map_err(|_| anyhow::anyhow!("Graphine target directory is not valid UTF-8"))
        })
        .transpose()?;
    let mut cargo = CargoConfig {
        all_targets: true,
        target: request.cargo.target.clone(),
        set_test: request.options.include_tests,
        no_deps: false,
        target_dir_config: trusted_target_dir.map_or(
            TargetDirectoryConfig::None,
            TargetDirectoryConfig::Directory,
        ),
        ..CargoConfig::default()
    };
    cargo.features = if request.cargo.all_features {
        CargoFeatures::All
    } else {
        CargoFeatures::Selected {
            features: request.cargo.features.clone(),
            no_default_features: request.cargo.no_default_features,
        }
    };
    if request.mode == AnalyzerMode::Safe {
        cargo.metadata_extra_args.push("--offline".to_owned());
        cargo.metadata_extra_args.push("--locked".to_owned());
        cargo
            .extra_env
            .insert("CARGO_NET_OFFLINE".to_owned(), Some("true".to_owned()));
    }
    cargo.extra_env.insert(
        "RUSTC".to_owned(),
        Some(request.rustc_executable.to_string_lossy().into_owned()),
    );
    let load = LoadCargoConfig {
        load_out_dirs_from_check: trusted_build_data,
        with_proc_macro_server: if trusted_build_data {
            ProcMacroServerChoice::Sysroot
        } else {
            ProcMacroServerChoice::None
        },
        prefill_caches: false,
        num_worker_threads: 1,
        proc_macro_processes: 1,
    };
    let (database, vfs, proc_macro_client) = load_workspace_at(root, &cargo, &load, &|_| {})?;
    let crate_targets = ra_ap_hir::Crate::all(&database)
        .into_iter()
        .filter_map(|krate| {
            target_for_hir_crate(&database, &vfs, krate, targets).map(|target| (krate, target))
        })
        .collect();
    Ok(SemanticWorkspace {
        database,
        vfs,
        proc_macro_client,
        crate_targets,
    })
}

fn target_for_hir_crate(
    database: &RootDatabase,
    vfs: &Vfs,
    krate: ra_ap_hir::Crate,
    targets: &[TargetInfo],
) -> Option<TargetInfo> {
    let root = vfs.file_path(krate.root_file(database)).as_path()?.as_ref();
    let display_name = krate
        .display_name(database)
        .map(|name| name.canonical_name().to_string());
    let candidates = targets
        .iter()
        .filter(|target| same_path(&target.source, root))
        .collect::<Vec<_>>();
    candidates
        .iter()
        .copied()
        .find(|target| {
            display_name.as_ref().is_some_and(|name| {
                target.name == *name || target.name.replace('-', "_") == name.replace('-', "_")
            })
        })
        .or_else(|| {
            if candidates.len() == 1 {
                candidates.first().copied()
            } else {
                None
            }
        })
        .cloned()
}

fn expand_trusted_attribute_macros(units: &[SourceUnit], workspace: &SemanticWorkspace) -> u64 {
    let sema = Semantics::new(&workspace.database);
    let mut expanded = 0_u64;
    for unit in units {
        let Some(file) = semantic_source_file(unit, &sema) else {
            continue;
        };
        for item in file.syntax().descendants().filter_map(ast::Item::cast) {
            if sema.expand_attr_macro(&item).is_some() {
                expanded += 1;
            }
        }
    }
    expanded
}

#[allow(clippy::too_many_lines)]
fn collect_type_definitions(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
    _request: &AnalyzeProjectRequest,
) {
    let sema = semantics.map(|workspace| Semantics::new(&workspace.database));
    let file = if let Some(sema) = sema.as_ref() {
        semantic_source_file(unit, sema)
            .unwrap_or_else(|| SourceFile::parse(&unit.text, Edition::CURRENT).tree())
    } else {
        SourceFile::parse(&unit.text, Edition::CURRENT).tree()
    };
    let root_module = module_id(&unit.target.crate_key, &unit.module_path);
    for strukt in file
        .syntax()
        .descendants()
        .filter_map(ast::Struct::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let exact_id = sema
            .as_ref()
            .and_then(|sema| sema.to_def(&strukt))
            .and_then(|definition| {
                semantics.and_then(|workspace| stable_for_hir_adt(workspace, definition.into()))
            })
            .filter(|id| stable_id_belongs_to_unit(id, unit));
        add_named_type(graph, unit, &strukt, "type", &root_module, exact_id);
        if let Some(fields) = strukt.field_list() {
            collect_fields(graph, unit, &strukt, &fields, semantics, sema.as_ref());
        }
    }
    for union in file
        .syntax()
        .descendants()
        .filter_map(ast::Union::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let exact_id = sema
            .as_ref()
            .and_then(|sema| sema.to_def(&union))
            .and_then(|definition| {
                semantics.and_then(|workspace| stable_for_hir_adt(workspace, definition.into()))
            })
            .filter(|id| stable_id_belongs_to_unit(id, unit));
        add_named_type(graph, unit, &union, "type", &root_module, exact_id);
        if let Some(fields) = union.record_field_list() {
            for field in fields.fields() {
                add_field(graph, unit, &union, &field, semantics, sema.as_ref());
            }
        }
    }
    for enumeration in file
        .syntax()
        .descendants()
        .filter_map(ast::Enum::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let exact_id = sema
            .as_ref()
            .and_then(|sema| sema.to_def(&enumeration))
            .and_then(|definition| {
                semantics.and_then(|workspace| stable_for_hir_adt(workspace, definition.into()))
            })
            .filter(|id| stable_id_belongs_to_unit(id, unit));
        add_named_type(graph, unit, &enumeration, "type", &root_module, exact_id);
        let Some(enum_name) = enumeration.name().map(|name| name.text().to_string()) else {
            continue;
        };
        let namespace = namespace_for(unit, enumeration.syntax());
        let owner = stable_for("type", unit, &namespace, &enum_name);
        if let Some(variants) = enumeration.variant_list() {
            for variant in variants.variants() {
                let Some(name) = variant.name().map(|name| name.text().to_string()) else {
                    continue;
                };
                let stable_id = format!(
                    "variant:{}::{}",
                    owner.strip_prefix("type:").unwrap_or(&owner),
                    name
                );
                add_definition_node(
                    graph,
                    unit,
                    variant.syntax(),
                    stable_id.clone(),
                    "variant",
                    &name,
                    &namespace,
                    sema.as_ref()
                        .and_then(|sema| sema.to_def(&variant))
                        .and_then(|definition| {
                            semantics.and_then(|workspace| {
                                stable_for_hir_module_def(
                                    workspace,
                                    ModuleDef::EnumVariant(definition),
                                )
                                .map(|(id, _)| id == stable_id)
                            })
                        })
                        .unwrap_or(false),
                    json!({"language":"rust","owner":owner}),
                );
                graph.add_edge(
                    &owner,
                    &stable_id,
                    "DECLARES",
                    Confidence::StaticInferred,
                    unit,
                    variant.syntax(),
                    Value::Null,
                );
            }
        }
    }
    for trait_node in file
        .syntax()
        .descendants()
        .filter_map(ast::Trait::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let exact_id = sema
            .as_ref()
            .and_then(|sema| sema.to_def(&trait_node))
            .and_then(|definition| {
                semantics.and_then(|workspace| stable_for_hir_trait(workspace, definition))
            })
            .filter(|id| stable_id_belongs_to_unit(id, unit));
        add_named_type(graph, unit, &trait_node, "trait", &root_module, exact_id);
    }
    for alias in file
        .syntax()
        .descendants()
        .filter_map(ast::TypeAlias::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        if alias
            .syntax()
            .ancestors()
            .skip(1)
            .any(|ancestor| ast::Impl::cast(ancestor).is_some())
        {
            continue;
        }
        if let Some(trait_node) = alias
            .syntax()
            .ancestors()
            .skip(1)
            .find_map(ast::Trait::cast)
        {
            let (Some(name), Some(trait_name)) = (
                alias.name().map(|name| name.text().to_string()),
                trait_node.name().map(|name| name.text().to_string()),
            ) else {
                continue;
            };
            let namespace = namespace_for(unit, trait_node.syntax());
            let trait_id = stable_for("trait", unit, &namespace, &trait_name);
            let stable_id = format!(
                "associated_type:{}::{name}",
                trait_id.strip_prefix("trait:").unwrap_or(&trait_id)
            );
            add_definition_node(
                graph,
                unit,
                alias.syntax(),
                stable_id.clone(),
                "associated_type",
                &name,
                &namespace,
                false,
                json!({"language":"rust","owner":trait_id}),
            );
            graph.add_edge(
                &trait_id,
                &stable_id,
                "DECLARES",
                Confidence::StaticInferred,
                unit,
                alias.syntax(),
                Value::Null,
            );
        } else {
            add_named_type(graph, unit, &alias, "type", &root_module, None);
        }
    }
    for constant in file
        .syntax()
        .descendants()
        .filter_map(ast::Const::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        add_named_type(graph, unit, &constant, "const", &root_module, None);
    }
    for static_node in file
        .syntax()
        .descendants()
        .filter_map(ast::Static::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        add_named_type(graph, unit, &static_node, "static", &root_module, None);
    }
    for macro_node in file
        .syntax()
        .descendants()
        .filter_map(ast::MacroRules::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        add_named_type(graph, unit, &macro_node, "macro", &root_module, None);
    }
    for macro_node in file
        .syntax()
        .descendants()
        .filter_map(ast::MacroDef::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        add_named_type(graph, unit, &macro_node, "macro", &root_module, None);
    }
}

#[allow(clippy::too_many_lines)]
fn collect_callable_definitions(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
    _request: &AnalyzeProjectRequest,
) {
    let sema = semantics.map(|workspace| Semantics::new(&workspace.database));
    let (file, _) = if let Some(sema) = sema.as_ref() {
        semantic_source_file(unit, sema).map_or_else(
            || {
                (
                    SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
                    false,
                )
            },
            |file| (file, true),
        )
    } else {
        (
            SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
            false,
        )
    };
    for function in file
        .syntax()
        .descendants()
        .filter_map(ast::Fn::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let Some(name) = function.name().map(|name| name.text().to_string()) else {
            continue;
        };
        let namespace = namespace_for(unit, function.syntax());
        let hir_function = sema
            .as_ref()
            .and_then(|sema| sema.to_def(&function))
            .filter(|definition| hir_function_belongs_to_unit(*definition, unit, semantics));
        let exact_identity = semantics
            .zip(hir_function)
            .and_then(|(workspace, definition)| hir_callable_identity(workspace, definition));
        let function_resolved = exact_identity.is_some();
        let (kind, stable_id, owner, trait_owner) = exact_identity
            .unwrap_or_else(|| callable_identity(graph, unit, &function, &namespace, &name));
        let metadata = json!({
            "language":"rust",
            "signature":function_signature(&function),
            "async":function.async_token().is_some(),
            "unsafe":function.unsafe_token().is_some(),
            "const":function.const_token().is_some(),
            "test":has_test_attribute(function.syntax()),
            "owner":owner,
            "implemented_trait":trait_owner,
        });
        add_definition_node(
            graph,
            unit,
            function.syntax(),
            stable_id.clone(),
            &kind,
            &name,
            &namespace,
            function_resolved,
            metadata,
        );
        let declaring = owner.unwrap_or_else(|| module_id(&unit.target.crate_key, &namespace));
        graph.add_edge(
            &declaring,
            &stable_id,
            "DECLARES",
            Confidence::StaticInferred,
            unit,
            function.syntax(),
            Value::Null,
        );
        if let Some(trait_id) = trait_owner {
            let exact_trait_method =
                semantics
                    .zip(hir_function)
                    .and_then(|(workspace, implementation_function)| {
                        let database = &workspace.database;
                        let trait_ = implementation_function
                            .as_assoc_item(database)?
                            .implemented_trait(database)?;
                        let trait_function =
                            trait_.function(database, implementation_function.name(database))?;
                        stable_for_hir_function(workspace, trait_function)
                    });
            let prefix = format!(
                "method:{}#",
                trait_id.strip_prefix("trait:").unwrap_or(&trait_id)
            );
            let exact_trait_method_available = exact_trait_method
                .as_ref()
                .is_some_and(|id| graph.nodes.contains_key(id));
            let trait_method = exact_trait_method
                .filter(|id| graph.nodes.contains_key(id))
                .or_else(|| {
                    graph
                        .unique_named(&name, Some("method"))
                        .find(|id| id.starts_with(&prefix))
                });
            if let Some(trait_method) = trait_method {
                graph.add_edge(
                    &stable_id,
                    &trait_method,
                    "IMPLEMENTS_METHOD",
                    if function_resolved && exact_trait_method_available {
                        Confidence::CompilerResolved
                    } else {
                        Confidence::StaticInferred
                    },
                    unit,
                    function.syntax(),
                    json!({"trait":trait_id}),
                );
            }
        }
    }
}

#[allow(clippy::redundant_closure_for_method_calls, clippy::too_many_lines)]
fn collect_relationships(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
    request: &AnalyzeProjectRequest,
) {
    let sema = semantics.map(|workspace| Semantics::new(&workspace.database));
    let (file, _) = if let Some(sema) = sema.as_ref() {
        semantic_source_file(unit, sema).map_or_else(
            || {
                (
                    SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
                    false,
                )
            },
            |file| (file, true),
        )
    } else {
        (
            SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
            false,
        )
    };

    for implementation in file
        .syntax()
        .descendants()
        .filter_map(ast::Impl::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let hir_implementation = sema.as_ref().and_then(|sema| sema.to_def(&implementation));
        let exact_implementation =
            semantics
                .zip(hir_implementation)
                .and_then(|(workspace, implementation)| {
                    let type_id = stable_for_hir_adt(
                        workspace,
                        implementation.self_ty(&workspace.database).as_adt()?,
                    )?;
                    let trait_id = stable_for_hir_trait(
                        workspace,
                        implementation.trait_(&workspace.database)?,
                    )?;
                    (graph.nodes.contains_key(&type_id) && graph.nodes.contains_key(&trait_id))
                        .then_some((type_id, trait_id))
                });
        if let Some((type_id, trait_id)) = exact_implementation {
            graph.add_edge(
                &type_id,
                &trait_id,
                "IMPLEMENTS",
                Confidence::CompilerResolved,
                unit,
                implementation.syntax(),
                json!({"unsafe":implementation.unsafe_token().is_some()}),
            );
            graph.bindings_resolved += 1;
            continue;
        }
        let Some(self_name) = implementation
            .self_ty()
            .and_then(|ty| final_name(&ty.syntax().to_string()))
        else {
            continue;
        };
        let Some(type_id) = graph.first_named(&self_name, Some("type")) else {
            continue;
        };
        if let Some(trait_name) = implementation
            .trait_()
            .and_then(|ty| final_name(&ty.syntax().to_string()))
            && let Some(trait_id) = graph.first_named(&trait_name, Some("trait"))
        {
            graph.add_edge(
                &type_id,
                &trait_id,
                "IMPLEMENTS",
                Confidence::StaticInferred,
                unit,
                implementation.syntax(),
                json!({"unsafe":implementation.unsafe_token().is_some()}),
            );
            graph.bindings_unresolved += 1;
        }
    }

    for function in file
        .syntax()
        .descendants()
        .filter_map(ast::Fn::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let Some(name) = function.name().map(|name| name.text().to_string()) else {
            continue;
        };
        let namespace = namespace_for(unit, function.syntax());
        let exact_identity = sema
            .as_ref()
            .and_then(|sema| sema.to_def(&function))
            .filter(|definition| hir_function_belongs_to_unit(*definition, unit, semantics))
            .and_then(|definition| {
                semantics.and_then(|workspace| hir_callable_identity(workspace, definition))
            });
        let (_, source_id, _, _) = exact_identity
            .unwrap_or_else(|| callable_identity(graph, unit, &function, &namespace, &name));
        if !graph.nodes.contains_key(&source_id) {
            continue;
        }

        collect_signature_types(graph, unit, &function, &source_id);
        if !request.options.include_method_bodies {
            continue;
        }
        for call in function
            .syntax()
            .descendants()
            .filter_map(ast::CallExpr::cast)
            .filter(|call| belongs_to_function(call.syntax(), &function))
        {
            let Some(path) = call
                .expr()
                .and_then(|expr| ast::PathExpr::cast(expr.syntax().clone()))
                .and_then(|expr| expr.path())
            else {
                continue;
            };
            let Some(target_name) = final_name(&path.syntax().to_string()) else {
                continue;
            };
            let resolution = sema.as_ref().and_then(|sema| sema.resolve_path(&path));
            if let Some((target, target_kind)) = exact_path_target(graph, semantics, resolution)
                && matches!(target_kind, "function" | "method" | "type" | "variant")
            {
                let kind = if matches!(target_kind, "type" | "variant") {
                    "CONSTRUCTS"
                } else {
                    "CALLS"
                };
                graph.add_resolved_edge(&source_id, &target, kind, true, unit, call.syntax());
            } else if resolution.is_some() {
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    call.syntax(),
                    "resolved_target_not_indexed",
                    &target_name,
                    "rust-analyzer resolved the call, but the exact HIR target has no Graphine node in this registered project",
                ));
            } else if let Some(target) = graph
                .first_named(&target_name, Some("function"))
                .or_else(|| graph.first_named(&target_name, Some("method")))
                .or_else(|| graph.first_named(&target_name, Some("type")))
                .or_else(|| graph.first_named(&target_name, Some("variant")))
            {
                let kind = if graph
                    .nodes
                    .get(&target)
                    .is_some_and(|node| matches!(node.kind.as_str(), "type" | "variant"))
                {
                    "CONSTRUCTS"
                } else {
                    "CALLS"
                };
                graph.add_resolved_edge(&source_id, &target, kind, false, unit, call.syntax());
            } else {
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    call.syntax(),
                    "unresolved_call",
                    &target_name,
                    "call target was not available in the selected Cargo graph",
                ));
            }
        }
        for call in function
            .syntax()
            .descendants()
            .filter_map(ast::MethodCallExpr::cast)
            .filter(|call| belongs_to_function(call.syntax(), &function))
        {
            let Some(target_name) = call.name_ref().map(|name| name.text().to_string()) else {
                continue;
            };
            let resolved_function = sema
                .as_ref()
                .and_then(|sema| sema.resolve_method_call(&call));
            let dynamic_trait = sema
                .as_ref()
                .and_then(|sema| {
                    call.receiver()
                        .and_then(|receiver| sema.type_of_expr(&receiver))
                })
                .is_some_and(|type_info| type_info.adjusted().as_dyn_trait().is_some());
            let candidates = graph
                .unique_named(&target_name, Some("method"))
                .collect::<Vec<_>>();
            let exact_target =
                semantics
                    .zip(resolved_function)
                    .and_then(|(workspace, function)| {
                        let database = &workspace.database;
                        let target_function = if dynamic_trait {
                            function
                                .as_assoc_item(database)
                                .and_then(|item| item.container_or_implemented_trait(database))
                                .and_then(|trait_| {
                                    trait_.function(database, function.name(database))
                                })
                                .unwrap_or(function)
                        } else {
                            function
                        };
                        let stable_id = stable_for_hir_function(workspace, target_function)?;
                        graph
                            .nodes
                            .contains_key(&stable_id)
                            .then_some((stable_id, target_function))
                    });
            if let Some((target, resolved_function)) = exact_target {
                let workspace = semantics.expect("exact target requires semantics");
                let container = resolved_function
                    .as_assoc_item(&workspace.database)
                    .map(|item| item.container(&workspace.database));
                let (dispatch, runtime_implementation_inferred) = if dynamic_trait {
                    ("dynamic_trait", false)
                } else if matches!(container, Some(AssocItemContainer::Impl(_))) {
                    ("static", true)
                } else {
                    ("trait", false)
                };
                graph.bindings_resolved += 1;
                graph.add_edge(
                    &source_id,
                    &target,
                    "CALLS",
                    Confidence::CompilerResolved,
                    unit,
                    call.syntax(),
                    json!({
                        "dispatch":dispatch,
                        "runtime_implementation_inferred":runtime_implementation_inferred,
                    }),
                );
            } else if resolved_function.is_some() {
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    call.syntax(),
                    "resolved_method_not_indexed",
                    &target_name,
                    "rust-analyzer resolved the method, but its exact HIR identifier has no Graphine node in this registered project",
                ));
            } else if candidates.len() == 1 {
                graph.add_resolved_edge(
                    &source_id,
                    &candidates[0],
                    "CALLS",
                    false,
                    unit,
                    call.syntax(),
                );
            } else {
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    call.syntax(),
                    if candidates.is_empty() {
                        "unresolved_method_call"
                    } else {
                        "ambiguous_method_call"
                    },
                    &target_name,
                    "method target could not be uniquely represented",
                ));
            }
        }
        for record in function
            .syntax()
            .descendants()
            .filter_map(ast::RecordExpr::cast)
            .filter(|record| belongs_to_function(record.syntax(), &function))
        {
            let record_path = record.path();
            let resolution = record_path
                .as_ref()
                .and_then(|path| sema.as_ref().and_then(|sema| sema.resolve_path(path)));
            if let Some((type_id, "type")) = exact_path_target(graph, semantics, resolution) {
                graph.add_resolved_edge(
                    &source_id,
                    &type_id,
                    "CONSTRUCTS",
                    true,
                    unit,
                    record.syntax(),
                );
            } else if resolution.is_some() {
                let name = record_path
                    .as_ref()
                    .and_then(|path| final_name(&path.syntax().to_string()))
                    .unwrap_or_else(|| "record".to_owned());
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    record.syntax(),
                    "resolved_type_not_indexed",
                    &name,
                    "rust-analyzer resolved the record type, but its exact HIR identifier has no Graphine node",
                ));
            } else if let Some(type_name) =
                record_path.and_then(|path| final_name(&path.syntax().to_string()))
                && let Some(type_id) = graph.first_named(&type_name, Some("type"))
            {
                graph.add_resolved_edge(
                    &source_id,
                    &type_id,
                    "CONSTRUCTS",
                    false,
                    unit,
                    record.syntax(),
                );
            }
        }
        for macro_call in function
            .syntax()
            .descendants()
            .filter_map(ast::MacroCall::cast)
            .filter(|call| belongs_to_function(call.syntax(), &function))
        {
            let Some(macro_name) = macro_call
                .path()
                .and_then(|path| final_name(&path.syntax().to_string()))
            else {
                continue;
            };
            let expansion = sema
                .as_ref()
                .and_then(|sema| sema.expand_macro_call(&macro_call));
            let resolved_macro = sema
                .as_ref()
                .and_then(|sema| sema.resolve_macro_call(&macro_call));
            let exact_macro = semantics
                .zip(resolved_macro)
                .and_then(|(workspace, macro_)| {
                    let (stable_id, _) =
                        stable_for_hir_module_def(workspace, ModuleDef::Macro(macro_))?;
                    graph.nodes.contains_key(&stable_id).then_some(stable_id)
                });
            if let Some(macro_id) = exact_macro {
                graph.add_resolved_edge(
                    &source_id,
                    &macro_id,
                    "INVOKES_MACRO",
                    true,
                    unit,
                    macro_call.syntax(),
                );
            } else if resolved_macro.is_some() {
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    macro_call.syntax(),
                    "resolved_macro_not_indexed",
                    &macro_name,
                    "rust-analyzer resolved the macro, but its exact HIR identifier has no Graphine node",
                ));
            } else if let Some(macro_id) = graph.first_named(&macro_name, Some("macro")) {
                graph.add_resolved_edge(
                    &source_id,
                    &macro_id,
                    "INVOKES_MACRO",
                    false,
                    unit,
                    macro_call.syntax(),
                );
            }
            if let Some(sema) = sema.as_ref()
                && let Some(expansion) = expansion
            {
                for expanded_call in expansion
                    .value
                    .descendants()
                    .filter_map(ast::CallExpr::cast)
                {
                    let Some(path) = expanded_call
                        .expr()
                        .and_then(|expr| ast::PathExpr::cast(expr.syntax().clone()))
                        .and_then(|expr| expr.path())
                    else {
                        continue;
                    };
                    let Some(target_name) = final_name(&path.syntax().to_string()) else {
                        continue;
                    };
                    let resolution = sema.resolve_path(&path);
                    if let Some((target, "function" | "method")) =
                        exact_path_target(graph, semantics, resolution)
                    {
                        graph.bindings_resolved += 1;
                        graph.add_edge(
                            &source_id,
                            &target,
                            "CALLS",
                            Confidence::CompilerResolved,
                            unit,
                            macro_call.syntax(),
                            json!({"inside_macro":macro_name}),
                        );
                    } else if resolution.is_some() {
                        graph.bindings_unresolved += 1;
                        graph.diagnostics.push(symbol_diagnostic(
                            unit,
                            macro_call.syntax(),
                            "resolved_macro_call_target_not_indexed",
                            &target_name,
                            "macro expansion resolved to a HIR function without a Graphine node",
                        ));
                    } else if let Some(target) = graph.first_named(&target_name, Some("function")) {
                        graph.bindings_unresolved += 1;
                        graph.add_edge(
                            &source_id,
                            &target,
                            "CALLS",
                            Confidence::StaticInferred,
                            unit,
                            macro_call.syntax(),
                            json!({"inside_macro":macro_name}),
                        );
                    }
                }
            }
        }
        if request.options.include_field_access {
            for field in function
                .syntax()
                .descendants()
                .filter_map(ast::FieldExpr::cast)
                .filter(|field| belongs_to_function(field.syntax(), &function))
            {
                let Some(field_name) = field.name_ref().map(|name| name.text().to_string()) else {
                    continue;
                };
                let candidates = graph
                    .unique_named(&field_name, Some("field"))
                    .collect::<Vec<_>>();
                let resolved_field = sema
                    .as_ref()
                    .and_then(|sema| sema.resolve_field(&field))
                    .and_then(|field| field.left());
                let exact_field = semantics
                    .zip(resolved_field)
                    .and_then(|(workspace, field)| {
                        let stable_id = stable_for_hir_field(workspace, field)?;
                        graph.nodes.contains_key(&stable_id).then_some(stable_id)
                    });
                if exact_field.is_some() || (resolved_field.is_none() && candidates.len() == 1) {
                    let target = exact_field
                        .as_ref()
                        .or_else(|| candidates.first())
                        .expect("a field target was checked above");
                    let compiler_resolved = exact_field.is_some();
                    let assignment =
                        field
                            .syntax()
                            .parent()
                            .and_then(ast::BinExpr::cast)
                            .filter(|binary| {
                                matches!(binary.op_kind(), Some(ast::BinaryOp::Assignment { .. }))
                                    && binary.lhs().is_some_and(|left| {
                                        left.syntax()
                                            .text_range()
                                            .contains_range(field.syntax().text_range())
                                    })
                            });
                    let write = assignment.is_some();
                    graph.add_resolved_edge(
                        &source_id,
                        target,
                        if write { "WRITES_FIELD" } else { "READS_FIELD" },
                        compiler_resolved,
                        unit,
                        field.syntax(),
                    );
                    if assignment.is_some_and(|binary| {
                        matches!(
                            binary.op_kind(),
                            Some(ast::BinaryOp::Assignment { op: Some(_) })
                        )
                    }) {
                        graph.add_resolved_edge(
                            &source_id,
                            target,
                            "READS_FIELD",
                            compiler_resolved,
                            unit,
                            field.syntax(),
                        );
                    }
                } else if resolved_field.is_some() {
                    graph.bindings_unresolved += 1;
                    graph.diagnostics.push(symbol_diagnostic(
                        unit,
                        field.syntax(),
                        "resolved_field_not_indexed",
                        &field_name,
                        "rust-analyzer resolved the field, but its exact HIR identifier has no Graphine node",
                    ));
                }
            }
        }
    }

    for use_item in file
        .syntax()
        .descendants()
        .filter_map(ast::Use::cast)
        .filter(|item| cfg_enabled(item.syntax(), unit, semantics))
    {
        let Some(tree) = use_item.use_tree() else {
            continue;
        };
        let source = module_id(
            &unit.target.crate_key,
            &namespace_for(unit, use_item.syntax()),
        );
        for imported in std::iter::once(tree.clone())
            .chain(tree.syntax().descendants().filter_map(ast::UseTree::cast))
        {
            let imported_path = imported.path();
            let resolution = imported_path
                .as_ref()
                .and_then(|path| sema.as_ref().and_then(|sema| sema.resolve_path(path)));
            let Some(name) = imported_path.and_then(|path| final_name(&path.syntax().to_string()))
            else {
                continue;
            };
            if let Some((target, _)) = exact_path_target(graph, semantics, resolution) {
                graph.add_resolved_edge(&source, &target, "IMPORTS", true, unit, imported.syntax());
            } else if resolution.is_some() {
                graph.bindings_unresolved += 1;
                graph.diagnostics.push(symbol_diagnostic(
                    unit,
                    imported.syntax(),
                    "resolved_import_not_indexed",
                    &name,
                    "rust-analyzer resolved the import, but its exact HIR identifier has no Graphine node",
                ));
            } else if let Some(target) = graph.first_named(&name, None) {
                graph.add_resolved_edge(
                    &source,
                    &target,
                    "IMPORTS",
                    false,
                    unit,
                    imported.syntax(),
                );
            }
        }
    }
}

fn collect_signature_types(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    function: &ast::Fn,
    source_id: &str,
) {
    if let Some(parameters) = function.param_list() {
        for path_type in parameters
            .syntax()
            .descendants()
            .filter_map(ast::PathType::cast)
        {
            add_type_reference(graph, unit, source_id, "ACCEPTS_TYPE", &path_type);
        }
    }
    if let Some(return_type) = function.ret_type() {
        for path_type in return_type
            .syntax()
            .descendants()
            .filter_map(ast::PathType::cast)
        {
            add_type_reference(graph, unit, source_id, "RETURNS_TYPE", &path_type);
        }
    }
}

fn add_type_reference(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    source_id: &str,
    kind: &str,
    path_type: &ast::PathType,
) {
    let Some(name) = path_type
        .path()
        .and_then(|path| final_name(&path.syntax().to_string()))
    else {
        return;
    };
    let target = graph
        .first_named(&name, Some("type"))
        .or_else(|| graph.first_named(&name, Some("trait")));
    if let Some(target) = target {
        graph.add_edge(
            source_id,
            &target,
            kind,
            Confidence::StaticInferred,
            unit,
            path_type.syntax(),
            Value::Null,
        );
    }
}

fn add_named_type<N: AstNode + HasName>(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    node: &N,
    kind: &str,
    root_module: &str,
    exact_stable_id: Option<String>,
) {
    let Some(name) = node.name().map(|name| name.text().to_string()) else {
        return;
    };
    let namespace = namespace_for(unit, node.syntax());
    let semantic = exact_stable_id.is_some();
    let stable_id = exact_stable_id.unwrap_or_else(|| stable_for(kind, unit, &namespace, &name));
    add_definition_node(
        graph,
        unit,
        node.syntax(),
        stable_id.clone(),
        kind,
        &name,
        &namespace,
        semantic,
        json!({"language":"rust"}),
    );
    let declaring = if namespace == unit.module_path {
        root_module.to_owned()
    } else {
        module_id(&unit.target.crate_key, &namespace)
    };
    graph.add_edge(
        &declaring,
        &stable_id,
        "DECLARES",
        Confidence::StaticInferred,
        unit,
        node.syntax(),
        Value::Null,
    );
}

fn collect_fields(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    owner: &ast::Struct,
    fields: &ast::FieldList,
    semantics: Option<&SemanticWorkspace>,
    sema: Option<&Semantics<'_, RootDatabase>>,
) {
    if let ast::FieldList::RecordFieldList(fields) = fields {
        for field in fields.fields() {
            add_field(graph, unit, owner, &field, semantics, sema);
        }
    }
}

fn add_field<N: AstNode + HasName>(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    owner: &N,
    field: &ast::RecordField,
    semantics: Option<&SemanticWorkspace>,
    sema: Option<&Semantics<'_, RootDatabase>>,
) {
    let (Some(owner_name), Some(field_name)) = (
        owner.name().map(|name| name.text().to_string()),
        field.name().map(|name| name.text().to_string()),
    ) else {
        return;
    };
    let namespace = namespace_for(unit, owner.syntax());
    let owner_id = stable_for("type", unit, &namespace, &owner_name);
    let inferred_stable_id = format!(
        "field:{}#{}",
        owner_id.strip_prefix("type:").unwrap_or(&owner_id),
        field_name
    );
    let exact_stable_id = sema
        .and_then(|sema| sema.to_def(field))
        .and_then(|definition| {
            semantics.and_then(|workspace| stable_for_hir_field(workspace, definition))
        })
        .filter(|id| stable_id_belongs_to_unit(id, unit));
    let semantic = exact_stable_id.is_some();
    let stable_id = exact_stable_id.unwrap_or(inferred_stable_id);
    add_definition_node(
        graph,
        unit,
        field.syntax(),
        stable_id.clone(),
        "field",
        &field_name,
        &namespace,
        semantic,
        json!({"language":"rust","owner":owner_id}),
    );
    graph.add_edge(
        &owner_id,
        &stable_id,
        "DECLARES",
        Confidence::StaticInferred,
        unit,
        field.syntax(),
        Value::Null,
    );
    if let Some(ty) = field.ty() {
        for path_type in ty.syntax().descendants().filter_map(ast::PathType::cast) {
            add_type_reference(graph, unit, &stable_id, "REFERENCES_TYPE", &path_type);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_definition_node(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    syntax: &ra_ap_syntax::SyntaxNode,
    stable_id: String,
    kind: &str,
    name: &str,
    namespace: &str,
    semantic: bool,
    metadata: Value,
) {
    let (start_line, end_line) = line_range(&unit.text, syntax);
    let confidence = if semantic {
        Confidence::CompilerResolved
    } else {
        Confidence::StaticInferred
    };
    graph.add_definition(
        Definition {
            stable_id: stable_id.clone(),
            simple_name: name.to_owned(),
        },
        SyntheticNode {
            stable_id,
            kind: kind.to_owned(),
            qualified_name: format!("{}::{namespace}::{name}", unit.target.package),
            simple_name: Some(name.to_owned()),
            module_name: Some(unit.target.crate_key.clone()),
            package_name: None,
            namespace_path: Some(namespace.to_owned()),
            file_path: Some(unit.relative_path.clone()),
            start_line: Some(start_line),
            end_line: Some(end_line),
            confidence,
            provenance: if semantic {
                "rust-analyzer"
            } else {
                "rust-syntax"
            }
            .to_owned(),
            metadata,
            unresolved: Vec::new(),
        },
    );
}

fn callable_identity(
    graph: &GraphBuilder,
    unit: &SourceUnit,
    function: &ast::Fn,
    namespace: &str,
    name: &str,
) -> (String, String, Option<String>, Option<String>) {
    if let Some(implementation) = function
        .syntax()
        .ancestors()
        .skip(1)
        .find_map(ast::Impl::cast)
    {
        let type_name = implementation
            .self_ty()
            .and_then(|ty| final_name(&ty.syntax().to_string()))
            .unwrap_or_else(|| "Self".to_owned());
        let type_id = graph
            .unique_named(&type_name, Some("type"))
            .next()
            .unwrap_or_else(|| stable_for("type", unit, namespace, &type_name));
        let trait_id = implementation
            .trait_()
            .and_then(|ty| final_name(&ty.syntax().to_string()))
            .and_then(|trait_name| graph.unique_named(&trait_name, Some("trait")).next());
        let owner_body = if let Some(trait_id) = &trait_id {
            format!(
                "{} as {}",
                type_id.strip_prefix("type:").unwrap_or(&type_id),
                trait_id.strip_prefix("trait:").unwrap_or(trait_id)
            )
        } else {
            type_id.strip_prefix("type:").unwrap_or(&type_id).to_owned()
        };
        return (
            "method".to_owned(),
            format!("method:{owner_body}#{name}()"),
            Some(type_id),
            trait_id,
        );
    }
    if let Some(trait_node) = function
        .syntax()
        .ancestors()
        .skip(1)
        .find_map(ast::Trait::cast)
        && let Some(trait_name) = trait_node.name().map(|name| name.text().to_string())
    {
        let trait_id = graph
            .unique_named(&trait_name, Some("trait"))
            .next()
            .unwrap_or_else(|| stable_for("trait", unit, namespace, &trait_name));
        return (
            "method".to_owned(),
            format!(
                "method:{}#{name}()",
                trait_id.strip_prefix("trait:").unwrap_or(&trait_id)
            ),
            Some(trait_id),
            None,
        );
    }
    (
        "function".to_owned(),
        format!("function:{}::{namespace}::{name}()", unit.target.crate_key),
        None,
        None,
    )
}

fn emit_crates_and_modules(graph: &mut GraphBuilder, targets: &[TargetInfo], units: &[SourceUnit]) {
    for target in targets {
        graph.nodes.insert(
            target.crate_key.clone(),
            SyntheticNode {
                stable_id: target.crate_key.clone(),
                kind: "crate".to_owned(),
                qualified_name: format!("{}::{}", target.package, target.name),
                simple_name: Some(target.name.clone()),
                module_name: Some(target.crate_key.clone()),
                package_name: None,
                namespace_path: Some("crate".to_owned()),
                file_path: None,
                start_line: None,
                end_line: None,
                confidence: Confidence::CompilerResolved,
                provenance: "cargo".to_owned(),
                metadata: json!({
                    "language":"rust",
                    "package":target.package,
                    "target_kind":target.kind,
                    "target_name":target.name,
                }),
                unresolved: Vec::new(),
            },
        );
    }
    let modules = units
        .iter()
        .map(|unit| (unit.target.crate_key.clone(), unit.module_path.clone()))
        .collect::<BTreeSet<_>>();
    for (crate_key, namespace) in modules {
        let stable_id = module_id(&crate_key, &namespace);
        graph
            .nodes
            .entry(stable_id.clone())
            .or_insert_with(|| SyntheticNode {
                stable_id: stable_id.clone(),
                kind: "module".to_owned(),
                qualified_name: format!("{crate_key}::{namespace}"),
                simple_name: namespace.rsplit("::").next().map(str::to_owned),
                module_name: Some(crate_key.clone()),
                package_name: None,
                namespace_path: Some(namespace.clone()),
                file_path: None,
                start_line: None,
                end_line: None,
                confidence: Confidence::StaticInferred,
                provenance: "cargo".to_owned(),
                metadata: json!({"language":"rust"}),
                unresolved: Vec::new(),
            });
        graph.add_edge_without_occurrence(
            &crate_key,
            &stable_id,
            "DECLARES",
            Confidence::StaticInferred,
            json!({}),
        );
    }
}

impl GraphBuilder {
    fn add_definition(&mut self, definition: Definition, node: SyntheticNode) {
        if self.nodes.insert(node.stable_id.clone(), node).is_none() {
            self.names
                .entry(definition.simple_name.clone())
                .or_default()
                .push(definition.stable_id.clone());
            self.definitions.push(definition);
        }
    }

    fn unique_named<'a>(
        &'a self,
        name: &'a str,
        kind: Option<&'a str>,
    ) -> impl Iterator<Item = String> + 'a {
        self.names
            .get(name)
            .into_iter()
            .flatten()
            .filter(move |id| {
                kind.is_none_or(|kind| self.nodes.get(*id).is_some_and(|node| node.kind == kind))
            })
            .cloned()
    }

    fn first_named(&self, name: &str, kind: Option<&str>) -> Option<String> {
        self.names.get(name).and_then(|ids| {
            ids.iter()
                .find(|id| {
                    kind.is_none_or(|kind| {
                        self.nodes.get(*id).is_some_and(|node| node.kind == kind)
                    })
                })
                .cloned()
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn add_edge(
        &mut self,
        source: &str,
        target: &str,
        kind: &str,
        confidence: Confidence,
        unit: &SourceUnit,
        syntax: &ra_ap_syntax::SyntaxNode,
        metadata: Value,
    ) {
        if !self.nodes.contains_key(source) || !self.nodes.contains_key(target) {
            return;
        }
        let key = (source.to_owned(), target.to_owned(), kind.to_owned());
        let edge = self.edges.entry(key).or_insert_with(|| SyntheticEdge {
            source_stable_id: source.to_owned(),
            target_stable_id: target.to_owned(),
            kind: kind.to_owned(),
            confidence,
            provenance: "rust-analyzer".to_owned(),
            metadata: if metadata.is_null() {
                json!({})
            } else {
                metadata
            },
            occurrences: Vec::new(),
        });
        let (start_line, end_line) = line_range(&unit.text, syntax);
        let closure = syntax
            .ancestors()
            .any(|ancestor| ast::ClosureExpr::cast(ancestor).is_some());
        edge.occurrences.push(EdgeOccurrence {
            file_path: unit.relative_path.clone(),
            start_line,
            end_line,
            metadata: json!({"inside_closure":closure}),
        });
    }

    fn add_edge_without_occurrence(
        &mut self,
        source: &str,
        target: &str,
        kind: &str,
        confidence: Confidence,
        metadata: Value,
    ) {
        self.edges
            .entry((source.to_owned(), target.to_owned(), kind.to_owned()))
            .or_insert_with(|| SyntheticEdge {
                source_stable_id: source.to_owned(),
                target_stable_id: target.to_owned(),
                kind: kind.to_owned(),
                confidence,
                provenance: "cargo".to_owned(),
                metadata,
                occurrences: Vec::new(),
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn add_resolved_edge(
        &mut self,
        source: &str,
        target: &str,
        kind: &str,
        compiler_resolved: bool,
        unit: &SourceUnit,
        syntax: &ra_ap_syntax::SyntaxNode,
    ) {
        if compiler_resolved {
            self.bindings_resolved += 1;
        } else {
            self.bindings_unresolved += 1;
        }
        self.add_edge(
            source,
            target,
            kind,
            if compiler_resolved {
                Confidence::CompilerResolved
            } else {
                Confidence::StaticInferred
            },
            unit,
            syntax,
            json!({"dispatch":if kind == "CALLS" {"static"} else {"direct"}}),
        );
    }
}

fn parse_unit(unit: &SourceUnit, semantics: Option<&SemanticWorkspace>) -> (SourceFile, bool) {
    if let Some(workspace) = semantics {
        let sema = Semantics::new(&workspace.database);
        if let Some(file) = semantic_source_file(unit, &sema) {
            return (file, true);
        }
    }
    (
        SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
        false,
    )
}

fn semantic_source_file(
    unit: &SourceUnit,
    sema: &Semantics<'_, RootDatabase>,
) -> Option<SourceFile> {
    SourceFile::cast(sema.module_definition_node(unit.hir_module?).value)
}

fn fingerprint(
    root: &Path,
    units: &[SourceUnit],
    request: &AnalyzeProjectRequest,
    analyzer_version: &str,
    metadata_complete: bool,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"graphine-rust-fingerprint-v1\0");
    hasher.update(analyzer_version.as_bytes());
    hasher.update(format!("{:?}", request.mode).as_bytes());
    hasher.update(serde_json::to_vec(&request.cargo)?);
    hasher.update(request.cargo_executable.to_string_lossy().as_bytes());
    hasher.update(request.rustc_executable.to_string_lossy().as_bytes());
    hasher.update([u8::from(metadata_complete)]);
    let mut project_configuration = WalkDir::new(root)
        .into_iter()
        .filter_entry(include_entry)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| {
            matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml")
            ) || relative_slash(root, path).ends_with(".cargo/config")
                || relative_slash(root, path).ends_with(".cargo/config.toml")
        })
        .collect::<Vec<_>>();
    project_configuration.sort();
    for path in project_configuration {
        hasher.update(relative_slash(root, &path).as_bytes());
        hasher.update(fs::read(path)?);
    }
    for unit in units {
        hasher.update(unit.target.crate_key.as_bytes());
        hasher.update(unit.relative_path.as_bytes());
        hasher.update(unit.text.as_bytes());
    }
    if request.mode == AnalyzerMode::Trusted {
        let generated_root = trusted_target_directory(root);
        let mut generated = WalkDir::new(&generated_root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(walkdir::DirEntry::into_path)
            .filter(|path| {
                let relative = relative_slash(&generated_root, path);
                relative.contains("/build/")
                    && (relative.contains("/out/")
                        || matches!(
                            path.file_name().and_then(|name| name.to_str()),
                            Some("output" | "root-output" | "stderr")
                        ))
            })
            .collect::<Vec<_>>();
        generated.sort();
        for path in generated {
            let metadata = fs::metadata(&path)?;
            if metadata.len() <= 8 * 1024 * 1024 {
                hasher.update(relative_slash(&generated_root, &path).as_bytes());
                hasher.update(fs::read(path)?);
            }
        }
    }
    for executable in [&request.cargo_executable, &request.rustc_executable] {
        if let Ok(output) = Command::new(executable).arg("--version").output() {
            hasher.update(&output.stdout);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn trusted_target_directory(root: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    std::env::temp_dir()
        .join("graphine-rust-targets")
        .join(&digest[..24])
}

fn function_signature(function: &ast::Fn) -> String {
    let mut signature = String::new();
    if function.async_token().is_some() {
        signature.push_str("async ");
    }
    if function.unsafe_token().is_some() {
        signature.push_str("unsafe ");
    }
    signature.push_str("fn ");
    signature.push_str(
        &function
            .name()
            .map_or_else(|| "_".to_owned(), |name| name.text().to_string()),
    );
    signature.push_str(&function.param_list().map_or_else(
        || "()".to_owned(),
        |parameters| parameters.syntax().to_string(),
    ));
    if let Some(return_type) = function.ret_type() {
        signature.push(' ');
        signature.push_str(return_type.syntax().text().to_string().trim());
    }
    signature
}

fn stable_for(kind: &str, unit: &SourceUnit, namespace: &str, name: &str) -> String {
    format!("{kind}:{}::{namespace}::{name}", unit.target.crate_key)
}

fn stable_id_belongs_to_unit(stable_id: &str, unit: &SourceUnit) -> bool {
    stable_id.contains(&format!("{}::", unit.target.crate_key))
}

fn hir_module_path(module: Module, database: &RootDatabase) -> String {
    let mut segments = module
        .path_to_root(database)
        .into_iter()
        .filter_map(|module| module.name(database))
        .map(|name| name.as_str().to_owned())
        .collect::<Vec<_>>();
    segments.reverse();
    if segments.is_empty() {
        "crate".to_owned()
    } else {
        segments.join("::")
    }
}

fn hir_named_stable_id(
    workspace: &SemanticWorkspace,
    kind: &str,
    module: Module,
    name: &str,
) -> Option<String> {
    let target = workspace
        .crate_targets
        .get(&module.krate(&workspace.database))?;
    Some(format!(
        "{kind}:{}::{}::{name}",
        target.crate_key,
        hir_module_path(module, &workspace.database)
    ))
}

fn stable_for_hir_adt(workspace: &SemanticWorkspace, adt: Adt) -> Option<String> {
    hir_named_stable_id(
        workspace,
        "type",
        adt.module(&workspace.database),
        adt.name(&workspace.database).as_str(),
    )
}

fn stable_for_hir_trait(workspace: &SemanticWorkspace, trait_: Trait) -> Option<String> {
    hir_named_stable_id(
        workspace,
        "trait",
        trait_.module(&workspace.database),
        trait_.name(&workspace.database).as_str(),
    )
}

fn stable_for_hir_function(workspace: &SemanticWorkspace, function: Function) -> Option<String> {
    hir_callable_identity(workspace, function).map(|(_, stable_id, _, _)| stable_id)
}

fn hir_function_belongs_to_unit(
    function: Function,
    unit: &SourceUnit,
    workspace: Option<&SemanticWorkspace>,
) -> bool {
    let (Some(workspace), Some(unit_module)) = (workspace, unit.hir_module) else {
        return false;
    };
    function
        .module(&workspace.database)
        .krate(&workspace.database)
        == unit_module.krate(&workspace.database)
}

fn hir_callable_identity(
    workspace: &SemanticWorkspace,
    function: Function,
) -> Option<(String, String, Option<String>, Option<String>)> {
    let database = &workspace.database;
    let name = function.name(database).as_str().to_owned();
    if let Some(item) = function.as_assoc_item(database) {
        let (owner_body, owner, implemented_trait) = match item.container(database) {
            AssocItemContainer::Trait(trait_) => {
                let trait_id = stable_for_hir_trait(workspace, trait_)?;
                (
                    trait_id.strip_prefix("trait:")?.to_owned(),
                    Some(trait_id),
                    None,
                )
            }
            AssocItemContainer::Impl(implementation) => {
                let owner_id =
                    stable_for_hir_adt(workspace, implementation.self_ty(database).as_adt()?)?;
                let owner_body = owner_id.strip_prefix("type:")?;
                if let Some(trait_) = implementation.trait_(database) {
                    let trait_id = stable_for_hir_trait(workspace, trait_)?;
                    (
                        format!("{owner_body} as {}", trait_id.strip_prefix("trait:")?),
                        Some(owner_id),
                        Some(trait_id),
                    )
                } else {
                    (owner_body.to_owned(), Some(owner_id), None)
                }
            }
        };
        return Some((
            "method".to_owned(),
            format!("method:{owner_body}#{name}()"),
            owner,
            implemented_trait,
        ));
    }
    Some((
        "function".to_owned(),
        hir_named_stable_id(
            workspace,
            "function",
            function.module(database),
            &format!("{name}()"),
        )?,
        None,
        None,
    ))
}

fn stable_for_hir_field(workspace: &SemanticWorkspace, field: ra_ap_hir::Field) -> Option<String> {
    let database = &workspace.database;
    let parent = field.parent_def(database);
    if matches!(parent, Variant::EnumVariant(_)) {
        return None;
    }
    let owner = stable_for_hir_adt(workspace, parent.adt(database))?;
    Some(format!(
        "field:{}#{}",
        owner.strip_prefix("type:")?,
        field.name(database).as_str()
    ))
}

fn stable_for_hir_module_def(
    workspace: &SemanticWorkspace,
    definition: ModuleDef,
) -> Option<(String, &'static str)> {
    let database = &workspace.database;
    match definition {
        ModuleDef::Module(module) => {
            let target = workspace.crate_targets.get(&module.krate(database))?;
            Some((
                module_id(&target.crate_key, &hir_module_path(module, database)),
                "module",
            ))
        }
        ModuleDef::Function(function) => {
            let stable_id = stable_for_hir_function(workspace, function)?;
            let kind = if stable_id.starts_with("method:") {
                "method"
            } else {
                "function"
            };
            Some((stable_id, kind))
        }
        ModuleDef::Adt(adt) => Some((stable_for_hir_adt(workspace, adt)?, "type")),
        ModuleDef::EnumVariant(variant) => {
            let owner = stable_for_hir_adt(workspace, variant.parent_enum(database).into())?;
            Some((
                format!(
                    "variant:{}::{}",
                    owner.strip_prefix("type:")?,
                    variant.name(database).as_str()
                ),
                "variant",
            ))
        }
        ModuleDef::Trait(trait_) => Some((stable_for_hir_trait(workspace, trait_)?, "trait")),
        ModuleDef::Const(constant) => Some((
            hir_named_stable_id(
                workspace,
                "const",
                constant.module(database),
                constant.name(database)?.as_str(),
            )?,
            "const",
        )),
        ModuleDef::Static(static_) => Some((
            hir_named_stable_id(
                workspace,
                "static",
                static_.module(database),
                static_.name(database).as_str(),
            )?,
            "static",
        )),
        ModuleDef::TypeAlias(alias) => Some((
            hir_named_stable_id(
                workspace,
                "type",
                alias.module(database),
                alias.name(database).as_str(),
            )?,
            "type",
        )),
        ModuleDef::Macro(macro_) => Some((
            hir_named_stable_id(
                workspace,
                "macro",
                macro_.module(database),
                macro_.name(database).as_str(),
            )?,
            "macro",
        )),
        ModuleDef::BuiltinType(_) => None,
    }
}

fn exact_path_target(
    graph: &GraphBuilder,
    workspace: Option<&SemanticWorkspace>,
    resolution: Option<PathResolution>,
) -> Option<(String, &'static str)> {
    let (stable_id, kind) = match resolution? {
        PathResolution::Def(definition) => stable_for_hir_module_def(workspace?, definition)?,
        _ => return None,
    };
    graph
        .nodes
        .contains_key(&stable_id)
        .then_some((stable_id, kind))
}

fn module_id(crate_key: &str, namespace: &str) -> String {
    format!("module:{crate_key}::{namespace}")
}

fn crate_key(package: &str, kind: &str, name: &str) -> String {
    format!("crate:{package}/{kind}/{name}")
}

fn module_path(target: &TargetInfo, path: &Path) -> String {
    if same_path(&target.source, path) {
        return "crate".to_owned();
    }
    let root = target.source.parent().unwrap_or(&target.source);
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut parts = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(last) = parts.last_mut() {
        *last = last.trim_end_matches(".rs").to_owned();
        if last == "mod" {
            parts.pop();
        }
    }
    if parts.is_empty() {
        "crate".to_owned()
    } else {
        parts.join("::")
    }
}

fn namespace_for(unit: &SourceUnit, syntax: &ra_ap_syntax::SyntaxNode) -> String {
    let mut inline = syntax
        .ancestors()
        .skip(1)
        .filter_map(ast::Module::cast)
        .filter(|module| module.item_list().is_some())
        .filter_map(|module| module.name().map(|name| name.text().to_string()))
        .collect::<Vec<_>>();
    inline.reverse();
    if inline.is_empty() {
        unit.module_path.clone()
    } else {
        format!("{}::{}", unit.module_path, inline.join("::"))
    }
}

fn final_name(path: &str) -> Option<String> {
    let without_generics = strip_generics(path);
    without_generics
        .trim()
        .trim_end_matches(['!', ';'])
        .rsplit("::")
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "self" && *value != "*")
        .map(str::to_owned)
}

fn strip_generics(value: &str) -> String {
    let mut depth = 0_u32;
    value
        .chars()
        .filter(|character| match character {
            '<' => {
                depth += 1;
                false
            }
            '>' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CfgState {
    Active,
    Inactive,
    Unknown,
}

struct CfgAudit {
    excluded: bool,
    diagnostics: Vec<AnalyzerDiagnostic>,
}

fn audit_cfg_items(units: &[SourceUnit], semantics: Option<&SemanticWorkspace>) -> CfgAudit {
    let mut excluded = false;
    let mut diagnostics = Vec::new();
    let mut reported = BTreeSet::new();
    for unit in units {
        let (file, _) = parse_unit(unit, semantics);
        for item in file.syntax().descendants().filter_map(ast::Item::cast) {
            match cfg_state(item.syntax(), unit, semantics) {
                CfgState::Active => {}
                CfgState::Inactive => excluded = true,
                CfgState::Unknown => {
                    let range = line_range(&unit.text, item.syntax());
                    if reported.insert((unit.relative_path.clone(), range)) {
                        diagnostics.push(symbol_diagnostic(
                            unit,
                            item.syntax(),
                            "cfg_configuration_unknown",
                            "cfg",
                            "rust-analyzer could not determine whether this cfg/cfg_attr expression is active; the item was excluded conservatively",
                        ));
                    }
                }
            }
        }
    }
    CfgAudit {
        excluded,
        diagnostics,
    }
}

fn cfg_enabled(
    syntax: &ra_ap_syntax::SyntaxNode,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
) -> bool {
    cfg_state(syntax, unit, semantics) == CfgState::Active
}

fn cfg_state(
    syntax: &ra_ap_syntax::SyntaxNode,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
) -> CfgState {
    let attributes = syntax
        .ancestors()
        .filter_map(ast::Item::cast)
        .flat_map(|item| item.attrs())
        .filter_map(|attribute| attribute.meta())
        .collect::<Vec<_>>();
    if attributes
        .iter()
        .all(|meta| !matches!(meta, ast::Meta::CfgMeta(_) | ast::Meta::CfgAttrMeta(_)))
    {
        return CfgState::Active;
    }
    let Some((workspace, module)) = semantics.zip(unit.hir_module) else {
        return CfgState::Unknown;
    };
    let options = module.krate(&workspace.database).cfg(&workspace.database);
    attributes
        .into_iter()
        .fold(CfgState::Active, |state, meta| {
            combine_cfg_state(state, cfg_meta_state(&meta, options))
        })
}

fn cfg_meta_state(meta: &ast::Meta, options: &ra_ap_cfg::CfgOptions) -> CfgState {
    match meta {
        ast::Meta::CfgMeta(cfg) => cfg
            .cfg_predicate()
            .map(CfgExpr::parse_from_ast)
            .and_then(|expression| options.check(&expression))
            .map_or(CfgState::Unknown, |active| {
                if active {
                    CfgState::Active
                } else {
                    CfgState::Inactive
                }
            }),
        ast::Meta::CfgAttrMeta(cfg_attr) => match cfg_attr
            .cfg_predicate()
            .map(CfgExpr::parse_from_ast)
            .and_then(|expression| options.check(&expression))
        {
            Some(false) => CfgState::Active,
            Some(true) => cfg_attr.metas().fold(CfgState::Active, |state, inner| {
                combine_cfg_state(state, cfg_meta_state(&inner, options))
            }),
            None => CfgState::Unknown,
        },
        _ => CfgState::Active,
    }
}

fn combine_cfg_state(left: CfgState, right: CfgState) -> CfgState {
    match (left, right) {
        (CfgState::Inactive, _) | (_, CfgState::Inactive) => CfgState::Inactive,
        (CfgState::Unknown, _) | (_, CfgState::Unknown) => CfgState::Unknown,
        (CfgState::Active, CfgState::Active) => CfgState::Active,
    }
}

fn belongs_to_function(syntax: &ra_ap_syntax::SyntaxNode, function: &ast::Fn) -> bool {
    syntax
        .ancestors()
        .find_map(ast::Fn::cast)
        .is_some_and(|owner| owner.syntax() == function.syntax())
}

fn has_test_attribute(syntax: &ra_ap_syntax::SyntaxNode) -> bool {
    syntax
        .children()
        .filter_map(ast::Attr::cast)
        .any(|attribute| {
            let text = attribute.syntax().text().to_string();
            text == "#[test]" || text.starts_with("#[cfg(test")
        })
}

fn line_range(text: &str, syntax: &ra_ap_syntax::SyntaxNode) -> (u32, u32) {
    let range = syntax.text_range();
    (
        line_number(text, u32::from(range.start()) as usize),
        line_number(text, u32::from(range.end()) as usize),
    )
}

fn line_number(text: &str, offset: usize) -> u32 {
    1 + u32::try_from(
        text.as_bytes()
            .iter()
            .take(offset.min(text.len()))
            .filter(|byte| **byte == b'\n')
            .count(),
    )
    .unwrap_or(u32::MAX - 1)
}

fn relative_slash(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .starts_with(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
}

fn include_entry(entry: &DirEntry) -> bool {
    !entry
        .file_name()
        .to_str()
        .is_some_and(|name| matches!(name, ".git" | "target" | ".graphine"))
}

fn project_diagnostic(kind: &str, reason: &str) -> AnalyzerDiagnostic {
    AnalyzerDiagnostic {
        kind: kind.to_owned(),
        file_path: None,
        start_line: None,
        end_line: None,
        symbol_text: None,
        reason: reason.to_owned(),
        severity: "warning".to_owned(),
    }
}

fn symbol_diagnostic(
    unit: &SourceUnit,
    syntax: &ra_ap_syntax::SyntaxNode,
    kind: &str,
    symbol: &str,
    reason: &str,
) -> AnalyzerDiagnostic {
    let (start_line, end_line) = line_range(&unit.text, syntax);
    AnalyzerDiagnostic {
        kind: kind.to_owned(),
        file_path: Some(unit.relative_path.clone()),
        start_line: Some(start_line),
        end_line: Some(end_line),
        symbol_text: Some(symbol.to_owned()),
        reason: reason.to_owned(),
        severity: "warning".to_owned(),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphine_protocol::{ANALYZER_PROTOCOL_VERSION, AnalyzerOptions, CargoAnalysisOptions};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "graphine-rust-worker-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let build_sentinel = root.join("build-script-ran");
        let proc_sentinel = root.join("procedural-macro-ran");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("local-macro/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"worker-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\nbuild=\"build.rs\"\n[features]\ndefault=[]\nalpha=[]\n[dependencies]\nlocal-macro={path=\"local-macro\"}\n",
        )
        .unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"local-macro\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"worker-fixture\"\nversion = \"0.1.0\"\ndependencies = [\n \"local-macro\",\n]\n",
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub trait Named { fn name(&self) -> &'static str; }\npub struct Item { pub value: u32 }\nimpl Named for Item { fn name(&self) -> &'static str { \"item\" } }\npub fn make() -> Item { Item { value: 1 } }\n#[local_macro::tag]\npub fn attributed() {}\n#[cfg(feature = \"alpha\")]\npub fn feature_only() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("build.rs"),
            format!(
                "fn main() {{ std::fs::write(r#\"{}\"#, \"executed\").unwrap(); }}\n",
                build_sentinel.display()
            ),
        )
        .unwrap();
        fs::write(
            root.join("local-macro/Cargo.toml"),
            "[package]\nname=\"local-macro\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[lib]\nproc-macro=true\n",
        )
        .unwrap();
        fs::write(
            root.join("local-macro/src/lib.rs"),
            format!(
                "use proc_macro::TokenStream;\n#[proc_macro_attribute]\npub fn tag(_: TokenStream, item: TokenStream) -> TokenStream {{ std::fs::write(r#\"{}\"#, \"executed\").unwrap(); item }}\n",
                proc_sentinel.display()
            ),
        )
        .unwrap();
        (root, build_sentinel, proc_sentinel)
    }

    fn request(root: PathBuf, mode: AnalyzerMode) -> AnalyzeProjectRequest {
        AnalyzeProjectRequest {
            protocol_version: ANALYZER_PROTOCOL_VERSION,
            request_id: "worker-test".to_owned(),
            operation: "analyze_project".to_owned(),
            language: ProjectLanguage::Rust,
            project_root: root,
            mode,
            source_sets: vec!["main".to_owned(), "test".to_owned()],
            options: AnalyzerOptions::default(),
            maven_executable: PathBuf::from("mvn"),
            cargo_executable: PathBuf::from(if cfg!(windows) { "cargo.exe" } else { "cargo" }),
            rustc_executable: PathBuf::from(if cfg!(windows) { "rustc.exe" } else { "rustc" }),
            cargo: CargoAnalysisOptions::default(),
            timeout_ms: 120_000,
        }
    }

    fn semantic_identity_fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "graphine-rust-identities-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::create_dir_all(root.join("external/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers=[\"app\",\"external\"]\nresolver=\"3\"\n",
        )
        .unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"external\",\n]\n\n[[package]]\nname = \"external\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("app/Cargo.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[dependencies]\nexternal={path=\"../external\"}\n",
        )
        .unwrap();
        fs::write(
            root.join("external/Cargo.toml"),
            "[package]\nname=\"external\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        fs::write(
            root.join("external/src/lib.rs"),
            "pub fn shared() -> u32 { 2 }\n",
        )
        .unwrap();
        fs::write(
            root.join("app/src/common.rs"),
            "pub fn common_target() -> u32 { 3 }\n",
        )
        .unwrap();
        fs::write(
            root.join("app/src/lib.rs"),
            r#"pub mod common;
pub fn shared() -> u32 { 1 }
pub struct Alpha;
pub struct Beta;
impl Alpha { pub fn same(&self) -> u32 { 10 } }
impl Beta { pub fn same(&self) -> u32 { 20 } }
pub fn run(alpha: &Alpha, beta: &Beta) -> u32 {
    shared() + external::shared() + alpha.same() + beta.same() + common::common_target()
}
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn known_platform() {}
#[cfg(all(target_os = "graphine-never", any(unix, windows)))]
pub fn complex_inactive() {}
#[cfg_attr(any(unix, windows), cfg(all(target_os = "graphine-never", unix)))]
pub fn cfg_attr_inactive() {}
#[cfg(graphine_unknown_operator(unix))]
pub fn uncertain_cfg() {}
"#,
        )
        .unwrap();
        fs::write(
            root.join("app/src/main.rs"),
            "mod common;\nfn main() { let _ = common::common_target(); }\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn safe_mode_does_not_execute_build_scripts_and_feature_changes_fingerprint() {
        let (root, build_sentinel, proc_sentinel) = fixture();
        let base = analyze(&request(root.clone(), AnalyzerMode::Safe), "test").unwrap();
        assert!(!build_sentinel.exists());
        assert!(!proc_sentinel.exists());
        assert!(base.nodes.iter().any(|node| node.kind == "trait"));
        assert!(
            !base
                .nodes
                .iter()
                .any(|node| node.simple_name.as_deref() == Some("feature_only"))
        );
        let mut featured = request(root.clone(), AnalyzerMode::Safe);
        featured.cargo.features.push("alpha".to_owned());
        let featured = analyze(&featured, "test").unwrap();
        assert!(!build_sentinel.exists());
        assert!(!proc_sentinel.exists());
        assert!(
            featured
                .nodes
                .iter()
                .any(|node| node.simple_name.as_deref() == Some("feature_only"))
        );
        assert_ne!(base.fingerprint, featured.fingerprint);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trusted_mode_executes_build_scripts_only_when_explicitly_selected() {
        let (root, build_sentinel, proc_sentinel) = fixture();
        let output = analyze(&request(root.clone(), AnalyzerMode::Trusted), "test").unwrap();
        assert!(build_sentinel.exists());
        assert!(
            proc_sentinel.exists(),
            "trusted resources: {:?}",
            output.summary.resources
        );
        assert!(
            output
                .summary
                .capabilities
                .get("cargo_trusted_mode")
                .copied()
                .unwrap_or(false)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safe_mode_stays_offline_and_falls_back_when_dependencies_are_unavailable() {
        let (root, build_sentinel, proc_sentinel) = fixture();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"worker-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\nbuild=\"build.rs\"\n[dependencies]\ndefinitely-unavailable-graphine-fixture={version=\"999.0.0\"}\n",
        )
        .unwrap();
        fs::remove_file(root.join("Cargo.lock")).unwrap();
        let output = analyze(&request(root.clone(), AnalyzerMode::Safe), "test").unwrap();
        assert!(!build_sentinel.exists());
        assert!(!proc_sentinel.exists());
        assert!(output.nodes.iter().any(|node| node.kind == "type"));
        assert!(output.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.kind.as_str(),
                "cargo_metadata_unavailable" | "semantic_workspace_unavailable"
            )
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compiler_resolved_edges_use_exact_hir_identity_for_duplicate_names() {
        let root = semantic_identity_fixture();
        let output = analyze(&request(root.clone(), AnalyzerMode::Safe), "test").unwrap();
        let source = "function:crate:app/lib/app::crate::run()";
        let expected_targets = [
            "function:crate:app/lib/app::crate::shared()",
            "function:crate:external/lib/external::crate::shared()",
            "method:crate:app/lib/app::crate::Alpha#same()",
            "method:crate:app/lib/app::crate::Beta#same()",
        ];
        for target in expected_targets {
            assert!(
                output.edges.iter().any(|edge| {
                    edge.source_stable_id == source
                        && edge.target_stable_id == target
                        && edge.kind == "CALLS"
                        && edge.confidence == Confidence::CompilerResolved
                }),
                "missing exact compiler-resolved edge to {target}; external nodes={:?}; shared nodes={:?}; call edges={:?}; diagnostics={:?}",
                output
                    .nodes
                    .iter()
                    .filter(|node| node.stable_id.contains("external"))
                    .map(|node| (&node.stable_id, node.confidence))
                    .collect::<Vec<_>>(),
                output
                    .nodes
                    .iter()
                    .filter(|node| node.simple_name.as_deref() == Some("shared"))
                    .map(|node| (&node.stable_id, node.confidence))
                    .collect::<Vec<_>>(),
                output
                    .edges
                    .iter()
                    .filter(|edge| edge.source_stable_id == source)
                    .map(|edge| (&edge.target_stable_id, &edge.kind, edge.confidence))
                    .collect::<Vec<_>>(),
                output
                    .diagnostics
                    .iter()
                    .map(|diagnostic| (&diagnostic.kind, &diagnostic.symbol_text))
                    .collect::<Vec<_>>()
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crate_graph_assigns_shared_module_to_library_and_binary_targets() {
        let root = semantic_identity_fixture();
        let output = analyze(&request(root.clone(), AnalyzerMode::Safe), "test").unwrap();
        for target in ["crate:app/lib/app", "crate:app/bin/app"] {
            let stable_id = format!("function:{target}::common::common_target()");
            assert!(
                output.nodes.iter().any(|node| node.stable_id == stable_id),
                "shared module was not indexed for {target}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rust_analyzer_cfg_handles_composites_cfg_attr_and_uncertainty() {
        let root = semantic_identity_fixture();
        let output = analyze(&request(root.clone(), AnalyzerMode::Safe), "test").unwrap();
        assert!(
            output
                .nodes
                .iter()
                .any(|node| { node.simple_name.as_deref() == Some("known_platform") })
        );
        for excluded in ["complex_inactive", "cfg_attr_inactive", "uncertain_cfg"] {
            assert!(
                !output
                    .nodes
                    .iter()
                    .any(|node| { node.simple_name.as_deref() == Some(excluded) })
            );
        }
        assert!(output.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == "cfg_configuration_unknown"
                && diagnostic.symbol_text.as_deref() == Some("cfg")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trusted_cargo_timeout_terminates_long_running_build_script() {
        let root = std::env::temp_dir().join(format!(
            "graphine-rust-timeout-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let sentinel = root.join("late-build-script-output");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"timeout-fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\nbuild=\"build.rs\"\n",
        )
        .unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"timeout-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn value() -> u32 { 1 }\n").unwrap();
        fs::write(
            root.join("build.rs"),
            format!(
                "fn main() {{ std::thread::sleep(std::time::Duration::from_secs(2)); std::fs::write(r#\"{}\"#, \"survived\").unwrap(); }}\n",
                sentinel.display()
            ),
        )
        .unwrap();
        let mut timed = request(root.clone(), AnalyzerMode::Trusted);
        timed.timeout_ms = 400;
        let started = Instant::now();
        let output = analyze(&timed, "test").unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "trusted_cargo_check_failed")
        );
        thread::sleep(Duration::from_millis(2_200));
        assert!(
            !sentinel.exists(),
            "timed-out build script survived Cargo termination"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
