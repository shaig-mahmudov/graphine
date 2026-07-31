use anyhow::{Context, Result, bail};
use cargo_metadata::{CargoOpt, MetadataCommand, PackageId};
use graphine_protocol::{
    AnalyzeProjectRequest, AnalyzerDiagnostic, AnalyzerMode, AnalyzerSummary, Confidence,
    EdgeOccurrence, ProjectLanguage, SyntheticEdge, SyntheticNode,
};
use ra_ap_hir::{ModuleDef, PathResolution, Semantics};
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_proc_macro_api::ProcMacroClient;
use ra_ap_project_model::{CargoConfig, CargoFeatures, TargetDirectoryConfig};
use ra_ap_syntax::{
    AstNode, Edition, SourceFile,
    ast::{self, HasAttrs, HasName},
};
use ra_ap_vfs::{Vfs, VfsPath};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
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

    let units = discover_sources(&root, &targets, request.options.include_tests)?;
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
    if has_excluded_cfg_items(&units, request) {
        diagnostics.push(project_diagnostic(
            "cfg_branch_excluded",
            "one or more declarations were excluded by the selected Cargo features or test configuration",
        ));
    }
    let trusted_check_started = Instant::now();
    if request.mode == AnalyzerMode::Trusted
        && let Err(error) = run_trusted_cargo_check(&root, request)
    {
        diagnostics.push(project_diagnostic(
            "trusted_cargo_check_failed",
            &format!(
                "trusted Cargo check did not complete; semantic recovery will continue: {error:#}"
            ),
        ));
    }
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
        match load_semantics(&root, request) {
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
    if request.mode == AnalyzerMode::Trusted
        && semantics
            .as_ref()
            .is_some_and(|workspace| workspace.proc_macro_client.is_none())
    {
        diagnostics.push(project_diagnostic(
            "procedural_macro_server_unavailable",
            "trusted Cargo check executed procedural macros, but rust-analyzer could not retain a procedural-macro server for semantic expansion",
        ));
    }
    let trusted_proc_macro_expansions = if request.mode == AnalyzerMode::Trusted
        && let Some(workspace) = semantics.as_ref()
    {
        ra_ap_hir::attach_db(&workspace.database, || {
            expand_trusted_attribute_macros(&units, workspace)
        })
    } else {
        0
    };
    let parsing_started = Instant::now();

    let mut graph = GraphBuilder::default();
    emit_crates_and_modules(&mut graph, &targets, &units);
    for unit in &units {
        collect_type_definitions(&mut graph, unit, semantics.as_ref(), request);
    }
    for unit in &units {
        collect_callable_definitions(&mut graph, unit, semantics.as_ref(), request);
    }
    for unit in &units {
        if let Some(workspace) = semantics.as_ref() {
            ra_ap_hir::attach_db(&workspace.database, || {
                collect_relationships(&mut graph, unit, semantics.as_ref(), request);
            });
        } else {
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
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
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
    let status = command
        .status()
        .context("failed to start trusted Cargo check")?;
    if !status.success() {
        bail!("trusted Cargo check exited with {status}");
    }
    Ok(())
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

fn load_semantics(root: &Path, request: &AnalyzeProjectRequest) -> Result<SemanticWorkspace> {
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
        load_out_dirs_from_check: request.mode == AnalyzerMode::Trusted,
        with_proc_macro_server: if request.mode == AnalyzerMode::Trusted {
            ProcMacroServerChoice::Sysroot
        } else {
            ProcMacroServerChoice::None
        },
        prefill_caches: false,
        num_worker_threads: 1,
        proc_macro_processes: 1,
    };
    let (database, vfs, proc_macro_client) = load_workspace_at(root, &cargo, &load, &|_| {})?;
    Ok(SemanticWorkspace {
        database,
        vfs,
        proc_macro_client,
    })
}

fn expand_trusted_attribute_macros(units: &[SourceUnit], workspace: &SemanticWorkspace) -> u64 {
    let sema = Semantics::new(&workspace.database);
    let mut expanded = 0_u64;
    for unit in units {
        let path = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        let vfs_path = VfsPath::new_real_path(path.to_string_lossy().into_owned());
        let Some((file_id, _)) = workspace.vfs.file_id(&vfs_path) else {
            continue;
        };
        let file = sema.parse_guess_edition(file_id);
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
    request: &AnalyzeProjectRequest,
) {
    let (file, semantic) = parse_unit(unit, semantics);
    let root_module = module_id(&unit.target.crate_key, &unit.module_path);
    for strukt in file
        .syntax()
        .descendants()
        .filter_map(ast::Struct::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &strukt, "type", &root_module, semantic);
        if let Some(fields) = strukt.field_list() {
            collect_fields(graph, unit, &strukt, &fields, semantic);
        }
    }
    for union in file
        .syntax()
        .descendants()
        .filter_map(ast::Union::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &union, "type", &root_module, semantic);
        if let Some(fields) = union.record_field_list() {
            for field in fields.fields() {
                add_field(graph, unit, &union, &field, semantic);
            }
        }
    }
    for enumeration in file
        .syntax()
        .descendants()
        .filter_map(ast::Enum::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &enumeration, "type", &root_module, semantic);
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
                    semantic,
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
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &trait_node, "trait", &root_module, semantic);
    }
    for alias in file
        .syntax()
        .descendants()
        .filter_map(ast::TypeAlias::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
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
                semantic,
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
            add_named_type(graph, unit, &alias, "type", &root_module, semantic);
        }
    }
    for constant in file
        .syntax()
        .descendants()
        .filter_map(ast::Const::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &constant, "const", &root_module, semantic);
    }
    for static_node in file
        .syntax()
        .descendants()
        .filter_map(ast::Static::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &static_node, "static", &root_module, semantic);
    }
    for macro_node in file
        .syntax()
        .descendants()
        .filter_map(ast::MacroRules::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &macro_node, "macro", &root_module, semantic);
    }
    for macro_node in file
        .syntax()
        .descendants()
        .filter_map(ast::MacroDef::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        add_named_type(graph, unit, &macro_node, "macro", &root_module, semantic);
    }
}

