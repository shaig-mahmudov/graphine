use graphine_index::{Database, EdgeRecord, EvidenceLocation, IndexStatus, NodeRecord};
use graphine_protocol::{
    Budget, Completeness, DetailLevel, Direction, EvidenceRef, GraphineConfig, GraphineError,
    Pagination, ResponseEnvelope, StableId, TruncationState, UncertaintyState,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::{BTreeSet, VecDeque};
use std::time::Instant;
use tracing::{debug, instrument};

mod phase4;
pub use phase4::{EndpointContextRequest, EvidenceRequestRef, GetEvidenceRequest};
use phase4::{compact_agent_result, evidence_id};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMapRequest {
    pub project: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSymbolRequest {
    pub project: String,
    pub query: String,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub framework_roles: Vec<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub package_prefix: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub detail: DetailLevel,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolContextRequest {
    pub project: String,
    pub stable_id: String,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub detail: DetailLevel,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceFlowRequest {
    pub project: String,
    pub start_stable_id: String,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub edge_kinds: Vec<String>,
    #[serde(default)]
    pub edge_groups: Vec<String>,
    #[serde(default)]
    pub node_kinds: Vec<String>,
    #[serde(default)]
    pub application_only: bool,
    #[serde(default = "default_true")]
    pub suppress_external: bool,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub max_nodes: Option<u32>,
    #[serde(default)]
    pub max_paths: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexStatusRequest {
    pub project: String,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

pub struct QueryService<'a> {
    database: &'a Database,
    config: &'a GraphineConfig,
}

impl<'a> QueryService<'a> {
    #[must_use]
    pub const fn new(database: &'a Database, config: &'a GraphineConfig) -> Self {
        Self { database, config }
    }

    /// Returns active graph counts and compact module/package summaries.
    ///
    /// # Errors
    ///
    /// Returns project, index, budget, or database errors.
    #[instrument(skip(self, request), fields(project = request.project))]
    pub fn get_project_map(
        &self,
        request: &ProjectMapRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let started = Instant::now();
        let budget = self.budget(request.token_budget)?;
        let graph = self.database.graph_summary(&request.project)?;
        if request
            .scope
            .as_deref()
            .is_some_and(|scope| scope != "application")
        {
            return Err(GraphineError::InvalidArgument(
                "scope must be application".to_owned(),
            ));
        }
        let counts = self.database.framework_counts(&graph.status)?;
        let major_areas: Vec<_> = self
            .database
            .package_metrics(&graph.status, 12)?
            .into_iter()
            .filter(|area| {
                !area.package.starts_with("java.")
                    && !area.package.starts_with("javax.")
                    && !area.package.starts_with("jakarta.")
                    && !area.package.starts_with("org.springframework.")
                    && (area.framework_components > 0 || area.internal_edges > 0)
            })
            .map(|area| {
                json!({
                    "package": area.package,
                    "roles": area.roles,
                    "framework_components": area.framework_components,
                    "internal_edges": area.internal_edges,
                    "routes": area.routes,
                    "repositories": area.repositories,
                    "entities": area.entities,
                })
            })
            .collect();
        let key_application_roots = major_areas
            .iter()
            .take(3)
            .filter_map(|area| area.get("package"))
            .cloned()
            .collect::<Vec<_>>();
        let mut result_value = json!({
            "summary": {
                "modules": graph.modules.len(),
                "controllers": counts.get("controllers").copied().unwrap_or(0),
                "services": counts.get("services").copied().unwrap_or(0),
                "repositories": counts.get("repositories").copied().unwrap_or(0),
                "entities": counts.get("entities").copied().unwrap_or(0),
                "routes": counts.get("routes").copied().unwrap_or(0),
                "beans": counts.get("beans").copied().unwrap_or(0),
                "unresolved_diagnostics": graph.unresolved_count,
            },
            "modules": graph.modules,
            "major_areas": major_areas,
            "active_generation": graph.status.active_generation,
            "stale": graph.status.stale,
            "partial": graph.status.partial,
            "key_application_roots": key_application_roots,
        });
        let envelope = compact_agent_result(
            &graph.status,
            &mut result_value,
            Vec::new(),
            Vec::new(),
            vec![json!({
                "tool": "search_symbol",
                "reason": "Narrow to a symbol or route in a major application area",
                "arguments": {"project": request.project, "query": "<name or route>"}
            })],
            Pagination::default(),
            budget,
            "application orientation".to_owned(),
            &["major_areas", "modules", "key_application_roots"],
        )?;
        debug!(
            duration_ms = started.elapsed().as_millis(),
            estimated_tokens = envelope.budget.estimated_tokens,
            truncated = envelope.budget.truncated,
            "project map complete"
        );
        Ok(envelope)
    }

    /// Performs deterministic bounded lexical search over the active generation.
    ///
    /// # Errors
    ///
    /// Returns validation, project, index, cursor, or database errors.
    #[instrument(skip(self, request), fields(project = request.project, query = request.query))]
    #[allow(clippy::missing_panics_doc, clippy::too_many_lines)]
    pub fn search_symbol(
        &self,
        request: &SearchSymbolRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        if request.query.trim().is_empty() || request.query.len() > 500 {
            return Err(GraphineError::InvalidArgument(
                "query must be non-empty".to_owned(),
            ));
        }
        let budget = self.budget(request.token_budget)?;
        let limit = request
            .limit
            .unwrap_or(10)
            .min(self.config.maximum_result_count);
        if limit == 0 {
            return Err(GraphineError::InvalidArgument(
                "limit must be positive".to_owned(),
            ));
        }
        let fetch_limit = self
            .config
            .maximum_result_count
            .saturating_mul(10)
            .max(limit);
        let (status, mut candidates) = self.database.search_candidates(
            &request.project,
            request.query.trim(),
            &request.kinds,
            fetch_limit,
        )?;
        let binding = CursorBinding::new(
            &status,
            "search_symbol",
            None,
            format!(
                "query={};kinds={:?};roles={:?};module={:?};package={:?};detail={:?}",
                request.query.trim(),
                request.kinds,
                request.framework_roles,
                request.module,
                request.package_prefix,
                request.detail
            ),
        );
        let offset = decode_cursor(request.cursor.as_deref(), &binding)?;
        candidates.retain(|node| lexical_rank(request.query.trim(), node) > 0);
        candidates.retain(|node| {
            request
                .module
                .as_ref()
                .is_none_or(|module| node.module_name.as_deref() == Some(module.as_str()))
                && request.package_prefix.as_ref().is_none_or(|prefix| {
                    node.package_name
                        .as_deref()
                        .is_some_and(|package| package.starts_with(prefix))
                })
                && (request.framework_roles.is_empty()
                    || resolved_framework_role(self.database, &status, node).is_some_and(|role| {
                        request
                            .framework_roles
                            .iter()
                            .any(|requested| requested.eq_ignore_ascii_case(&role))
                    }))
        });
        candidates.sort_by_key(|node| {
            (
                Reverse(agent_search_rank(request, node)),
                node.qualified_name.to_ascii_lowercase(),
                node.stable_id.clone(),
            )
        });
        let total = candidates.len();
        let results: Vec<_> = candidates
            .into_iter()
            .skip(offset)
            .take(limit as usize)
            .map(|node| {
                let mut fact = search_result_fact(&status, &node, request.detail);
                fact.as_object_mut().expect("object literal").insert(
                    "framework_role".to_owned(),
                    json!(resolved_framework_role(self.database, &status, &node)),
                );
                fact
            })
            .collect();
        let mut result_value = json!({
            "query": request.query,
            "matched": total,
            "results": results,
        });
        let page_has_more =
            offset.saturating_add(result_value["results"].as_array().map_or(0, Vec::len)) < total;
        let mut response = compact_agent_result(
            &status,
            &mut result_value,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Pagination {
                has_more: page_has_more,
                cursor: None,
            },
            budget,
            format!("query={} filters", request.query),
            &["results"],
        )?;
        let returned = response.result["results"].as_array().map_or(0, Vec::len);
        response.pagination.has_more = offset.saturating_add(returned) < total;
        response.pagination.cursor = response
            .pagination
            .has_more
            .then(|| encode_cursor(&binding, offset.saturating_add(returned)));
        response.budget.estimated_tokens = estimated_tokens(&response);
        Ok(response)
    }

    /// Returns a symbol and its direct inbound/outbound relationships.
    ///
    /// # Errors
    ///
    /// Returns stable-ID, symbol, index, budget, or database errors.
    #[allow(clippy::missing_panics_doc, clippy::too_many_lines)]
    pub fn get_symbol_context(
        &self,
        request: &SymbolContextRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let budget = self.budget(request.token_budget)?;
        let depth = request
            .depth
            .unwrap_or(1)
            .min(self.config.maximum_query_depth);
        if depth == 0 {
            return Err(GraphineError::InvalidArgument(
                "depth must be positive".to_owned(),
            ));
        }
        let stable_id = StableId::parse(request.stable_id.clone())?;
        let (status, node) = self.database.node(&request.project, &stable_id)?;
        let binding = CursorBinding::new(
            &status,
            "symbol_context",
            Some(stable_id.as_str()),
            format!(
                "detail={:?};include={:?};depth={depth}",
                request.detail, request.include
            ),
        );
        let offset = decode_cursor(request.cursor.as_deref(), &binding)?;
        let (total, relationships) = self.database.symbol_context_edges(
            &status,
            stable_id.as_str(),
            offset,
            self.config.maximum_result_count,
        )?;
        let mut grouped = std::collections::BTreeMap::<String, Vec<Value>>::from([
            ("callers".to_owned(), Vec::new()),
            ("callees".to_owned(), Vec::new()),
            ("dependencies".to_owned(), Vec::new()),
            ("data_access".to_owned(), Vec::new()),
            ("routes".to_owned(), Vec::new()),
            ("events".to_owned(), Vec::new()),
            ("tests".to_owned(), Vec::new()),
        ]);
        let mut evidence_refs = BTreeSet::new();
        let mut ambiguities = Vec::new();
        let mut unresolved = node.unresolved.clone();
        for (direction, edge) in &relationships {
            let occurrence_limit = match request.detail {
                DetailLevel::Summary => 0,
                DetailLevel::Standard => 1,
                DetailLevel::Detailed => 10,
                DetailLevel::Evidence => self.config.maximum_result_count,
            };
            let occurrences = if occurrence_limit == 0 {
                Vec::new()
            } else {
                self.database
                    .relationship_occurrences(&status, edge, occurrence_limit)?
            };
            let occurrence_ids: Vec<_> = occurrences
                .iter()
                .map(|occurrence| {
                    evidence_id(
                        &status,
                        &EvidenceLocation {
                            file_path: occurrence.file_path.clone(),
                            start_line: occurrence.start_line,
                            end_line: occurrence.end_line,
                        },
                    )
                })
                .collect();
            evidence_refs.extend(occurrence_ids.iter().cloned());
            let related_id = if direction == "inbound" {
                &edge.source_stable_id
            } else {
                &edge.target_stable_id
            };
            let related = self.database.node_in_status(&status, related_id)?;
            unresolved.extend(related.unresolved.clone());
            if edge.confidence == graphine_protocol::Confidence::Ambiguous {
                ambiguities.push(format!("{} has multiple static candidates", edge.kind));
            }
            let category = match (direction.as_str(), edge.kind.as_str()) {
                ("inbound", "CALLS") => "callers",
                ("outbound", "CALLS") => "callees",
                (_, "INJECTS" | "SELECTED_BEAN" | "BEAN_CANDIDATE") => "dependencies",
                (
                    _,
                    "READS_ENTITY" | "WRITES_ENTITY" | "READS" | "WRITES" | "USES_REPOSITORY"
                    | "REPOSITORY_FOR",
                ) => "data_access",
                (_, "HANDLED_BY" | "EXPOSES_ROUTE") => "routes",
                (_, "PUBLISHES_EVENT" | "LISTENS_TO_EVENT") => "events",
                _ if related
                    .file_path
                    .as_deref()
                    .is_some_and(|path| path.replace('\\', "/").contains("src/test/")) =>
                {
                    "tests"
                }
                _ => continue,
            };
            if !request.include.is_empty()
                && !request.include.iter().any(|included| {
                    included == category || (included == "injections" && category == "dependencies")
                })
            {
                continue;
            }
            grouped.get_mut(category).expect("known category").push(json!({
                "stable_id": related.stable_id,
                "qualified_name": related.qualified_name,
                "kind": related.kind,
                "relationship": edge.kind,
                "direction": direction,
                "confidence": edge.confidence,
                "occurrence_count": edge.occurrence_count,
                "evidence_refs": occurrence_ids,
                "evidence_truncated": u64::try_from(occurrences.len()).unwrap_or(u64::MAX) < edge.occurrence_count,
                "metadata": (request.detail != DetailLevel::Summary).then_some(edge.metadata.clone()),
            }));
        }
        if depth > 1
            && (request.include.is_empty()
                || request.include.iter().any(|value| value == "callees"))
        {
            let mut queue = VecDeque::from([(node.stable_id.clone(), 1_u32)]);
            let mut seen = BTreeSet::from([node.stable_id.clone()]);
            while let Some((current, current_depth)) = queue.pop_front() {
                if current_depth >= depth {
                    continue;
                }
                for edge in self.database.direct_edges(
                    &status,
                    &current,
                    false,
                    &["CALLS".to_owned()],
                    self.config.maximum_result_count,
                )? {
                    if !seen.insert(edge.target_stable_id.clone()) {
                        continue;
                    }
                    let related = self
                        .database
                        .node_in_status(&status, &edge.target_stable_id)?;
                    if related.file_path.is_none() {
                        continue;
                    }
                    grouped
                        .get_mut("callees")
                        .expect("known category")
                        .push(json!({
                            "stable_id": related.stable_id,
                            "qualified_name": related.qualified_name,
                            "kind": related.kind,
                            "relationship": edge.kind,
                            "depth": current_depth + 1,
                            "confidence": edge.confidence,
                        }));
                    queue.push_back((related.stable_id, current_depth + 1));
                }
            }
        }
        if node.confidence == graphine_protocol::Confidence::Ambiguous {
            ambiguities.push("symbol confidence is AMBIGUOUS".to_owned());
        }
        if let (Some(file_path), Some(start_line), Some(end_line)) =
            (&node.file_path, node.start_line, node.end_line)
        {
            evidence_refs.insert(evidence_id(
                &status,
                &EvidenceLocation {
                    file_path: file_path.clone(),
                    start_line,
                    end_line,
                },
            ));
        }
        let mut result_value = json!({
            "symbol": search_result_fact(&status, &node, request.detail),
            "java_signature": java_signature(&node),
            "annotations": node.metadata.get("annotations").cloned().unwrap_or_else(|| json!([])),
            "framework_role": framework_role(&node),
            "callers": grouped.remove("callers").unwrap_or_default(),
            "callees": grouped.remove("callees").unwrap_or_default(),
            "dependencies": grouped.remove("dependencies").unwrap_or_default(),
            "data_access": grouped.remove("data_access").unwrap_or_default(),
            "routes": grouped.remove("routes").unwrap_or_default(),
            "events": grouped.remove("events").unwrap_or_default(),
            "tests": grouped.remove("tests").unwrap_or_default(),
            "evidence_refs": evidence_refs,
        });
        let action_references = result_value["evidence_refs"]
            .as_array()
            .into_iter()
            .flatten()
            .take(3)
            .map(|id| json!({"id": id}))
            .collect::<Vec<_>>();
        let page_has_more = offset.saturating_add(relationships.len()) < total;
        let mut response = compact_agent_result(
            &status,
            &mut result_value,
            unresolved,
            ambiguities,
            vec![json!({
                "tool": "get_evidence",
                "reason": "Verify selected symbol relationships",
                "arguments": {"project": request.project, "references": action_references}
            })],
            Pagination {
                has_more: page_has_more,
                cursor: page_has_more
                    .then(|| encode_cursor(&binding, offset.saturating_add(relationships.len()))),
            },
            budget,
            format!("depth={depth}"),
            &[
                "tests",
                "evidence_refs",
                "events",
                "routes",
                "dependencies",
                "data_access",
                "callees",
                "callers",
            ],
        )?;
        response.budget.estimated_tokens = estimated_tokens(&response);
        Ok(response)
    }

    /// Traverses direct edges breadth-first without loading the full graph.
    ///
    /// # Errors
    ///
    /// Returns validation, stable-ID, index, cursor, budget, or database errors.
    #[allow(clippy::too_many_lines)]
    pub fn trace_flow(
        &self,
        request: &TraceFlowRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let budget = self.budget(request.token_budget)?;
        let start = StableId::parse(request.start_stable_id.clone())?;
        let max_depth = request
            .max_depth
            .unwrap_or(3)
            .min(self.config.maximum_query_depth);
        let max_nodes = request
            .max_nodes
            .unwrap_or(50)
            .min(self.config.maximum_result_count);
        let max_paths = request
            .max_paths
            .unwrap_or(20)
            .min(self.config.maximum_result_count);
        let mode = request.mode.as_deref().unwrap_or("all_paths_bounded");
        if !matches!(mode, "shortest_path" | "all_paths_bounded") {
            return Err(GraphineError::InvalidArgument(
                "mode must be shortest_path or all_paths_bounded".to_owned(),
            ));
        }
        if max_depth == 0 || max_nodes == 0 || max_paths == 0 {
            return Err(GraphineError::InvalidArgument(
                "depth and node limits must be positive".to_owned(),
            ));
        }
        let (status, _) = self.database.node(&request.project, &start)?;
        let mut kinds = request.edge_kinds.clone();
        for group in &request.edge_groups {
            let mapped: &[&str] = match group.as_str() {
                "calls" => &["CALLS"],
                "data_access" => &[
                    "USES_REPOSITORY",
                    "READS_ENTITY",
                    "WRITES_ENTITY",
                    "READS",
                    "WRITES",
                    "REPOSITORY_FOR",
                ],
                "events" => &["PUBLISHES_EVENT", "LISTENS_TO_EVENT"],
                "dependencies" => &["INJECTS", "SELECTED_BEAN", "BEAN_CANDIDATE"],
                "routes" => &["EXPOSES_ROUTE", "HANDLED_BY"],
                _ => {
                    return Err(GraphineError::InvalidArgument(format!(
                        "unknown edge group: {group}"
                    )));
                }
            };
            kinds.extend(mapped.iter().map(ToString::to_string));
        }
        kinds.sort();
        kinds.dedup();
        let binding = CursorBinding::new(
            &status,
            "trace_flow",
            Some(start.as_str()),
            format!(
                "direction={:?};kinds={:?};groups={:?};node_kinds={:?};application={};external={};mode={mode};depth={max_depth};nodes={max_nodes};paths={max_paths}",
                request.direction,
                kinds,
                request.edge_groups,
                request.node_kinds,
                request.application_only,
                request.suppress_external
            ),
        );
        let offset = decode_cursor(request.cursor.as_deref(), &binding)?;
        let start_node = self.database.node_in_status(&status, start.as_str())?;
        let mut queue = VecDeque::from([(
            start_node.clone(),
            vec![start_node.clone()],
            Vec::<String>::new(),
            0_u32,
        )]);
        let mut visited = BTreeSet::from([start.as_str().to_owned()]);
        let mut paths = Vec::new();
        let mut cycle_count = 0_u64;
        let mut suppressed_external_count = 0_u64;
        while let Some((current, path_nodes, path_edges, depth)) = queue.pop_front() {
            if paths.len() >= max_paths as usize {
                break;
            }
            if depth >= max_depth || visited.len() >= max_nodes as usize {
                paths.push(path_value(&path_nodes, &path_edges));
                continue;
            }
            let inbound = request.direction == Direction::Inbound;
            let edges = self.database.direct_edges(
                &status,
                &current.stable_id,
                inbound,
                &kinds,
                max_nodes,
            )?;
            let mut expanded = false;
            for edge in edges {
                let next = if inbound {
                    edge.source_stable_id.clone()
                } else {
                    edge.target_stable_id.clone()
                };
                let next_node = self.database.node_in_status(&status, &next)?;
                if (request.application_only || request.suppress_external)
                    && next_node.file_path.is_none()
                {
                    suppressed_external_count = suppressed_external_count.saturating_add(1);
                    continue;
                }
                if !request.node_kinds.is_empty()
                    && !request
                        .node_kinds
                        .iter()
                        .any(|kind| kind.eq_ignore_ascii_case(&next_node.kind))
                {
                    continue;
                }
                if path_nodes
                    .iter()
                    .any(|node| node.stable_id == next_node.stable_id)
                {
                    cycle_count = cycle_count.saturating_add(1);
                    continue;
                }
                if mode == "shortest_path" && !visited.insert(next_node.stable_id.clone()) {
                    continue;
                }
                if mode != "shortest_path" {
                    visited.insert(next_node.stable_id.clone());
                }
                let mut next_nodes = path_nodes.clone();
                next_nodes.push(next_node.clone());
                let mut next_edges = path_edges.clone();
                next_edges.push(edge.kind);
                queue.push_back((next_node, next_nodes, next_edges, depth + 1));
                expanded = true;
            }
            if !expanded {
                paths.push(path_value(&path_nodes, &path_edges));
            }
        }
        paths.sort_by_key(Value::to_string);
        paths.dedup();
        let total = paths.len();
        let page: Vec<_> = paths
            .into_iter()
            .skip(offset)
            .take(max_paths as usize)
            .collect();
        let returned_count = page.len();
        let has_more = offset.saturating_add(page.len()) < total;
        let mut result_value = json!({
            "start": search_result_fact(&status, &start_node, DetailLevel::Summary),
            "direction": request.direction,
            "mode": mode,
            "edge_groups": request.edge_groups,
            "paths": page,
            "visited_node_count": visited.len(),
            "cycle_count": cycle_count,
            "suppressed_external_count": suppressed_external_count,
            "application_only": request.application_only,
        });
        let mut response = compact_agent_result(
            &status,
            &mut result_value,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Pagination {
                has_more,
                cursor: has_more
                    .then(|| encode_cursor(&binding, offset.saturating_add(returned_count))),
            },
            budget,
            format!("max_depth={max_depth}, max_paths={max_paths}"),
            &["paths"],
        )?;
        let returned_paths = response
            .result
            .get("paths")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        response.pagination.has_more = offset.saturating_add(returned_paths) < total;
        response.pagination.cursor = response
            .pagination
            .has_more
            .then(|| encode_cursor(&binding, offset.saturating_add(returned_paths)));
        response.budget.estimated_tokens = estimated_tokens(&response);
        Ok(response)
    }

    /// Returns current lifecycle state and active graph counts.
    ///
    /// # Errors
    ///
    /// Returns project, budget, or database errors.
    pub fn index_status(
        &self,
        request: &IndexStatusRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let budget = self.budget(request.token_budget)?;
        let status = self.database.status(&request.project)?;
        let diagnostics = self.database.diagnostic_counts(&status)?;
        let graph = self.database.graph_summary(&request.project)?;
        let source_sets = self.database.source_sets(&status)?;
        let capabilities = status
            .analysis_summary
            .as_ref()
            .and_then(|summary| summary.get("capabilities"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let indexing_time_ms = status
            .analysis_summary
            .as_ref()
            .and_then(|summary| summary.get("duration_ms"))
            .cloned();
        let mut result_value = json!({
            "state": status.state,
            "generation": status.active_generation,
            "source_fingerprint": status.project.source_fingerprint,
            "stale": status.stale,
            "partial": status.partial,
            "analyzer_versions": {
                "graphine": env!("CARGO_PKG_VERSION"),
                "java_analyzer": status.project.analyzer_version,
                "protocol": status.analyzer_protocol_version,
            },
            "capabilities": {
                "java": capabilities.get("java_semantics").or_else(|| capabilities.get("java")).cloned().unwrap_or(Value::Bool(true)),
                "spring": capabilities.get("spring_static_semantics").or_else(|| capabilities.get("spring")).cloned().unwrap_or(Value::Bool(false)),
                "reported": capabilities,
            },
            "diagnostic_counts": diagnostics,
            "indexing_time_ms": indexing_time_ms,
            "indexed_modules": graph.modules,
            "indexed_source_sets": source_sets,
            "excluded_files": [],
            "evidence_excluded_paths": self.config.evidence_excluded_paths,
            "unsupported_areas": ["runtime execution paths", "runtime bean condition outcomes", "reflection-only wiring", "dynamic routes"],
            "counts": {"nodes": status.node_count, "edges": status.edge_count, "occurrences": status.occurrence_count},
            "last_successful_activation_ms": status.last_successful_activation_ms,
        });
        compact_agent_result(
            &status,
            &mut result_value,
            status.failure_summary.clone().into_iter().collect(),
            Vec::new(),
            Vec::new(),
            Pagination::default(),
            budget,
            "active generation status".to_owned(),
            &[
                "unsupported_areas",
                "indexed_source_sets",
                "indexed_modules",
                "diagnostic_counts",
            ],
        )
    }

    fn budget(&self, requested: Option<u32>) -> Result<u32, GraphineError> {
        let budget = requested.unwrap_or(self.config.default_token_budget);
        if !(128..=self.config.maximum_token_budget).contains(&budget) {
            return Err(GraphineError::InvalidTokenBudget);
        }
        Ok(budget)
    }
}

#[allow(dead_code)]
struct EnvelopeParts {
    project: String,
    generation: Option<i64>,
    stale: bool,
    partial: bool,
    summary: Value,
    essential_summary: Value,
    facts: Vec<Value>,
    secondary_facts: Vec<Value>,
    unresolved: Vec<String>,
    ambiguities: Vec<String>,
    evidence: Vec<EvidenceRef>,
    has_more: bool,
    cursor: Option<String>,
}

#[allow(dead_code)]
impl EnvelopeParts {
    fn new(status: &IndexStatus, summary: Value) -> Self {
        Self {
            project: status.project.display_name.clone(),
            generation: status.active_generation,
            stale: status.stale,
            partial: status.partial,
            summary,
            essential_summary: json!({"active_generation": status.active_generation}),
            facts: Vec::new(),
            secondary_facts: Vec::new(),
            unresolved: Vec::new(),
            ambiguities: Vec::new(),
            evidence: Vec::new(),
            has_more: false,
            cursor: None,
        }
    }
}

#[allow(dead_code, clippy::too_many_lines)]
fn compact(parts: EnvelopeParts, requested: u32) -> ResponseEnvelope {
    let total_facts = parts
        .facts
        .len()
        .saturating_add(parts.secondary_facts.len());
    let total_evidence = parts.evidence.len();
    let total_unresolved = parts.unresolved.len();
    let total_ambiguities = parts.ambiguities.len();
    let mut envelope = ResponseEnvelope {
        project: parts.project,
        generation: parts.generation,
        stale: parts.stale,
        partial: parts.partial,
        completeness: Completeness::default(),
        result: Value::Null,
        complete: true,
        summary: parts.essential_summary.clone(),
        facts: Vec::new(),
        unresolved: Vec::new(),
        ambiguities: Vec::new(),
        uncertainty: UncertaintyState {
            unresolved_count: u64::try_from(total_unresolved).unwrap_or(u64::MAX),
            ambiguity_count: u64::try_from(total_ambiguities).unwrap_or(u64::MAX),
            ..UncertaintyState::default()
        },
        evidence_refs: Vec::new(),
        pagination: Pagination {
            has_more: parts.has_more,
            cursor: parts.cursor,
        },
        next_actions: Vec::new(),
        budget: Budget {
            requested_tokens: requested,
            estimated_tokens: 0,
            truncated: false,
        },
        truncation: TruncationState::default(),
    };
    refresh_estimate(&mut envelope);
    if envelope.budget.estimated_tokens > requested {
        envelope.summary = json!({"identity": "requested answer"});
        refresh_estimate(&mut envelope);
    }

    // Critical uncertainty details are admitted before facts. Aggregate counts above are
    // mandatory and therefore remain even when no individual detail can fit.
    for ambiguity in parts.ambiguities {
        envelope.ambiguities.push(ambiguity);
        refresh_estimate(&mut envelope);
        if envelope.budget.estimated_tokens > requested {
            envelope.ambiguities.pop();
            break;
        }
    }
    for unresolved in parts.unresolved {
        envelope.unresolved.push(unresolved);
        refresh_estimate(&mut envelope);
        if envelope.budget.estimated_tokens > requested {
            envelope.unresolved.pop();
            break;
        }
    }
    admit_facts(&mut envelope, parts.facts, requested);
    for evidence in parts.evidence {
        envelope.evidence_refs.push(evidence);
        refresh_estimate(&mut envelope);
        if envelope.budget.estimated_tokens > requested {
            envelope.evidence_refs.pop();
            break;
        }
    }

    // Lower-confidence and secondary relationships follow evidence so uncertainty and
    // source support cannot be displaced by inferred graph expansion.
    admit_facts(&mut envelope, parts.secondary_facts, requested);

    // Full summaries frequently contain optional counts and analyzer metadata, so they
    // are restored only after requested facts and evidence have been considered.
    let essential_summary = envelope.summary.clone();
    envelope.summary = parts.summary;
    refresh_estimate(&mut envelope);
    let summary_truncated = envelope.budget.estimated_tokens > requested;
    if summary_truncated {
        envelope.summary = essential_summary;
    }

    envelope.truncation = TruncationState {
        facts_truncated: (envelope.facts.len() < total_facts).then_some(true),
        evidence_truncated: (envelope.evidence_refs.len() < total_evidence).then_some(true),
        unresolved_truncated: (envelope.unresolved.len() < total_unresolved).then_some(true),
        ambiguities_truncated: (envelope.ambiguities.len() < total_ambiguities).then_some(true),
        summary_truncated: summary_truncated.then_some(true),
    };
    envelope.uncertainty.unresolved_truncated = envelope.truncation.unresolved_truncated;
    envelope.uncertainty.ambiguities_truncated = envelope.truncation.ambiguities_truncated;
    envelope.budget.truncated = envelope.truncation.facts_truncated.is_some()
        || envelope.truncation.evidence_truncated.is_some()
        || envelope.truncation.unresolved_truncated.is_some()
        || envelope.truncation.ambiguities_truncated.is_some()
        || envelope.truncation.summary_truncated.is_some();
    envelope.complete = !envelope.budget.truncated && !envelope.partial && !envelope.stale;
    envelope.completeness = Completeness {
        status: if envelope.partial {
            "partial"
        } else if envelope.budget.truncated {
            "truncated"
        } else if envelope.uncertainty.ambiguity_count > 0 {
            "ambiguous"
        } else if envelope.uncertainty.unresolved_count > 0 {
            "unresolved"
        } else {
            "complete"
        }
        .to_owned(),
        scope: "requested scope".to_owned(),
        facts_truncated: envelope.truncation.facts_truncated.unwrap_or(false),
        evidence_truncated: envelope.truncation.evidence_truncated.unwrap_or(false),
        uncertainty_truncated: envelope.truncation.unresolved_truncated.unwrap_or(false)
            || envelope.truncation.ambiguities_truncated.unwrap_or(false),
    };
    refresh_estimate(&mut envelope);
    envelope
}

#[allow(dead_code)]
fn admit_facts(envelope: &mut ResponseEnvelope, facts: Vec<Value>, requested: u32) {
    for fact in facts {
        envelope.facts.push(fact);
        refresh_estimate(envelope);
        if envelope.budget.estimated_tokens > requested {
            envelope.facts.pop();
            break;
        }
    }
}

#[allow(dead_code)]
fn refresh_estimate(envelope: &mut ResponseEnvelope) {
    envelope.budget.estimated_tokens = 0;
    envelope.budget.estimated_tokens = estimated_tokens(envelope);
}

#[allow(dead_code)]
fn apply_pagination(
    envelope: &mut ResponseEnvelope,
    binding: &CursorBinding,
    offset: usize,
    total: usize,
    requested: u32,
) {
    loop {
        let consumed = offset.saturating_add(envelope.facts.len());
        envelope.pagination.has_more = consumed < total;
        envelope.pagination.cursor = envelope
            .pagination
            .has_more
            .then(|| encode_cursor(binding, consumed));
        refresh_estimate(envelope);
        if envelope.budget.estimated_tokens <= requested {
            break;
        }
        if envelope.evidence_refs.pop().is_some() {
            envelope.truncation.evidence_truncated = Some(true);
        } else if envelope.facts.pop().is_some() {
            envelope.truncation.facts_truncated = Some(true);
        } else if envelope.summary
            != json!({"identity": binding.stable_id.as_deref().unwrap_or(binding.operation)})
        {
            envelope.summary = json!({
                "identity": binding.stable_id.as_deref().unwrap_or(binding.operation)
            });
            envelope.truncation.summary_truncated = Some(true);
        } else {
            break;
        }
        envelope.budget.truncated = true;
        envelope.complete = false;
    }
    refresh_estimate(envelope);
}

/// Returns a conservative character-based estimate, not an exact tokenizer count.
#[must_use]
pub fn estimated_tokens<T: Serialize>(value: &T) -> u32 {
    let chars = serde_json::to_string(value).map_or(0, |serialized| serialized.chars().count());
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

/// Computes deterministic Phase 1 lexical relevance.
#[must_use]
pub fn lexical_rank(query: &str, node: &NodeRecord) -> u32 {
    let query_lower = query.to_ascii_lowercase();
    let qualified = node.qualified_name.to_ascii_lowercase();
    let simple = node.simple_name.to_ascii_lowercase();
    let normalized_query = normalize_tokens(query);
    let normalized_simple = normalize_tokens(&node.simple_name);
    if query == node.stable_id {
        900
    } else if query_lower == qualified {
        700
    } else if query_lower == simple {
        600
    } else if normalized_query == normalized_simple {
        500
    } else if qualified.starts_with(&query_lower) || simple.starts_with(&query_lower) {
        400
    } else if normalized_simple.starts_with(&normalized_query)
        || normalized_simple
            .split(' ')
            .any(|token| token == normalized_query)
    {
        350
    } else if qualified.contains(&query_lower) || normalized_simple.contains(&normalized_query) {
        200
    } else {
        0
    }
}

fn agent_search_rank(request: &SearchSymbolRequest, node: &NodeRecord) -> u32 {
    let mut rank = lexical_rank(request.query.trim(), node);
    if node.file_path.is_some() {
        rank += 80;
    }
    if request
        .module
        .as_ref()
        .is_some_and(|module| node.module_name.as_deref() == Some(module.as_str()))
    {
        rank += 40;
    }
    if request.package_prefix.as_ref().is_some_and(|prefix| {
        node.package_name
            .as_deref()
            .is_some_and(|package| package.starts_with(prefix))
    }) {
        rank += 30;
    }
    if framework_role(node).is_some() {
        rank += 20;
    }
    rank += match node.kind.as_str() {
        "ROUTE" => 35,
        "REPOSITORY" | "ENTITY" | "BEAN" | "EVENT_TYPE" => 25,
        "METHOD" if node.metadata.get("has_body").and_then(Value::as_bool) == Some(true) => 10,
        _ => 0,
    };
    rank
}

fn framework_role(node: &NodeRecord) -> Option<String> {
    match node.kind.as_str() {
        "ROUTE" => Some("ROUTE".to_owned()),
        "REPOSITORY" => Some("REPOSITORY".to_owned()),
        "ENTITY" => Some("ENTITY".to_owned()),
        "EVENT_TYPE" => Some("EVENT".to_owned()),
        "CONFIG_PROPERTY" => Some("CONFIGURATION".to_owned()),
        "BEAN" => node
            .metadata
            .get("stereotype")
            .and_then(Value::as_str)
            .map(str::to_ascii_uppercase)
            .or_else(|| Some("BEAN".to_owned())),
        _ => node
            .metadata
            .get("annotations")
            .and_then(Value::as_array)
            .and_then(|annotations| {
                annotations.iter().find_map(|annotation| {
                    let annotation = annotation.as_str()?;
                    if annotation.ends_with("RestController") || annotation.ends_with("Controller")
                    {
                        Some("CONTROLLER".to_owned())
                    } else if annotation.ends_with("Service") {
                        Some("SERVICE".to_owned())
                    } else {
                        None
                    }
                })
            }),
    }
}

fn resolved_framework_role(
    database: &Database,
    status: &IndexStatus,
    node: &NodeRecord,
) -> Option<String> {
    framework_role(node).or_else(|| {
        let owner = node
            .metadata
            .get("declaring_type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                if node.kind != "METHOD" && node.kind != "CONSTRUCTOR" {
                    return None;
                }
                let (_, value) = node.stable_id.split_once(':')?;
                let (owner, _) = value.split_once('#')?;
                Some(format!("type:{owner}"))
            })?;
        database
            .node_in_status(status, &owner)
            .ok()
            .and_then(|owner| framework_role(&owner))
    })
}

fn search_result_fact(status: &IndexStatus, node: &NodeRecord, detail: DetailLevel) -> Value {
    let route = (node.kind == "ROUTE").then(|| {
        json!({
            "method": node.metadata.get("http_method"),
            "path": node.metadata.get("path"),
        })
    });
    let evidence = match (&node.file_path, node.start_line, node.end_line) {
        (Some(file_path), Some(start_line), Some(end_line)) => Some(evidence_id(
            status,
            &EvidenceLocation {
                file_path: file_path.clone(),
                start_line,
                end_line,
            },
        )),
        _ => None,
    };
    let mut fact = json!({
        "stable_id": node.stable_id,
        "kind": node.kind,
        "qualified_name": node.qualified_name,
        "framework_role": framework_role(node),
        "signature": (node.kind == "METHOD" || node.kind == "CONSTRUCTOR").then(|| java_signature(node)),
        "route": route,
        "location": node.file_path.as_ref().map(|file| json!({
            "file": file,
            "start_line": node.start_line,
            "end_line": node.end_line,
        })),
        "evidence_ref": evidence,
        "confidence": node.confidence,
    });
    if matches!(detail, DetailLevel::Detailed | DetailLevel::Evidence) {
        fact.as_object_mut()
            .expect("object literal")
            .insert("metadata".to_owned(), node.metadata.clone());
    }
    fact
}

fn path_value(nodes: &[NodeRecord], edge_kinds: &[String]) -> Value {
    json!({
        "nodes": nodes.iter().map(|node| json!({
            "stable_id": node.stable_id,
            "qualified_name": node.qualified_name,
            "kind": node.kind,
            "confidence": node.confidence,
        })).collect::<Vec<_>>(),
        "edge_kinds": edge_kinds,
    })
}

fn java_signature(node: &NodeRecord) -> String {
    node.stable_id.split_once(':').map_or_else(
        || node.qualified_name.clone(),
        |(_, signature)| signature.to_owned(),
    )
}

fn normalize_tokens(value: &str) -> String {
    let mut normalized = String::new();
    let mut previous_lower = false;
    for character in value.chars() {
        if character.is_ascii_uppercase() && previous_lower {
            normalized.push(' ');
        }
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
            previous_lower = character.is_lowercase();
        } else {
            if !normalized.ends_with(' ') {
                normalized.push(' ');
            }
            previous_lower = false;
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct CursorBinding {
    project_id: String,
    generation: i64,
    operation: &'static str,
    stable_id: Option<String>,
    scope: String,
}

impl CursorBinding {
    fn new(
        status: &IndexStatus,
        operation: &'static str,
        stable_id: Option<&str>,
        scope: String,
    ) -> Self {
        Self {
            project_id: status.project.id.as_str().to_owned(),
            generation: status.active_generation.unwrap_or_default(),
            operation,
            stable_id: stable_id.map(str::to_owned),
            scope,
        }
    }
}

fn encode_cursor(binding: &CursorBinding, offset: usize) -> String {
    format!(
        "v1:{}:{offset}:{}",
        binding.generation,
        cursor_binding_digest(binding)
    )
}

fn decode_cursor(cursor: Option<&str>, binding: &CursorBinding) -> Result<usize, GraphineError> {
    let Some(cursor) = cursor else { return Ok(0) };
    if cursor.len() > 256 {
        return Err(GraphineError::InvalidCursor);
    }
    let mut parts = cursor.split(':');
    if parts.next() != Some("v1") {
        return Err(GraphineError::InvalidCursor);
    }
    let generation = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(GraphineError::InvalidCursor)?;
    let offset = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(GraphineError::InvalidCursor)?;
    let digest = parts.next().ok_or(GraphineError::InvalidCursor)?;
    if parts.next().is_some()
        || generation != binding.generation
        || digest != cursor_binding_digest(binding)
    {
        return Err(GraphineError::InvalidCursor);
    }
    Ok(offset)
}

fn cursor_binding_digest(binding: &CursorBinding) -> String {
    fn update(mut hash: u64, bytes: &[u8]) -> u64 {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    let canonical = format!(
        "{}\0{}\0{}\0{}\0{}",
        binding.project_id,
        binding.generation,
        binding.operation,
        binding.stable_id.as_deref().unwrap_or(""),
        binding.scope
    );
    let first = update(0xcbf2_9ce4_8422_2325, canonical.as_bytes());
    let second = update(0x8422_2325_cbf2_9ce4, canonical.as_bytes());
    format!("{first:016x}{second:016x}")
}

#[allow(dead_code)]
fn node_fact(node: &NodeRecord, detail: DetailLevel) -> Value {
    let mut fact = json!({
        "stable_id": node.stable_id,
        "kind": node.kind,
        "qualified_name": node.qualified_name,
        "simple_name": node.simple_name,
        "confidence": node.confidence,
    });
    if detail != DetailLevel::Summary {
        let object = fact.as_object_mut().expect("object literal");
        object.insert("file_path".to_owned(), json!(node.file_path));
        object.insert("start_line".to_owned(), json!(node.start_line));
        object.insert("end_line".to_owned(), json!(node.end_line));
        object.insert("module_name".to_owned(), json!(node.module_name));
        object.insert("package_name".to_owned(), json!(node.package_name));
    }
    if matches!(detail, DetailLevel::Detailed | DetailLevel::Evidence) {
        fact.as_object_mut()
            .expect("object literal")
            .insert("metadata".to_owned(), node.metadata.clone());
    }
    fact
}

#[allow(dead_code)]
fn edge_fact(
    edge: &EdgeRecord,
    direction: &str,
    detail: DetailLevel,
    occurrences: &[graphine_protocol::EdgeOccurrence],
) -> Value {
    let mut fact = json!({
        "direction": direction,
        "source": edge.source_stable_id,
        "target": edge.target_stable_id,
        "kind": edge.kind,
        "confidence": edge.confidence,
        "occurrence_count": edge.occurrence_count,
    });
    if matches!(detail, DetailLevel::Detailed | DetailLevel::Evidence) {
        fact.as_object_mut()
            .expect("object literal")
            .insert("metadata".to_owned(), edge.metadata.clone());
        let evidence: Vec<_> = occurrences
            .iter()
            .map(|occurrence| {
                json!({
                    "file": occurrence.file_path,
                    "start_line": occurrence.start_line,
                    "end_line": occurrence.end_line,
                    "metadata": occurrence.metadata,
                })
            })
            .collect();
        let object = fact.as_object_mut().expect("object literal");
        object.insert("evidence_refs".to_owned(), json!(evidence));
        object.insert(
            "evidence_truncated".to_owned(),
            json!(u64::try_from(occurrences.len()).unwrap_or(u64::MAX) < edge.occurrence_count),
        );
    }
    fact
}

#[allow(dead_code)]
fn is_high_confidence_fact(fact: &Value) -> bool {
    matches!(
        fact.get("confidence").and_then(Value::as_str),
        Some(
            "RUNTIME_CONFIRMED" | "BYTECODE_CONFIRMED" | "COMPILER_RESOLVED" | "FRAMEWORK_RESOLVED"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphine_protocol::{Confidence, SyntheticEdge, SyntheticGraph, SyntheticNode};
    use std::fs;
    use std::time::Instant;

    fn node(name: &str) -> NodeRecord {
        NodeRecord {
            stable_id: format!("type:example.{name}"),
            kind: "TYPE".to_owned(),
            qualified_name: format!("example.{name}"),
            simple_name: name.to_owned(),
            module_name: None,
            package_name: Some("example".to_owned()),
            file_path: None,
            start_line: None,
            end_line: None,
            confidence: Confidence::CompilerResolved,
            provenance: "test".to_owned(),
            metadata: json!({}),
            unresolved: Vec::new(),
        }
    }

    #[test]
    fn estimator_is_character_based_and_deterministic() {
        assert_eq!(estimated_tokens(&"12345678"), 3);
        assert_eq!(
            estimated_tokens(&json!({"a": 1})),
            estimated_tokens(&json!({"a": 1}))
        );
    }

    #[test]
    fn lexical_ranking_follows_contract_priority() {
        let candidate = node("CreateEvent");
        assert!(
            lexical_rank("example.CreateEvent", &candidate)
                > lexical_rank("CreateEvent", &node("CreateEventHandler"))
        );
        assert!(lexical_rank("CreateEvent", &candidate) > lexical_rank("create", &candidate));
        assert!(lexical_rank("create event", &candidate) > lexical_rank("event", &candidate));
    }

    #[test]
    fn cursors_are_bound_and_reject_malformed_values() {
        let status = fake_status();
        let binding = CursorBinding::new(
            &status,
            "symbol_context",
            Some("type:example.A"),
            "detail=Summary".to_owned(),
        );
        let cursor = encode_cursor(&binding, 42);
        assert_eq!(decode_cursor(Some(&cursor), &binding).unwrap(), 42);
        let other = CursorBinding::new(
            &status,
            "symbol_context",
            Some("type:example.B"),
            "detail=Summary".to_owned(),
        );
        assert!(decode_cursor(Some(&cursor), &other).is_err());
        assert!(decode_cursor(Some("offset:42"), &binding).is_err());
        assert!(decode_cursor(Some("v1:nope"), &binding).is_err());
    }

    #[test]
    fn smaller_budgets_produce_valid_deterministic_subsets() {
        let status = fake_status();
        let mut parts = EnvelopeParts::new(&status, json!({"answer": "identity"}));
        parts.facts = (0..20)
            .map(|index| json!({"rank": index, "value": "a compact fact"}))
            .collect();
        let small = compact(parts, 128);
        assert!(small.budget.truncated);
        assert!(small.facts.len() < 20);
        assert!(serde_json::to_string(&small).is_ok());
        assert_eq!(
            small,
            serde_json::from_str(&serde_json::to_string(&small).unwrap()).unwrap()
        );
    }

    #[test]
    fn tiny_budgets_never_hide_aggregate_uncertainty() {
        let status = fake_status();
        let mut parts = EnvelopeParts::new(
            &status,
            json!({"answer": "identity", "optional": "metadata"}),
        );
        parts.essential_summary = json!({"answer": "identity"});
        parts.unresolved = (0..8)
            .map(|index| format!("unresolved-{index}-with-details"))
            .collect();
        parts.ambiguities = (0..2)
            .map(|index| format!("ambiguity-{index}-with-details"))
            .collect();
        parts.facts = (0..10).map(|index| json!({"fact": index})).collect();
        let first = compact(parts, 128);
        assert_eq!(first.uncertainty.unresolved_count, 8);
        assert_eq!(first.uncertainty.ambiguity_count, 2);
        assert!(
            first.uncertainty.unresolved_truncated.unwrap_or(false)
                || first.uncertainty.ambiguities_truncated.unwrap_or(false)
        );
        assert!(first.budget.truncated);
        assert!(!first.complete);
        assert!(serde_json::to_string(&first).is_ok());
    }

    #[test]
    fn large_synthetic_graph_lookup_and_context_are_bounded() {
        let root = std::env::temp_dir().join(format!("graphine-large-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut database = Database::open_in_memory().unwrap();
        database
            .register_project(&root, Some("large"), &[])
            .unwrap();
        let node_count = 10_000_usize;
        let nodes = (0..node_count)
            .map(|index| SyntheticNode {
                stable_id: format!("type:bench.Node{index}"),
                kind: "TYPE".to_owned(),
                qualified_name: format!("bench.Node{index}"),
                simple_name: None,
                module_name: Some("benchmark".to_owned()),
                package_name: Some("bench".to_owned()),
                file_path: None,
                start_line: None,
                end_line: None,
                confidence: Confidence::CompilerResolved,
                provenance: "generated".to_owned(),
                metadata: json!({}),
                unresolved: Vec::new(),
            })
            .collect();
        let edges = (0..node_count)
            .flat_map(|index| {
                (1..=3).map(move |step| SyntheticEdge {
                    source_stable_id: format!("type:bench.Node{index}"),
                    target_stable_id: format!("type:bench.Node{}", (index + step) % node_count),
                    kind: "RELATED".to_owned(),
                    confidence: Confidence::CompilerResolved,
                    provenance: "generated".to_owned(),
                    metadata: json!({}),
                    occurrences: Vec::new(),
                })
            })
            .collect();
        let graph = SyntheticGraph {
            project: "large".to_owned(),
            nodes,
            edges,
        };
        let load_started = Instant::now();
        database.load_synthetic("large", &graph).unwrap();
        let load_ms = load_started.elapsed().as_millis();
        let config = GraphineConfig::default();
        let service = QueryService::new(&database, &config);
        let lookup_started = Instant::now();
        let result = service
            .search_symbol(&SearchSymbolRequest {
                project: "large".to_owned(),
                query: "bench.Node9999".to_owned(),
                kinds: vec!["TYPE".to_owned()],
                framework_roles: Vec::new(),
                module: None,
                package_prefix: None,
                limit: Some(10),
                cursor: None,
                detail: DetailLevel::Summary,
                token_budget: Some(800),
            })
            .unwrap();
        let lookup_ms = lookup_started.elapsed().as_millis();
        let context_started = Instant::now();
        let context = service
            .get_symbol_context(&SymbolContextRequest {
                project: "large".to_owned(),
                stable_id: "type:bench.Node9999".to_owned(),
                include: Vec::new(),
                depth: None,
                cursor: None,
                detail: DetailLevel::Standard,
                token_budget: Some(800),
            })
            .unwrap();
        let context_ms = context_started.elapsed().as_millis();
        println!(
            "large_graph load_ms={load_ms} exact_lookup_ms={lookup_ms} context_ms={context_ms} estimated_tokens={} nodes=10000 edges=30000",
            context.budget.estimated_tokens
        );
        assert_eq!(result.result["matched"], 1);
        assert!(context.budget.estimated_tokens <= 800);
        assert_eq!(database.status("large").unwrap().edge_count, 30_000);
    }

    fn fake_status() -> IndexStatus {
        IndexStatus {
            project: graphine_index::ProjectRecord {
                id: graphine_protocol::ProjectId::from_canonical_root(std::path::Path::new(
                    "/tmp/example",
                )),
                canonical_root: "/tmp/example".into(),
                display_name: "example".to_owned(),
                registered_at_ms: 0,
                source_fingerprint: None,
                analyzer_version: "test".to_owned(),
                schema_version: 1,
            },
            active_generation: Some(1),
            state: graphine_protocol::GenerationState::Ready,
            stale: false,
            last_successful_activation_ms: Some(0),
            failure_summary: None,
            node_count: 1,
            edge_count: 0,
            occurrence_count: 0,
            diagnostic_count: 0,
            partial: false,
            analyzer_protocol_version: None,
            analysis_summary: None,
        }
    }
}
