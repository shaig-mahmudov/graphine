use super::{CursorBinding, QueryService, decode_cursor, encode_cursor, estimated_tokens};
use graphine_index::{EdgeRecord, EvidenceLocation, IndexStatus, NodeRecord};
use graphine_protocol::{
    Budget, Completeness, Confidence, GraphineError, Pagination, ResponseEnvelope, TruncationState,
    UncertaintyState,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path};

const DEFAULT_SUPPRESSION_POLICY: &str = "agent-default-v1";
const FLOW_EDGE_KINDS: &[&str] = &[
    "CALLS",
    "USES_REPOSITORY",
    "READS_ENTITY",
    "WRITES_ENTITY",
    "READS",
    "WRITES",
    "PUBLISHES_EVENT",
    "LISTENS_TO_EVENT",
    "CONFIGURED_BY",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointContextRequest {
    pub project: String,
    #[serde(default)]
    pub route_stable_id: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub controller_method_stable_id: Option<String>,
    #[serde(default)]
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub suppression_policy: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetEvidenceRequest {
    pub project: String,
    pub references: Vec<EvidenceRequestRef>,
    #[serde(default)]
    pub context_lines: Option<u32>,
    #[serde(default)]
    pub max_total_lines: Option<u32>,
    #[serde(default)]
    pub token_budget: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRequestRef {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
}

impl QueryService<'_> {
    /// Returns controller-to-data static structure for one Spring HTTP endpoint.
    ///
    /// # Errors
    ///
    /// Returns lookup, graph, cursor, or budget validation errors.
    #[allow(clippy::missing_panics_doc, clippy::too_many_lines)]
    pub fn get_endpoint_context(
        &self,
        request: &EndpointContextRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let budget = self.budget(request.token_budget)?;
        let status = self.database.status(&request.project)?;
        let max_depth = request
            .max_depth
            .unwrap_or(4)
            .min(self.config.maximum_query_depth);
        if max_depth == 0 {
            return Err(GraphineError::InvalidArgument(
                "max_depth must be positive".to_owned(),
            ));
        }
        let route = self.resolve_route(&status, request)?;
        let binding = CursorBinding::new(
            &status,
            "endpoint_context",
            Some(&route.stable_id),
            format!(
                "depth={max_depth};include={:?};suppression={}",
                request.include,
                request
                    .suppression_policy
                    .as_deref()
                    .unwrap_or(DEFAULT_SUPPRESSION_POLICY)
            ),
        );
        let evidence_offset = decode_cursor(request.cursor.as_deref(), &binding)?;
        let handlers = self.edge_targets(&status, &route.stable_id, "HANDLED_BY")?;
        let mut ambiguities = Vec::new();
        if handlers.len() > 1 || route.confidence == Confidence::Ambiguous {
            ambiguities.push(format!(
                "{} statically valid handlers map to this normalized route",
                handlers.len()
            ));
        }
        let mut unresolved = route.unresolved.clone();
        let mut flows = Vec::new();
        let mut branch_points = BTreeSet::new();
        let mut dependencies = Vec::new();
        let mut reads = BTreeSet::new();
        let mut writes = BTreeSet::new();
        let mut events = Vec::new();
        let mut configuration = BTreeSet::new();
        let mut evidence = Vec::new();
        add_node_evidence(&route, &mut evidence);
        let mut suppressed = 0_u64;
        for handler in &handlers {
            add_node_evidence(handler, &mut evidence);
            unresolved.extend(handler.unresolved.clone());
            let flow = self.endpoint_flow(
                &status,
                handler,
                max_depth,
                request
                    .suppression_policy
                    .as_deref()
                    .unwrap_or(DEFAULT_SUPPRESSION_POLICY),
                &mut branch_points,
                &mut dependencies,
                &mut reads,
                &mut writes,
                &mut events,
                &mut configuration,
                &mut evidence,
                &mut unresolved,
                &mut suppressed,
            )?;
            flows.extend(flow);
        }
        evidence.sort();
        evidence.dedup();
        let evidence_total = evidence.len();
        let evidence_ids: Vec<_> = evidence
            .iter()
            .skip(evidence_offset)
            .take(self.config.maximum_result_count as usize)
            .map(|location| evidence_id(&status, location))
            .collect();
        let request_pack = self.request_pack(&status, &route)?;
        let handler_pack: Vec<_> = handlers
            .iter()
            .map(|handler| {
                json!({
                    "stable_id": handler.stable_id,
                    "type": declaring_type_name(&handler.stable_id),
                    "method": handler.simple_name,
                    "signature": handler.stable_id.split_once(':').map_or(handler.qualified_name.as_str(), |(_, signature)| signature),
                    "confidence": handler.confidence,
                })
            })
            .collect();
        let mut result = json!({
            "endpoint": {
                "stable_id": route.stable_id,
                "method": route.metadata.get("http_method").and_then(Value::as_str).unwrap_or("UNKNOWN"),
                "path": route.metadata.get("path").and_then(Value::as_str).unwrap_or(&route.qualified_name),
                "confidence": route.confidence,
            },
            "handlers": handler_pack,
            "request": request_pack,
            "flow": {
                "paths": flows,
                "branch_points": branch_points,
                "interpretation": "possible static structural flow; not a runtime execution trace"
            },
            "data_access": {"writes": writes, "reads": reads},
            "events": events,
            "configuration": configuration,
            "dependencies": dependencies,
            "evidence_refs": evidence_ids,
            "suppressed_fact_count": suppressed,
            "suppression_policy": request.suppression_policy.as_deref().unwrap_or(DEFAULT_SUPPRESSION_POLICY),
        });
        if !request.include.is_empty() {
            let object = result.as_object_mut().expect("object literal");
            if !request.include.iter().any(|value| value == "validation") {
                if let Some(request_pack) = object.get_mut("request").and_then(Value::as_object_mut)
                {
                    request_pack.remove("validation");
                }
            }
            for (include, key) in [
                ("dependencies", "dependencies"),
                ("data_access", "data_access"),
                ("events", "events"),
                ("configuration", "configuration"),
            ] {
                if !request.include.iter().any(|value| value == include) {
                    object.remove(key);
                }
            }
        }
        let mut next_actions = Vec::new();
        if evidence_total > 0 {
            next_actions.push(json!({
                "tool": "get_evidence",
                "reason": "Verify selected endpoint-flow claims without reading full files",
                "arguments": {"project": request.project, "references": evidence_ids.iter().take(3).map(|id| json!({"id": id})).collect::<Vec<_>>()}
            }));
        }
        if handlers.len() > 1 {
            next_actions.push(json!({
                "tool": "get_symbol_context",
                "reason": "Inspect each ambiguous handler before choosing one",
                "arguments": {"project": request.project, "stable_id": handlers[0].stable_id}
            }));
        }
        let has_more = evidence_offset.saturating_add(evidence_ids.len()) < evidence_total;
        let cursor = has_more
            .then(|| encode_cursor(&binding, evidence_offset.saturating_add(evidence_ids.len())));
        let mut response = compact_agent_result(
            &status,
            &mut result,
            unresolved,
            ambiguities,
            next_actions,
            Pagination { has_more, cursor },
            budget,
            format!("endpoint max_depth={max_depth}"),
            &[
                "metadata",
                "secondary_calls",
                "evidence_refs",
                "dependencies",
                "events",
                "configuration",
                "request.validation",
                "data_access.reads",
                "data_access.writes",
                "flow.paths",
            ],
        )?;
        let returned_evidence = response
            .result
            .get("evidence_refs")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        response.pagination.has_more =
            evidence_offset.saturating_add(returned_evidence) < evidence_total;
        response.pagination.cursor = response
            .pagination
            .has_more
            .then(|| encode_cursor(&binding, evidence_offset.saturating_add(returned_evidence)));
        response.budget.estimated_tokens = estimated_tokens(&response);
        Ok(response)
    }

    /// Returns bounded, line-numbered source evidence from the registered repository.
    ///
    /// # Errors
    ///
    /// Rejects stale IDs, escaped/sensitive/binary paths, and unsafe ranges.
    #[allow(clippy::too_many_lines)]
    pub fn get_evidence(
        &self,
        request: &GetEvidenceRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let budget = self.budget(request.token_budget)?;
        if request.references.is_empty()
            || request.references.len() > self.config.maximum_result_count as usize
        {
            return Err(GraphineError::InvalidArgument(
                "references must be a non-empty bounded list".to_owned(),
            ));
        }
        let status = self.database.status(&request.project)?;
        let context_lines = request.context_lines.unwrap_or(2).min(20);
        let max_total_lines = request
            .max_total_lines
            .unwrap_or(80)
            .min(self.config.maximum_evidence_lines);
        let indexed = self.database.evidence_locations(&status)?;
        let by_id: BTreeMap<_, _> = indexed
            .iter()
            .map(|location| (evidence_id(&status, location), location.clone()))
            .collect();
        let mut locations = Vec::new();
        for reference in &request.references {
            let location = match (&reference.id, &reference.file) {
                (Some(id), None) => resolve_evidence_id(&status, id, &by_id)?,
                (None, Some(file)) => EvidenceLocation {
                    file_path: file.clone(),
                    start_line: reference.start_line.ok_or_else(|| {
                        GraphineError::InvalidArgument("start_line is required".to_owned())
                    })?,
                    end_line: reference.end_line.ok_or_else(|| {
                        GraphineError::InvalidArgument("end_line is required".to_owned())
                    })?,
                },
                _ => {
                    return Err(GraphineError::InvalidArgument(
                        "each reference must contain either id or file/range".to_owned(),
                    ));
                }
            };
            if location.start_line == 0
                || location.end_line < location.start_line
                || location.end_line - location.start_line + 1 > self.config.maximum_evidence_lines
            {
                return Err(GraphineError::InvalidArgument(
                    "evidence range is invalid or too large".to_owned(),
                ));
            }
            locations.push(location);
        }
        let required_lines = locations
            .iter()
            .try_fold(0_u32, |total, location| {
                total.checked_add(location.end_line - location.start_line + 1)
            })
            .ok_or_else(|| GraphineError::InvalidArgument("evidence ranges overflow".to_owned()))?;
        if required_lines > max_total_lines {
            return Err(GraphineError::InvalidArgument(
                "max_total_lines must fit every requested range".to_owned(),
            ));
        }
        let mut context_remaining = max_total_lines - required_lines;
        let mut snippets = Vec::new();
        let mut total_lines = 0_u32;
        let mut total_bytes = 0_usize;
        let mut truncated = false;
        for location in locations {
            let canonical = validated_source_path(
                &status.project.canonical_root,
                &location.file_path,
                &self.config.evidence_excluded_paths,
            )?;
            let bytes = fs::read(&canonical).map_err(|_| GraphineError::InvalidPath)?;
            if bytes.len() > self.config.maximum_evidence_bytes || bytes.contains(&0) {
                return Err(GraphineError::InvalidPath);
            }
            let source = String::from_utf8(bytes).map_err(|_| GraphineError::InvalidPath)?;
            let lines: Vec<_> = source.lines().collect();
            if location.end_line as usize > lines.len() {
                return Err(GraphineError::InvalidArgument(
                    "evidence range exceeds file length".to_owned(),
                ));
            }
            let requested_lines = numbered_lines(&lines, location.start_line, location.end_line);
            let before_start = location.start_line.saturating_sub(context_lines).max(1);
            let before_candidates =
                numbered_lines(&lines, before_start, location.start_line.saturating_sub(1));
            let file_end = u32::try_from(lines.len()).unwrap_or(u32::MAX);
            let after_candidates = numbered_lines(
                &lines,
                location.end_line.saturating_add(1),
                location
                    .end_line
                    .saturating_add(context_lines)
                    .min(file_end),
            );
            let before_take = before_candidates.len().min(context_remaining as usize);
            let context_before =
                before_candidates[before_candidates.len() - before_take..].to_vec();
            context_remaining = context_remaining
                .saturating_sub(u32::try_from(context_before.len()).unwrap_or(u32::MAX));
            let after_take = after_candidates.len().min(context_remaining as usize);
            let context_after = after_candidates[..after_take].to_vec();
            context_remaining = context_remaining
                .saturating_sub(u32::try_from(context_after.len()).unwrap_or(u32::MAX));
            truncated |=
                before_take < before_candidates.len() || after_take < after_candidates.len();
            total_bytes = total_bytes.saturating_add(
                requested_lines
                    .iter()
                    .chain(&context_before)
                    .chain(&context_after)
                    .filter_map(|line| line.get("text").and_then(Value::as_str))
                    .map(str::len)
                    .sum::<usize>(),
            );
            if total_bytes > self.config.maximum_evidence_bytes {
                return Err(GraphineError::InvalidPath);
            }
            total_lines = total_lines.saturating_add(
                u32::try_from(requested_lines.len() + context_before.len() + context_after.len())
                    .unwrap_or(u32::MAX),
            );
            let returned_start = context_before
                .first()
                .and_then(|line| line.get("line"))
                .and_then(Value::as_u64)
                .and_then(|line| u32::try_from(line).ok())
                .unwrap_or(location.start_line);
            let returned_end = context_after
                .last()
                .and_then(|line| line.get("line"))
                .and_then(Value::as_u64)
                .and_then(|line| u32::try_from(line).ok())
                .unwrap_or(location.end_line);
            let stable_id = by_id.iter().find_map(|(id, indexed_location)| {
                (indexed_location == &location).then(|| id.clone())
            });
            snippets.push(json!({
                "id": stable_id,
                "file": location.file_path,
                "requested": {"start_line": location.start_line, "end_line": location.end_line},
                "returned": {"start_line": returned_start, "end_line": returned_end},
                "requested_lines": requested_lines,
                "context_before": context_before,
                "context_after": context_after,
            }));
        }
        let mut result = json!({
            "snippets": snippets,
            "total_lines": total_lines,
            "total_bytes": total_bytes,
            "text_only": true,
        });
        let mut envelope = compact_agent_result(
            &status,
            &mut result,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Pagination::default(),
            budget,
            format!("max_total_lines={max_total_lines}"),
            &[
                "snippets.context_after",
                "snippets.context_before",
                "snippets.requested_lines",
                "snippets",
            ],
        )?;
        if truncated {
            "truncated".clone_into(&mut envelope.completeness.status);
            envelope.completeness.evidence_truncated = true;
            envelope.complete = false;
            envelope.budget.truncated = true;
        }
        Ok(envelope)
    }

    fn resolve_route(
        &self,
        status: &IndexStatus,
        request: &EndpointContextRequest,
    ) -> Result<NodeRecord, GraphineError> {
        let selectors = usize::from(request.route_stable_id.is_some())
            + usize::from(request.controller_method_stable_id.is_some())
            + usize::from(request.method.is_some() || request.path.is_some());
        if selectors != 1 {
            return Err(GraphineError::InvalidArgument(
                "provide exactly one route selector".to_owned(),
            ));
        }
        if let Some(id) = &request.route_stable_id {
            let node = self.database.node_in_status(status, id)?;
            return (node.kind == "ROUTE")
                .then_some(node)
                .ok_or(GraphineError::InvalidArgument(
                    "stable ID is not a route".to_owned(),
                ));
        }
        if let Some(handler) = &request.controller_method_stable_id {
            let routes = self.database.direct_edges(
                status,
                handler,
                true,
                &["HANDLED_BY".to_owned()],
                self.config.maximum_result_count,
            )?;
            if routes.len() != 1 {
                return Err(if routes.is_empty() {
                    GraphineError::SymbolNotFound
                } else {
                    GraphineError::AmbiguousSymbol
                });
            }
            return self
                .database
                .node_in_status(status, &routes[0].source_stable_id);
        }
        let method = request.method.as_deref().ok_or_else(|| {
            GraphineError::InvalidArgument("method and path are both required".to_owned())
        })?;
        let path = request.path.as_deref().ok_or_else(|| {
            GraphineError::InvalidArgument("method and path are both required".to_owned())
        })?;
        let id = format!(
            "route:{}:{}",
            method.trim().to_ascii_uppercase(),
            normalize_route_path(path)
        );
        self.database.node_in_status(status, &id)
    }

    fn edge_targets(
        &self,
        status: &IndexStatus,
        stable_id: &str,
        kind: &str,
    ) -> Result<Vec<NodeRecord>, GraphineError> {
        self.database
            .direct_edges(
                status,
                stable_id,
                false,
                &[kind.to_owned()],
                self.config.maximum_result_count,
            )?
            .into_iter()
            .map(|edge| self.database.node_in_status(status, &edge.target_stable_id))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn endpoint_flow(
        &self,
        status: &IndexStatus,
        handler: &NodeRecord,
        max_depth: u32,
        suppression_policy: &str,
        branch_points: &mut BTreeSet<String>,
        dependencies: &mut Vec<Value>,
        reads: &mut BTreeSet<String>,
        writes: &mut BTreeSet<String>,
        events: &mut Vec<Value>,
        configuration: &mut BTreeSet<String>,
        evidence: &mut Vec<EvidenceLocation>,
        unresolved: &mut Vec<String>,
        suppressed: &mut u64,
    ) -> Result<Vec<Vec<String>>, GraphineError> {
        let mut queue =
            VecDeque::from([(handler.clone(), vec![handler.qualified_name.clone()], 0_u32)]);
        let mut paths = Vec::new();
        let mut seen_depth = BTreeMap::<String, u32>::new();
        while let Some((node, path, depth)) = queue.pop_front() {
            if depth >= max_depth || paths.len() >= self.config.maximum_result_count as usize {
                paths.push(path);
                continue;
            }
            self.collect_injections(status, &node, dependencies, evidence)?;
            let edges = self.database.direct_edges(
                status,
                &node.stable_id,
                false,
                &FLOW_EDGE_KINDS
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                self.config.maximum_result_count,
            )?;
            let mut continuations = Vec::new();
            for edge in edges {
                let target = self
                    .database
                    .node_in_status(status, &edge.target_stable_id)?;
                if should_suppress(&target, suppression_policy) {
                    *suppressed = suppressed.saturating_add(1);
                    continue;
                }
                self.collect_edge_semantics(
                    status,
                    &edge,
                    &target,
                    reads,
                    writes,
                    events,
                    configuration,
                    evidence,
                )?;
                add_node_evidence(&target, evidence);
                unresolved.extend(target.unresolved.clone());
                if edge.kind == "CALLS" && is_application_node(&target) {
                    continuations.push(target);
                }
            }
            if continuations.len() > 1 {
                branch_points.insert(node.qualified_name.clone());
            }
            if continuations.is_empty() {
                paths.push(path);
                continue;
            }
            continuations.sort_by(|left, right| left.stable_id.cmp(&right.stable_id));
            for next in continuations {
                let next_depth = depth + 1;
                if seen_depth
                    .get(&next.stable_id)
                    .is_some_and(|seen| *seen <= next_depth)
                {
                    continue;
                }
                seen_depth.insert(next.stable_id.clone(), next_depth);
                let mut next_path = path.clone();
                next_path.push(next.qualified_name.clone());
                queue.push_back((next, next_path, next_depth));
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn collect_injections(
        &self,
        status: &IndexStatus,
        method: &NodeRecord,
        dependencies: &mut Vec<Value>,
        evidence: &mut Vec<EvidenceLocation>,
    ) -> Result<(), GraphineError> {
        let owner_id = method
            .metadata
            .get("declaring_type")
            .and_then(Value::as_str)
            .map_or_else(
                || format!("type:{}", declaring_type_name(&method.stable_id)),
                str::to_owned,
            );
        let owner = match self.database.node_in_status(status, &owner_id) {
            Ok(owner) => owner,
            Err(GraphineError::SymbolNotFound) => return Ok(()),
            Err(error) => return Err(error),
        };
        for edge in self.database.direct_edges(
            status,
            &owner.stable_id,
            false,
            &["INJECTS".to_owned()],
            self.config.maximum_result_count,
        )? {
            let target = self
                .database
                .node_in_status(status, &edge.target_stable_id)?;
            dependencies.push(json!({
                "from": owner.qualified_name,
                "to": target.metadata.get("declared_type").cloned().unwrap_or_else(|| json!(target.qualified_name)),
                "bean": target.simple_name,
                "kind": edge.kind,
                "confidence": edge.confidence,
            }));
            add_edge_evidence(self.database, status, &edge, evidence)?;
        }
        dependencies.sort_by_key(std::string::ToString::to_string);
        dependencies.dedup();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_edge_semantics(
        &self,
        status: &IndexStatus,
        edge: &EdgeRecord,
        target: &NodeRecord,
        reads: &mut BTreeSet<String>,
        writes: &mut BTreeSet<String>,
        events: &mut Vec<Value>,
        configuration: &mut BTreeSet<String>,
        evidence: &mut Vec<EvidenceLocation>,
    ) -> Result<(), GraphineError> {
        match edge.kind.as_str() {
            "CALLS" => {
                if let Some(domain) = self.repository_domain(status, target) {
                    let name = target.simple_name.to_ascii_lowercase();
                    if name.starts_with("save")
                        || name.starts_with("delete")
                        || name.starts_with("remove")
                    {
                        writes.insert(domain);
                    } else if name.starts_with("find")
                        || name.starts_with("read")
                        || name.starts_with("get")
                        || name.starts_with("list")
                        || name.starts_with("count")
                        || name.starts_with("exists")
                    {
                        reads.insert(domain);
                    }
                }
            }
            "READS_ENTITY" | "READS" => {
                reads.insert(target.qualified_name.clone());
            }
            "WRITES_ENTITY" | "WRITES" => {
                writes.insert(target.qualified_name.clone());
            }
            "PUBLISHES_EVENT" | "LISTENS_TO_EVENT" => events.push(json!({
                "kind": edge.kind,
                "event": target.qualified_name,
                "confidence": edge.confidence,
                "runtime_delivery": "not confirmed",
            })),
            "CONFIGURED_BY" => {
                configuration.insert(target.qualified_name.clone());
            }
            _ => {}
        }
        add_edge_evidence(self.database, status, edge, evidence)
    }

    fn repository_domain(&self, status: &IndexStatus, method: &NodeRecord) -> Option<String> {
        let owner = declaring_type_name(&method.stable_id);
        if !owner.ends_with("Repository") {
            return None;
        }
        if method.simple_name.starts_with("save") {
            if let Some(parameter) = method
                .metadata
                .get("parameter_types")
                .and_then(Value::as_array)
                .and_then(|parameters| parameters.first())
                .and_then(Value::as_str)
            {
                if self
                    .database
                    .node_in_status(status, &format!("type:{parameter}"))
                    .is_ok()
                    || self
                        .database
                        .node_in_status(status, &format!("entity:{parameter}"))
                        .is_ok()
                {
                    return Some(parameter.to_owned());
                }
            }
        }
        let domain = owner.strip_suffix("Repository")?;
        (self
            .database
            .node_in_status(status, &format!("type:{domain}"))
            .is_ok()
            || self
                .database
                .node_in_status(status, &format!("entity:{domain}"))
                .is_ok())
        .then(|| domain.to_owned())
    }

    #[allow(clippy::unnecessary_wraps)]
    fn request_pack(
        &self,
        status: &IndexStatus,
        route: &NodeRecord,
    ) -> Result<Value, GraphineError> {
        let parameters = route
            .metadata
            .get("handler_parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let body = parameters.iter().find(|parameter| {
            parameter.get("binding_kind").and_then(Value::as_str) == Some("REQUEST_BODY")
        });
        let body_type = body
            .and_then(|parameter| parameter.get("java_type"))
            .and_then(Value::as_str);
        let mut validation = body
            .and_then(|parameter| parameter.get("validation"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(body_type) = body_type {
            if let Ok(dto) = self
                .database
                .node_in_status(status, &format!("type:{body_type}"))
            {
                validation.extend(
                    dto.metadata
                        .get("validation_components")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
        Ok(json!({
            "body_type": body_type,
            "parameters": parameters,
            "validation": validation,
        }))
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::unnecessary_wraps
)]
pub(super) fn compact_agent_result(
    status: &IndexStatus,
    result: &mut Value,
    mut unresolved: Vec<String>,
    mut ambiguities: Vec<String>,
    mut next_actions: Vec<Value>,
    pagination: Pagination,
    requested: u32,
    scope: String,
    prune_order: &[&str],
) -> Result<ResponseEnvelope, GraphineError> {
    unresolved.sort();
    unresolved.dedup();
    ambiguities.sort();
    ambiguities.dedup();
    let unresolved_count = u64::try_from(unresolved.len()).unwrap_or(u64::MAX);
    let ambiguity_count = u64::try_from(ambiguities.len()).unwrap_or(u64::MAX);
    let mut envelope = ResponseEnvelope {
        project: status.project.display_name.clone(),
        generation: status.active_generation,
        stale: status.stale,
        partial: status.partial,
        completeness: Completeness {
            status: if status.partial {
                "partial"
            } else if !ambiguities.is_empty() {
                "ambiguous"
            } else if !unresolved.is_empty() {
                "unresolved"
            } else {
                "complete"
            }
            .to_owned(),
            scope,
            facts_truncated: false,
            evidence_truncated: false,
            uncertainty_truncated: false,
        },
        result: result.clone(),
        complete: !status.stale
            && !status.partial
            && unresolved.is_empty()
            && ambiguities.is_empty(),
        summary: Value::Null,
        facts: Vec::new(),
        unresolved,
        ambiguities,
        uncertainty: UncertaintyState {
            unresolved_count,
            ambiguity_count,
            ..UncertaintyState::default()
        },
        evidence_refs: Vec::new(),
        pagination,
        next_actions: next_actions.clone(),
        budget: Budget {
            requested_tokens: requested,
            estimated_tokens: 0,
            truncated: false,
        },
        truncation: TruncationState::default(),
    };
    envelope.budget.estimated_tokens = estimated_tokens(&envelope);
    let mut changed = false;
    while envelope.budget.estimated_tokens > requested {
        let mut removed = false;
        for path in prune_order {
            if truncate_array_at_path(&mut envelope.result, path) {
                removed = true;
                changed = true;
                if path.contains("evidence") || path.contains("snippets") {
                    envelope.completeness.evidence_truncated = true;
                } else {
                    envelope.completeness.facts_truncated = true;
                }
                break;
            }
        }
        if !removed && !envelope.next_actions.is_empty() {
            envelope.next_actions.pop();
            next_actions.pop();
            changed = true;
            removed = true;
        }
        if !removed && envelope.unresolved.len() > 1 {
            envelope.unresolved.pop();
            envelope.completeness.uncertainty_truncated = true;
            envelope.uncertainty.unresolved_truncated = Some(true);
            changed = true;
            removed = true;
        }
        if !removed && envelope.ambiguities.len() > 1 {
            envelope.ambiguities.pop();
            envelope.completeness.uncertainty_truncated = true;
            envelope.uncertainty.ambiguities_truncated = Some(true);
            changed = true;
            removed = true;
        }
        if !removed && envelope.result != minimal_result(&envelope.result) {
            envelope.result = minimal_result(&envelope.result);
            envelope.completeness.facts_truncated = true;
            changed = true;
            removed = true;
        }
        if !removed && envelope.completeness.scope != "requested" {
            "requested".clone_into(&mut envelope.completeness.scope);
            changed = true;
            removed = true;
        }
        if !removed {
            break;
        }
        envelope.budget.estimated_tokens = estimated_tokens(&envelope);
    }
    if changed {
        envelope.budget.truncated = true;
        envelope.complete = false;
        if envelope.completeness.status == "complete" {
            "truncated".clone_into(&mut envelope.completeness.status);
        }
    }
    envelope.budget.estimated_tokens = estimated_tokens(&envelope);
    Ok(envelope)
}

fn minimal_result(result: &Value) -> Value {
    if let Some(endpoint) = result.get("endpoint") {
        return json!({
            "endpoint": endpoint,
            "handlers": result.get("handlers").and_then(Value::as_array).and_then(|handlers| handlers.first()).cloned().into_iter().collect::<Vec<_>>()
        });
    }
    if let Some(symbol) = result.get("symbol") {
        return json!({"symbol": {
            "stable_id": symbol.get("stable_id"),
            "kind": symbol.get("kind"),
            "confidence": symbol.get("confidence"),
        }});
    }
    if result.get("query").is_some() {
        return json!({
            "query": result.get("query"),
            "matched": result.get("matched"),
            "results": result.get("results").and_then(Value::as_array).and_then(|values| values.first()).cloned().into_iter().collect::<Vec<_>>()
        });
    }
    if let Some(start) = result.get("start") {
        return json!({"start": start, "paths": []});
    }
    if let Some(summary) = result.get("summary") {
        return json!({"summary": summary});
    }
    if result.get("snippets").is_some() {
        return json!({"snippets": []});
    }
    result.clone()
}

fn truncate_array_at_path(value: &mut Value, path: &str) -> bool {
    let segments: Vec<_> = path.split('.').collect();
    truncate_segments(value, &segments)
}

fn truncate_segments(value: &mut Value, segments: &[&str]) -> bool {
    if segments.is_empty() {
        return false;
    }
    if let Some(array) = value.as_array_mut() {
        for item in array.iter_mut().rev() {
            if truncate_segments(item, segments) {
                return true;
            }
        }
        return false;
    }
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    if segments.len() == 1 {
        let key = segments[0];
        let Some(array) = object.get_mut(key).and_then(Value::as_array_mut) else {
            return false;
        };
        if array.is_empty() {
            return false;
        }
        array.pop();
        let count_key = format!("{key}_omitted_count");
        let omitted = object.get(&count_key).and_then(Value::as_u64).unwrap_or(0) + 1;
        object.insert(count_key, json!(omitted));
        object.insert(format!("{key}_truncated"), json!(true));
        true
    } else {
        object
            .get_mut(segments[0])
            .is_some_and(|nested| truncate_segments(nested, &segments[1..]))
    }
}

pub(super) fn evidence_id(status: &IndexStatus, location: &EvidenceLocation) -> String {
    let canonical = format!(
        "{}\0{}\0{}\0{}",
        status.active_generation.unwrap_or_default(),
        location.file_path,
        location.start_line,
        location.end_line
    );
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in canonical.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(
        "evidence:{}:{hash:016x}",
        status.active_generation.unwrap_or_default()
    )
}

fn resolve_evidence_id(
    status: &IndexStatus,
    id: &str,
    indexed: &BTreeMap<String, EvidenceLocation>,
) -> Result<EvidenceLocation, GraphineError> {
    let mut parts = id.split(':');
    if parts.next() != Some("evidence")
        || parts
            .next()
            .and_then(|generation| generation.parse::<i64>().ok())
            != status.active_generation
        || parts.next().is_none()
        || parts.next().is_some()
    {
        return Err(GraphineError::GenerationConflict);
    }
    indexed.get(id).cloned().ok_or(GraphineError::InvalidPath)
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn validated_source_path(
    root: &Path,
    relative: &str,
    excluded: &[String],
) -> Result<std::path::PathBuf, GraphineError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GraphineError::InvalidPath);
    }
    let normalized = relative.replace('\\', "/").to_ascii_lowercase();
    let file_name = relative_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let sensitive = file_name == ".env"
        || file_name.starts_with(".env.")
        || file_name.contains("credential")
        || file_name.contains("secret")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
        || normalized.contains("/.ssh/")
        || excluded.iter().any(|entry| {
            let entry = entry.replace('\\', "/").to_ascii_lowercase();
            normalized == entry || normalized.starts_with(&format!("{entry}/"))
        });
    if sensitive {
        return Err(GraphineError::InvalidPath);
    }
    let canonical_root = fs::canonicalize(root).map_err(|_| GraphineError::InvalidPath)?;
    let canonical = fs::canonicalize(canonical_root.join(relative_path))
        .map_err(|_| GraphineError::InvalidPath)?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return Err(GraphineError::InvalidPath);
    }
    Ok(canonical)
}

fn normalize_route_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/" {
        "/".to_owned()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    }
}

fn numbered_lines(lines: &[&str], start: u32, end: u32) -> Vec<Value> {
    if end < start {
        return Vec::new();
    }
    (start..=end)
        .filter_map(|line| {
            lines
                .get(line.saturating_sub(1) as usize)
                .map(|text| json!({"line": line, "text": text}))
        })
        .collect()
}

fn declaring_type_name(stable_id: &str) -> String {
    stable_id
        .split_once(':')
        .map_or(stable_id, |(_, value)| value)
        .split_once('#')
        .map_or(stable_id, |(owner, _)| owner)
        .to_owned()
}

fn is_application_node(node: &NodeRecord) -> bool {
    node.file_path.is_some()
        && !node.stable_id.starts_with("method:java.")
        && !node.stable_id.starts_with("method:javax.")
        && !node.stable_id.starts_with("method:jakarta.")
        && !node.stable_id.starts_with("method:org.springframework.")
}

fn should_suppress(node: &NodeRecord, policy: &str) -> bool {
    if policy == "none" {
        return false;
    }
    if !is_application_node(node) {
        return true;
    }
    let name = node.simple_name.as_str();
    let accessor = (name.starts_with("get") || name.starts_with("set") || name.starts_with("is"))
        && node.kind == "METHOD";
    accessor
        || matches!(
            name,
            "toString"
                | "hashCode"
                | "equals"
                | "requireNonNull"
                | "debug"
                | "info"
                | "warn"
                | "error"
                | "trace"
        )
        || node
            .metadata
            .get("generated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn add_node_evidence(node: &NodeRecord, evidence: &mut Vec<EvidenceLocation>) {
    if let (Some(file_path), Some(start_line), Some(end_line)) =
        (&node.file_path, node.start_line, node.end_line)
    {
        evidence.push(EvidenceLocation {
            file_path: file_path.clone(),
            start_line,
            end_line,
        });
    }
}

fn add_edge_evidence(
    database: &graphine_index::Database,
    status: &IndexStatus,
    edge: &EdgeRecord,
    evidence: &mut Vec<EvidenceLocation>,
) -> Result<(), GraphineError> {
    evidence.extend(
        database
            .relationship_occurrences(status, edge, 8)?
            .into_iter()
            .map(|occurrence| EvidenceLocation {
                file_path: occurrence.file_path,
                start_line: occurrence.start_line,
                end_line: occurrence.end_line,
            }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphine_index::Database;
    use graphine_protocol::{
        EdgeOccurrence, GraphineConfig, SyntheticEdge, SyntheticGraph, SyntheticNode,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn node(
        id: &str,
        kind: &str,
        qualified: &str,
        file: Option<&str>,
        confidence: Confidence,
        metadata: Value,
    ) -> SyntheticNode {
        SyntheticNode {
            stable_id: id.to_owned(),
            kind: kind.to_owned(),
            qualified_name: qualified.to_owned(),
            simple_name: id.split(['#', ':']).next_back().map(str::to_owned),
            module_name: Some("app".to_owned()),
            package_name: Some("app.event".to_owned()),
            file_path: file.map(str::to_owned),
            start_line: file.map(|_| 1),
            end_line: file.map(|_| 4),
            confidence,
            provenance: "phase4-test".to_owned(),
            metadata,
            unresolved: Vec::new(),
        }
    }

    fn edge(
        source: &str,
        target: &str,
        kind: &str,
        confidence: Confidence,
        file: Option<&str>,
    ) -> SyntheticEdge {
        SyntheticEdge {
            source_stable_id: source.to_owned(),
            target_stable_id: target.to_owned(),
            kind: kind.to_owned(),
            confidence,
            provenance: "phase4-test".to_owned(),
            metadata: json!({}),
            occurrences: file
                .into_iter()
                .map(|file_path| EdgeOccurrence {
                    file_path: file_path.to_owned(),
                    start_line: 2,
                    end_line: 2,
                    metadata: json!({}),
                })
                .collect(),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn fixture() -> (Database, GraphineConfig, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "graphine-phase4-query-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        for file in [
            "Controller.java",
            "Service.java",
            "Repository.java",
            "Event.java",
            "Dto.java",
        ] {
            fs::write(root.join(file), "line1\nline2\nline3\nline4\n").unwrap();
        }
        fs::write(root.join(".env"), "SECRET=value\n").unwrap();
        let route = "route:POST:/events";
        let handler = "method:app.event.Controller#create(app.event.Dto)";
        let duplicate = "method:app.event.Controller#duplicate(app.event.Dto)";
        let service = "method:app.event.Service#create(app.event.Dto)";
        let audit = "method:app.event.Service#audit()";
        let repository = "method:app.event.Repository#save(app.event.Event)";
        let external = "method:java.util.Objects#requireNonNull(java.lang.Object)";
        let nodes = vec![
            node(
                "type:app.event.Controller",
                "TYPE",
                "app.event.Controller",
                Some("Controller.java"),
                Confidence::CompilerResolved,
                json!({"annotations":["RestController"]}),
            ),
            node(
                handler,
                "METHOD",
                "app.event.Controller#create",
                Some("Controller.java"),
                Confidence::CompilerResolved,
                json!({"declaring_type":"type:app.event.Controller"}),
            ),
            node(
                duplicate,
                "METHOD",
                "app.event.Controller#duplicate",
                Some("Controller.java"),
                Confidence::CompilerResolved,
                json!({"declaring_type":"type:app.event.Controller"}),
            ),
            node(
                route,
                "ROUTE",
                "POST /events",
                Some("Controller.java"),
                Confidence::Ambiguous,
                json!({"http_method":"POST","path":"/events","handler_parameters":[{"binding_kind":"REQUEST_BODY","java_type":"app.event.Dto","validation":["Valid"]}]}),
            ),
            node(
                "type:app.event.Service",
                "TYPE",
                "app.event.Service",
                Some("Service.java"),
                Confidence::CompilerResolved,
                json!({}),
            ),
            node(
                service,
                "METHOD",
                "app.event.Service#create",
                Some("Service.java"),
                Confidence::CompilerResolved,
                json!({"declaring_type":"type:app.event.Service"}),
            ),
            node(
                audit,
                "METHOD",
                "app.event.Service#audit",
                Some("Service.java"),
                Confidence::CompilerResolved,
                json!({"declaring_type":"type:app.event.Service"}),
            ),
            node(
                "bean:eventService",
                "BEAN",
                "eventService",
                Some("Service.java"),
                Confidence::FrameworkResolved,
                json!({"declared_type":"app.event.Service","stereotype":"SERVICE"}),
            ),
            node(
                "bean:one",
                "BEAN",
                "one",
                Some("Service.java"),
                Confidence::Ambiguous,
                json!({"declared_type":"app.event.Payment"}),
            ),
            node(
                "bean:two",
                "BEAN",
                "two",
                Some("Service.java"),
                Confidence::Ambiguous,
                json!({"declared_type":"app.event.Payment"}),
            ),
            node(
                "type:app.event.Repository",
                "REPOSITORY",
                "app.event.Repository",
                Some("Repository.java"),
                Confidence::FrameworkResolved,
                json!({"domain_type":"app.event.Event"}),
            ),
            node(
                repository,
                "METHOD",
                "app.event.Repository#save",
                Some("Repository.java"),
                Confidence::FrameworkResolved,
                json!({"declaring_type":"type:app.event.Repository"}),
            ),
            node(
                "entity:app.event.Event",
                "ENTITY",
                "app.event.Event",
                Some("Event.java"),
                Confidence::FrameworkResolved,
                json!({}),
            ),
            node(
                "type:app.event.Created",
                "EVENT_TYPE",
                "app.event.Created",
                Some("Event.java"),
                Confidence::FrameworkResolved,
                json!({}),
            ),
            node(
                "type:app.event.Dto",
                "TYPE",
                "app.event.Dto",
                Some("Dto.java"),
                Confidence::FrameworkResolved,
                json!({"validation_components":[{"component":"name","validation":["NotBlank"]}]}),
            ),
            node(
                external,
                "METHOD",
                "java.util.Objects#requireNonNull",
                None,
                Confidence::CompilerResolved,
                json!({"external":true}),
            ),
        ];
        let edges = vec![
            edge(
                route,
                handler,
                "HANDLED_BY",
                Confidence::FrameworkResolved,
                Some("Controller.java"),
            ),
            edge(
                route,
                duplicate,
                "HANDLED_BY",
                Confidence::FrameworkResolved,
                Some("Controller.java"),
            ),
            edge(
                "type:app.event.Controller",
                route,
                "EXPOSES_ROUTE",
                Confidence::FrameworkResolved,
                Some("Controller.java"),
            ),
            edge(
                "type:app.event.Controller",
                "bean:eventService",
                "INJECTS",
                Confidence::FrameworkResolved,
                Some("Controller.java"),
            ),
            edge(
                handler,
                service,
                "CALLS",
                Confidence::CompilerResolved,
                Some("Controller.java"),
            ),
            edge(
                handler,
                external,
                "CALLS",
                Confidence::CompilerResolved,
                Some("Controller.java"),
            ),
            edge(
                service,
                repository,
                "CALLS",
                Confidence::CompilerResolved,
                Some("Service.java"),
            ),
            edge(
                service,
                audit,
                "CALLS",
                Confidence::CompilerResolved,
                Some("Service.java"),
            ),
            edge(
                service,
                "type:app.event.Repository",
                "USES_REPOSITORY",
                Confidence::CompilerResolved,
                Some("Service.java"),
            ),
            edge(
                service,
                "entity:app.event.Event",
                "WRITES_ENTITY",
                Confidence::FrameworkResolved,
                Some("Service.java"),
            ),
            edge(
                service,
                "type:app.event.Created",
                "PUBLISHES_EVENT",
                Confidence::FrameworkResolved,
                Some("Service.java"),
            ),
            edge(
                "type:app.event.Service",
                "bean:one",
                "BEAN_CANDIDATE",
                Confidence::Ambiguous,
                Some("Service.java"),
            ),
            edge(
                "type:app.event.Service",
                "bean:two",
                "BEAN_CANDIDATE",
                Confidence::Ambiguous,
                Some("Service.java"),
            ),
        ];
        let mut database = Database::open_in_memory().unwrap();
        database
            .register_project(&root, Some("phase4"), &[])
            .unwrap();
        database
            .load_synthetic(
                "phase4",
                &SyntheticGraph {
                    project: "phase4".to_owned(),
                    nodes,
                    edges,
                },
            )
            .unwrap();
        (database, GraphineConfig::default(), root)
    }

    #[test]
    fn evidence_ids_are_generation_bound_and_deterministic() {
        let root = std::env::temp_dir();
        let status = IndexStatus {
            project: graphine_index::ProjectRecord {
                id: graphine_protocol::ProjectId::from_canonical_root(&root),
                canonical_root: root,
                display_name: "fixture".to_owned(),
                registered_at_ms: 0,
                source_fingerprint: None,
                analyzer_version: "test".to_owned(),
                schema_version: 3,
            },
            active_generation: Some(7),
            state: graphine_protocol::GenerationState::Ready,
            stale: false,
            last_successful_activation_ms: None,
            failure_summary: None,
            node_count: 0,
            edge_count: 0,
            occurrence_count: 0,
            diagnostic_count: 0,
            partial: false,
            analyzer_protocol_version: None,
            analysis_summary: None,
        };
        let location = EvidenceLocation {
            file_path: "src/A.java".to_owned(),
            start_line: 2,
            end_line: 4,
        };
        let first = evidence_id(&status, &location);
        assert_eq!(first, evidence_id(&status, &location));
        assert!(first.starts_with("evidence:7:"));
    }

    #[test]
    fn sensitive_and_traversing_paths_are_rejected() {
        let root = std::env::temp_dir();
        assert!(validated_source_path(&root, "../secret", &[]).is_err());
        assert!(validated_source_path(&root, ".env", &[]).is_err());
        assert!(validated_source_path(&root, "keys/server.pem", &[]).is_err());
    }

    #[test]
    fn structured_array_compaction_retains_omitted_counts() {
        let mut value = json!({"data_access":{"reads":["A","B"]}});
        assert!(truncate_array_at_path(&mut value, "data_access.reads"));
        assert_eq!(value["data_access"]["reads"], json!(["A"]));
        assert_eq!(value["data_access"]["reads_omitted_count"], 1);
    }

    #[test]
    fn endpoint_context_is_branch_aware_and_reports_ambiguous_handlers() {
        let (database, config, _) = fixture();
        let response = QueryService::new(&database, &config)
            .get_endpoint_context(&EndpointContextRequest {
                project: "phase4".to_owned(),
                route_stable_id: None,
                method: Some("POST".to_owned()),
                path: Some("/events".to_owned()),
                controller_method_stable_id: None,
                max_depth: Some(4),
                include: Vec::new(),
                suppression_policy: None,
                cursor: None,
                token_budget: Some(1400),
            })
            .unwrap();
        assert_eq!(response.completeness.status, "ambiguous");
        assert_eq!(response.result["handlers"].as_array().unwrap().len(), 2);
        assert!(
            response.result["flow"]["paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path.to_string().contains("Repository#save"))
        );
        assert!(
            response.result["flow"]["branch_points"]
                .to_string()
                .contains("Service#create")
        );
        assert_eq!(
            response.result["data_access"]["writes"],
            json!(["app.event.Event"])
        );
        assert!(
            response.result["events"]
                .to_string()
                .contains("PUBLISHES_EVENT")
        );
        assert!(response.result["suppressed_fact_count"].as_u64().unwrap() >= 1);
        assert!(response.budget.estimated_tokens <= 1400);
        let actual = json!({
            "ambiguity_count": response.ambiguities.len(),
            "completeness": response.completeness.status,
            "data_writes": response.result["data_access"]["writes"],
            "endpoint": {
                "confidence": response.result["endpoint"]["confidence"],
                "method": response.result["endpoint"]["method"],
                "path": response.result["endpoint"]["path"],
            },
            "event_kinds": response.result["events"].as_array().unwrap().iter().filter_map(|event| event.get("kind")).cloned().collect::<Vec<_>>(),
            "handler_count": response.result["handlers"].as_array().unwrap().len(),
            "suppression_policy": response.result["suppression_policy"],
        });
        let golden: Value =
            serde_json::from_str(include_str!("../tests/golden/endpoint-context.json")).unwrap();
        assert_eq!(actual, golden);
    }

    #[test]
    fn grouped_symbol_context_preserves_ambiguous_bean_candidates() {
        let (database, config, _) = fixture();
        let response = QueryService::new(&database, &config)
            .get_symbol_context(&super::super::SymbolContextRequest {
                project: "phase4".to_owned(),
                stable_id: "type:app.event.Service".to_owned(),
                include: vec!["injections".to_owned()],
                depth: Some(1),
                cursor: None,
                detail: graphine_protocol::DetailLevel::Standard,
                token_budget: Some(1000),
            })
            .unwrap();
        assert_eq!(response.result["dependencies"].as_array().unwrap().len(), 2);
        assert_eq!(response.completeness.status, "ambiguous");
        assert_eq!(response.ambiguities.len(), 1);
    }

    #[test]
    fn evidence_retrieval_is_secure_line_numbered_and_rejects_stale_ids() {
        let (mut database, config, _) = fixture();
        let endpoint = QueryService::new(&database, &config)
            .get_endpoint_context(&EndpointContextRequest {
                project: "phase4".to_owned(),
                route_stable_id: Some("route:POST:/events".to_owned()),
                method: None,
                path: None,
                controller_method_stable_id: None,
                max_depth: Some(2),
                include: Vec::new(),
                suppression_policy: None,
                cursor: None,
                token_budget: Some(1200),
            })
            .unwrap();
        let id = endpoint.result["evidence_refs"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let evidence = QueryService::new(&database, &config)
            .get_evidence(&GetEvidenceRequest {
                project: "phase4".to_owned(),
                references: vec![EvidenceRequestRef {
                    id: Some(id.clone()),
                    file: None,
                    start_line: None,
                    end_line: None,
                }],
                context_lines: Some(1),
                max_total_lines: Some(10),
                token_budget: Some(800),
            })
            .unwrap();
        assert_eq!(
            evidence.result["snippets"][0]["requested_lines"][0]["line"],
            1
        );
        assert!(
            QueryService::new(&database, &config)
                .get_evidence(&GetEvidenceRequest {
                    project: "phase4".to_owned(),
                    references: vec![EvidenceRequestRef {
                        id: None,
                        file: Some(".env".to_owned()),
                        start_line: Some(1),
                        end_line: Some(1)
                    }],
                    context_lines: None,
                    max_total_lines: None,
                    token_budget: Some(800),
                })
                .is_err()
        );
        assert!(
            QueryService::new(&database, &config)
                .get_evidence(&GetEvidenceRequest {
                    project: "phase4".to_owned(),
                    references: vec![EvidenceRequestRef {
                        id: None,
                        file: Some("Controller.java".to_owned()),
                        start_line: Some(1),
                        end_line: Some(4),
                    }],
                    context_lines: Some(0),
                    max_total_lines: Some(3),
                    token_budget: Some(800),
                })
                .is_err()
        );
        let graph = SyntheticGraph {
            project: "phase4".to_owned(),
            nodes: vec![node(
                "type:app.New",
                "TYPE",
                "app.New",
                Some("Controller.java"),
                Confidence::CompilerResolved,
                json!({}),
            )],
            edges: Vec::new(),
        };
        database.load_synthetic("phase4", &graph).unwrap();
        let stale = QueryService::new(&database, &config)
            .get_evidence(&GetEvidenceRequest {
                project: "phase4".to_owned(),
                references: vec![EvidenceRequestRef {
                    id: Some(id),
                    file: None,
                    start_line: None,
                    end_line: None,
                }],
                context_lines: None,
                max_total_lines: None,
                token_budget: Some(800),
            })
            .unwrap_err();
        assert_eq!(stale.code(), "generation_conflict");
    }

    #[test]
    fn application_only_trace_suppresses_external_nodes_and_handles_cycles() {
        let (database, config, _) = fixture();
        let response = QueryService::new(&database, &config)
            .trace_flow(&super::super::TraceFlowRequest {
                project: "phase4".to_owned(),
                start_stable_id: "method:app.event.Controller#create(app.event.Dto)".to_owned(),
                direction: graphine_protocol::Direction::Outbound,
                edge_kinds: Vec::new(),
                edge_groups: vec!["calls".to_owned()],
                node_kinds: Vec::new(),
                application_only: true,
                suppress_external: true,
                mode: Some("all_paths_bounded".to_owned()),
                max_depth: Some(4),
                max_nodes: Some(20),
                max_paths: Some(10),
                cursor: None,
                token_budget: Some(1000),
            })
            .unwrap();
        assert!(
            response.result["suppressed_external_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(
            !response.result["paths"]
                .to_string()
                .contains("java.util.Objects")
        );
    }
}