fn collect_callable_definitions(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
    request: &AnalyzeProjectRequest,
) {
    let sema = semantics.map(|workspace| Semantics::new(&workspace.database));
    let (file, semantic) = if let (Some(workspace), Some(sema)) = (semantics, sema.as_ref()) {
        let path = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        let vfs_path = VfsPath::new_real_path(path.to_string_lossy().into_owned());
        if let Some((file_id, _)) = workspace.vfs.file_id(&vfs_path) {
            (sema.parse_guess_edition(file_id), true)
        } else {
            (
                SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
                false,
            )
        }
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
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        let Some(name) = function.name().map(|name| name.text().to_string()) else {
            continue;
        };
        let namespace = namespace_for(unit, function.syntax());
        let function_resolved = semantic
            && sema
                .as_ref()
                .and_then(|sema| sema.to_def(&function))
                .is_some();
        let (kind, stable_id, owner, trait_owner) =
            callable_identity(graph, unit, &function, &namespace, &name);
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
            let prefix = format!(
                "method:{}#",
                trait_id.strip_prefix("trait:").unwrap_or(&trait_id)
            );
            let trait_method = graph
                .unique_named(&name, Some("method"))
                .find(|id| id.starts_with(&prefix));
            if let Some(trait_method) = trait_method {
                graph.add_edge(
                    &stable_id,
                    &trait_method,
                    "IMPLEMENTS_METHOD",
                    if function_resolved {
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

#[allow(clippy::too_many_lines)]
fn collect_relationships(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    semantics: Option<&SemanticWorkspace>,
    request: &AnalyzeProjectRequest,
) {
    let sema = semantics.map(|workspace| Semantics::new(&workspace.database));
    let (file, semantic) = if let (Some(workspace), Some(sema)) = (semantics, sema.as_ref()) {
        let path = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        let vfs_path = VfsPath::new_real_path(path.to_string_lossy().into_owned());
        if let Some((file_id, _)) = workspace.vfs.file_id(&vfs_path) {
            (sema.parse_guess_edition(file_id), true)
        } else {
            (
                SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
                false,
            )
        }
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
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        let implementation_resolved = semantic
            && sema
                .as_ref()
                .and_then(|sema| sema.to_def(&implementation))
                .is_some();
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
                if implementation_resolved {
                    Confidence::CompilerResolved
                } else {
                    Confidence::StaticInferred
                },
                unit,
                implementation.syntax(),
                json!({"unsafe":implementation.unsafe_token().is_some()}),
            );
        }
    }

    for function in file
        .syntax()
        .descendants()
        .filter_map(ast::Fn::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
    {
        let Some(name) = function.name().map(|name| name.text().to_string()) else {
            continue;
        };
        let namespace = namespace_for(unit, function.syntax());
        let (_, source_id, _, _) = callable_identity(graph, unit, &function, &namespace, &name);
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
            let resolved_semantically = semantic
                && sema
                    .as_ref()
                    .and_then(|sema| sema.resolve_path(&path))
                    .is_some_and(|resolution| {
                        matches!(
                            resolution,
                            PathResolution::Def(
                                ModuleDef::Function(_)
                                    | ModuleDef::Adt(_)
                                    | ModuleDef::EnumVariant(_)
                            )
                        )
                    });
            let target = graph
                .unique_named(&target_name, Some("function"))
                .chain(graph.unique_named(&target_name, Some("method")))
                .next();
            if let Some(target) = target {
                let kind = if graph
                    .nodes
                    .get(&target)
                    .is_some_and(|node| matches!(node.kind.as_str(), "type" | "variant"))
                {
                    "CONSTRUCTS"
                } else {
                    "CALLS"
                };
                graph.add_resolved_edge(
                    &source_id,
                    &target,
                    kind,
                    resolved_semantically,
                    unit,
                    call.syntax(),
                );
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
            let resolved_semantically = semantic
                && sema
                    .as_ref()
                    .and_then(|sema| sema.resolve_method_call(&call))
                    .is_some();
            let candidates = graph
                .unique_named(&target_name, Some("method"))
                .collect::<Vec<_>>();
            let signature = function_signature(&function);
            if candidates.len() == 1 {
                graph.add_resolved_edge(
                    &source_id,
                    &candidates[0],
                    "CALLS",
                    resolved_semantically,
                    unit,
                    call.syntax(),
                );
            } else if resolved_semantically
                && let Some(concrete_method) = candidates.iter().find(|candidate| {
                    graph
                        .nodes
                        .get(*candidate)
                        .and_then(|node| node.metadata.get("owner"))
                        .and_then(Value::as_str)
                        .and_then(|owner| graph.nodes.get(owner))
                        .filter(|owner| owner.kind == "type")
                        .and_then(|owner| owner.simple_name.as_deref())
                        .is_some_and(|type_name| signature.contains(type_name))
                })
            {
                graph.bindings_resolved += 1;
                graph.add_edge(
                    &source_id,
                    concrete_method,
                    "CALLS",
                    Confidence::CompilerResolved,
                    unit,
                    call.syntax(),
                    json!({
                        "dispatch":"static",
                        "runtime_implementation_inferred":true,
                    }),
                );
            } else if resolved_semantically
                && let Some(trait_method) = candidates.iter().find(|candidate| {
                    graph
                        .nodes
                        .get(*candidate)
                        .and_then(|node| node.metadata.get("owner"))
                        .and_then(Value::as_str)
                        .and_then(|owner| graph.nodes.get(owner))
                        .is_some_and(|owner| owner.kind == "trait")
                })
            {
                graph.bindings_resolved += 1;
                graph.add_edge(
                    &source_id,
                    trait_method,
                    "CALLS",
                    Confidence::CompilerResolved,
                    unit,
                    call.syntax(),
                    json!({
                        "dispatch": if signature.contains("dyn ") {
                            "dynamic_trait"
                        } else {
                            "trait"
                        },
                        "runtime_implementation_inferred":false,
                    }),
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
            let resolved_semantically = semantic
                && record_path
                    .as_ref()
                    .and_then(|path| sema.as_ref().and_then(|sema| sema.resolve_path(path)))
                    .is_some_and(|resolution| {
                        matches!(resolution, PathResolution::Def(ModuleDef::Adt(_)))
                    });
            if let Some(type_name) =
                record_path.and_then(|path| final_name(&path.syntax().to_string()))
                && let Some(type_id) = graph.first_named(&type_name, Some("type"))
            {
                graph.add_resolved_edge(
                    &source_id,
                    &type_id,
                    "CONSTRUCTS",
                    resolved_semantically,
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
            if let Some(macro_id) = graph.first_named(&macro_name, Some("macro")) {
                graph.add_resolved_edge(
                    &source_id,
                    &macro_id,
                    "INVOKES_MACRO",
                    expansion.is_some(),
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
                    let resolved = sema.resolve_path(&path).is_some_and(|resolution| {
                        matches!(resolution, PathResolution::Def(ModuleDef::Function(_)))
                    });
                    let Some(target) = graph.first_named(&target_name, Some("function")) else {
                        continue;
                    };
                    if resolved {
                        graph.bindings_resolved += 1;
                    } else {
                        graph.bindings_unresolved += 1;
                    }
                    graph.add_edge(
                        &source_id,
                        &target,
                        "CALLS",
                        if resolved {
                            Confidence::CompilerResolved
                        } else {
                            Confidence::StaticInferred
                        },
                        unit,
                        macro_call.syntax(),
                        json!({"inside_macro":macro_name}),
                    );
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
                if candidates.len() == 1 {
                    let resolved_semantically = sema
                        .as_ref()
                        .and_then(|sema| sema.resolve_field(&field))
                        .is_some();
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
                        &candidates[0],
                        if write { "WRITES_FIELD" } else { "READS_FIELD" },
                        resolved_semantically,
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
                            &candidates[0],
                            "READS_FIELD",
                            resolved_semantically,
                            unit,
                            field.syntax(),
                        );
                    }
                }
            }
        }
    }

    for use_item in file
        .syntax()
        .descendants()
        .filter_map(ast::Use::cast)
        .filter(|item| cfg_enabled(item.syntax(), request))
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
            let resolved_semantically = semantic
                && imported_path
                    .as_ref()
                    .and_then(|path| sema.as_ref().and_then(|sema| sema.resolve_path(path)))
                    .is_some();
            let Some(name) = imported_path.and_then(|path| final_name(&path.syntax().to_string()))
            else {
                continue;
            };
            if let Some(target) = graph.first_named(&name, None) {
                graph.add_resolved_edge(
                    &source,
                    &target,
                    "IMPORTS",
                    resolved_semantically,
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
    semantic: bool,
) {
    let Some(name) = node.name().map(|name| name.text().to_string()) else {
        return;
    };
    let namespace = namespace_for(unit, node.syntax());
    let stable_id = stable_for(kind, unit, &namespace, &name);
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
    semantic: bool,
) {
    if let ast::FieldList::RecordFieldList(fields) = fields {
        for field in fields.fields() {
            add_field(graph, unit, owner, &field, semantic);
        }
    }
}

fn add_field<N: AstNode + HasName>(
    graph: &mut GraphBuilder,
    unit: &SourceUnit,
    owner: &N,
    field: &ast::RecordField,
    semantic: bool,
) {
    let (Some(owner_name), Some(field_name)) = (
        owner.name().map(|name| name.text().to_string()),
        field.name().map(|name| name.text().to_string()),
    ) else {
        return;
    };
    let namespace = namespace_for(unit, owner.syntax());
    let owner_id = stable_for("type", unit, &namespace, &owner_name);
    let stable_id = format!(
        "field:{}#{}",
        owner_id.strip_prefix("type:").unwrap_or(&owner_id),
        field_name
    );
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
        let path = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        let vfs_path = VfsPath::new_real_path(path.to_string_lossy().into_owned());
        if let Some((file_id, _)) = workspace.vfs.file_id(&vfs_path) {
            let sema = Semantics::new(&workspace.database);
            return (sema.parse_guess_edition(file_id), true);
        }
    }
    (
        SourceFile::parse(&unit.text, Edition::CURRENT).tree(),
        false,
    )
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

fn has_excluded_cfg_items(units: &[SourceUnit], request: &AnalyzeProjectRequest) -> bool {
    units.iter().any(|unit| {
        SourceFile::parse(&unit.text, Edition::CURRENT)
            .tree()
            .syntax()
            .descendants()
            .filter_map(ast::Item::cast)
            .any(|item| !cfg_enabled(item.syntax(), request))
    })
}

fn cfg_enabled(syntax: &ra_ap_syntax::SyntaxNode, request: &AnalyzeProjectRequest) -> bool {
    syntax
        .ancestors()
        .filter_map(ast::Item::cast)
        .flat_map(|item| item.attrs())
        .all(|attribute| {
            cfg_attribute_enabled(&attribute.syntax().to_string(), request).unwrap_or(true)
        })
}

fn cfg_attribute_enabled(text: &str, request: &AnalyzeProjectRequest) -> Option<bool> {
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    if !compact.starts_with("#[cfg(") {
        return None;
    }
    if compact.starts_with("#[cfg(test") {
        return Some(request.options.include_tests);
    }
    let marker = "feature=\"";
    let feature_start = compact.find(marker)? + marker.len();
    let feature_end = compact[feature_start..].find('"')? + feature_start;
    let feature = &compact[feature_start..feature_end];
    let selected = request.cargo.all_features
        || request
            .cargo
            .features
            .iter()
            .any(|selected| selected == feature);
    Some(if compact.starts_with("#[cfg(not(") {
        !selected
    } else {
        selected
    })
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
}
