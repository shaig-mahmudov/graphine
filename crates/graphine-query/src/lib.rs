use graphine_index::{Database, EdgeRecord, IndexStatus, NodeRecord};
use graphine_protocol::{
    Budget, DetailLevel, Direction, EvidenceRef, GraphineConfig, GraphineError, Pagination,
    ResponseEnvelope, StableId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::{BTreeSet, VecDeque};
use std::time::Instant;
use tracing::{debug, instrument};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMapRequest {
    pub project: String,
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
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub max_nodes: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub token_budget: Option<u32>,
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
        let summary = json!({
            "node_counts_by_kind": graph.node_counts.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
            "edge_counts_by_kind": graph.edge_counts.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
            "modules": graph.modules,
            "packages": graph.packages,
            "active_generation": graph.status.active_generation,
            "unresolved_count": graph.unresolved_count,
        });
        let envelope = compact(EnvelopeParts::new(&graph.status, summary), budget);
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
        let offset = parse_cursor(request.cursor.as_deref())?;
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
        candidates.retain(|node| lexical_rank(request.query.trim(), node) > 0);
        candidates.sort_by_key(|node| {
            (
                Reverse(lexical_rank(request.query.trim(), node)),
                node.qualified_name.to_ascii_lowercase(),
                node.stable_id.clone(),
            )
        });
        let total = candidates.len();
        let facts: Vec<_> = candidates
            .into_iter()
            .skip(offset)
            .take(limit as usize)
            .map(|node| node_fact(&node, request.detail))
            .collect();
        let next_offset = offset.saturating_add(facts.len());
        let has_more = next_offset < total;
        let mut parts = EnvelopeParts::new(
            &status,
            json!({"query": request.query, "matched": total, "returned": facts.len()}),
        );
        parts.essential_summary = json!({"query": request.query, "matched": total});
        parts.facts = facts;
        parts.has_more = has_more;
        parts.cursor = has_more.then(|| format!("offset:{next_offset}"));
        for node in &parts.facts {
            if let (Some(file), Some(start), Some(end)) = (
                node.get("file_path").and_then(Value::as_str),
                node.get("start_line").and_then(Value::as_u64),
                node.get("end_line").and_then(Value::as_u64),
            ) {
                parts.evidence.push(EvidenceRef {
                    file: file.to_owned(),
                    start_line: u32::try_from(start).unwrap_or(u32::MAX),
                    end_line: u32::try_from(end).unwrap_or(u32::MAX),
                });
            }
        }
        Ok(compact(parts, budget))
    }

    /// Returns a symbol and its direct inbound/outbound relationships.
    ///
    /// # Errors
    ///
    /// Returns stable-ID, symbol, index, budget, or database errors.
    pub fn get_symbol_context(
        &self,
        request: &SymbolContextRequest,
    ) -> Result<ResponseEnvelope, GraphineError> {
        let budget = self.budget(request.token_budget)?;
        let stable_id = StableId::parse(request.stable_id.clone())?;
        let (status, node) = self.database.node(&request.project, &stable_id)?;
        let inbound = self.database.direct_edges(
            &status,
            stable_id.as_str(),
            true,
            &[],
            self.config.maximum_result_count,
        )?;
        let outbound = self.database.direct_edges(
            &status,
            stable_id.as_str(),
            false,
            &[],
            self.config.maximum_result_count,
        )?;
        let mut parts = EnvelopeParts::new(
            &status,
            json!({"symbol": node_fact(&node, request.detail), "inbound_count": inbound.len(), "outbound_count": outbound.len()}),
        );
        parts.essential_summary = json!({
            "symbol": {
                "stable_id": node.stable_id,
                "kind": node.kind,
                "confidence": node.confidence,
            }
        });
        parts.facts = inbound
            .iter()
            .map(|edge| edge_fact(edge, "inbound", request.detail))
            .chain(
                outbound
                    .iter()
                    .map(|edge| edge_fact(edge, "outbound", request.detail)),
            )
            .collect();
        parts.unresolved = node.unresolved;
        if node.confidence == graphine_protocol::Confidence::Ambiguous {
            parts
                .ambiguities
                .push("symbol confidence is AMBIGUOUS".to_owned());
        }
        if let (Some(file), Some(start), Some(end)) =
            (node.file_path, node.start_line, node.end_line)
        {
            parts.evidence.push(EvidenceRef {
                file,
                start_line: start,
                end_line: end,
            });
        }
        let total_facts = parts.facts.len();
        let mut result = compact(parts, budget);
        if result.facts.len() < total_facts {
            result.pagination.has_more = true;
            result.pagination.cursor = Some(format!("context:{}", result.facts.len()));
        }
        Ok(result)
    }

    /// Traverses direct edges breadth-first without loading the full graph.
    ///
    /// # Errors
    ///
    /// Returns validation, stable-ID, index, cursor, budget, or database errors.
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
        if max_depth == 0 || max_nodes == 0 {
            return Err(GraphineError::InvalidArgument(
                "depth and node limits must be positive".to_owned(),
            ));
        }
        let offset = parse_cursor(request.cursor.as_deref())?;
        let (status, _) = self.database.node(&request.project, &start)?;
        let mut queue = VecDeque::from([(start.as_str().to_owned(), 0_u32)]);
        let mut visited = BTreeSet::from([start.as_str().to_owned()]);
        let mut traversed = Vec::new();
        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_depth || visited.len() > max_nodes as usize {
                continue;
            }
            let inbound = request.direction == Direction::Inbound;
            for edge in self.database.direct_edges(
                &status,
                &current,
                inbound,
                &request.edge_kinds,
                max_nodes,
            )? {
                let next = if inbound {
                    edge.source_stable_id.clone()
                } else {
                    edge.target_stable_id.clone()
                };
                traversed.push(json!({
                    "depth": depth + 1,
                    "source": edge.source_stable_id,
                    "target": edge.target_stable_id,
                    "kind": edge.kind,
                    "confidence": edge.confidence,
                }));
                if visited.insert(next.clone()) && visited.len() <= max_nodes as usize {
                    queue.push_back((next, depth + 1));
                }
            }
        }
        let total = traversed.len();
        let facts: Vec<_> = traversed.into_iter().skip(offset).collect();
        let mut parts = EnvelopeParts::new(
            &status,
            json!({"start": start, "direction": request.direction, "visited_nodes": visited.len(), "edge_count": total}),
        );
        parts.essential_summary = json!({"start": start, "direction": request.direction});
        parts.facts = facts;
        let mut result = compact(parts, budget);
        let consumed = offset.saturating_add(result.facts.len());
        if consumed < total {
            result.pagination.has_more = true;
            result.pagination.cursor = Some(format!("offset:{consumed}"));
        }
        Ok(result)
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
        let unresolved = status.failure_summary.clone().into_iter().collect();
        let mut parts = EnvelopeParts {
            unresolved,
            ..EnvelopeParts::new(
                &status,
                json!({
                    "state": status.state,
                    "active_generation": status.active_generation,
                    "last_successful_activation_ms": status.last_successful_activation_ms,
                    "node_count": status.node_count,
                    "edge_count": status.edge_count,
                    "failure_summary": status.failure_summary,
                }),
            )
        };
        parts.essential_summary =
            json!({"state": status.state, "active_generation": status.active_generation});
        Ok(compact(parts, budget))
    }

    fn budget(&self, requested: Option<u32>) -> Result<u32, GraphineError> {
        let budget = requested.unwrap_or(self.config.default_token_budget);
        if !(128..=self.config.maximum_token_budget).contains(&budget) {
            return Err(GraphineError::InvalidTokenBudget);
        }
        Ok(budget)
    }
}

struct EnvelopeParts {
    project: String,
    generation: Option<i64>,
    stale: bool,
    summary: Value,
    essential_summary: Value,
    facts: Vec<Value>,
    unresolved: Vec<String>,
    ambiguities: Vec<String>,
    evidence: Vec<EvidenceRef>,
    has_more: bool,
    cursor: Option<String>,
}

impl EnvelopeParts {
    fn new(status: &IndexStatus, summary: Value) -> Self {
        Self {
            project: status.project.display_name.clone(),
            generation: status.active_generation,
            stale: status.stale,
            summary,
            essential_summary: json!({"active_generation": status.active_generation}),
            facts: Vec::new(),
            unresolved: Vec::new(),
            ambiguities: Vec::new(),
            evidence: Vec::new(),
            has_more: false,
            cursor: None,
        }
    }
}

fn compact(parts: EnvelopeParts, requested: u32) -> ResponseEnvelope {
    let total_facts = parts.facts.len();
    let total_evidence = parts.evidence.len();
    let mut envelope = ResponseEnvelope {
        project: parts.project,
        generation: parts.generation,
        stale: parts.stale,
        summary: parts.summary,
        facts: Vec::new(),
        unresolved: parts.unresolved,
        ambiguities: parts.ambiguities,
        evidence_refs: Vec::new(),
        pagination: Pagination {
            has_more: parts.has_more,
            cursor: parts.cursor,
        },
        budget: Budget {
            requested_tokens: requested,
            estimated_tokens: 0,
            truncated: false,
        },
    };
    refresh_estimate(&mut envelope);
    let mut base_truncated = false;
    if envelope.budget.estimated_tokens > requested {
        envelope.summary = parts.essential_summary;
        base_truncated = true;
        refresh_estimate(&mut envelope);
    }
    if envelope.budget.estimated_tokens > requested {
        envelope.summary = json!({"identity": "project and generation"});
        refresh_estimate(&mut envelope);
    }
    while envelope.budget.estimated_tokens > requested && !envelope.ambiguities.is_empty() {
        envelope.ambiguities.pop();
        refresh_estimate(&mut envelope);
    }
    while envelope.budget.estimated_tokens > requested && !envelope.unresolved.is_empty() {
        envelope.unresolved.pop();
        refresh_estimate(&mut envelope);
    }
    for fact in parts.facts {
        envelope.facts.push(fact);
        refresh_estimate(&mut envelope);
        if envelope.budget.estimated_tokens > requested {
            envelope.facts.pop();
            break;
        }
    }
    if envelope.facts.len() == total_facts {
        for evidence in parts.evidence {
            envelope.evidence_refs.push(evidence);
            refresh_estimate(&mut envelope);
            if envelope.budget.estimated_tokens > requested {
                envelope.evidence_refs.pop();
                break;
            }
        }
    }
    envelope.budget.truncated = base_truncated
        || envelope.facts.len() < total_facts
        || envelope.evidence_refs.len() < total_evidence;
    if envelope.budget.truncated {
        envelope.pagination.has_more = true;
    }
    refresh_estimate(&mut envelope);
    envelope
}

fn refresh_estimate(envelope: &mut ResponseEnvelope) {
    envelope.budget.estimated_tokens = 0;
    envelope.budget.estimated_tokens = estimated_tokens(envelope);
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
    if query_lower == qualified {
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

fn parse_cursor(cursor: Option<&str>) -> Result<usize, GraphineError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    cursor
        .strip_prefix("offset:")
        .and_then(|value| value.parse().ok())
        .ok_or(GraphineError::InvalidCursor)
}

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

fn edge_fact(edge: &EdgeRecord, direction: &str, detail: DetailLevel) -> Value {
    let mut fact = json!({
        "direction": direction,
        "source": edge.source_stable_id,
        "target": edge.target_stable_id,
        "kind": edge.kind,
        "confidence": edge.confidence,
    });
    if matches!(detail, DetailLevel::Detailed | DetailLevel::Evidence) {
        fact.as_object_mut()
            .expect("object literal")
            .insert("metadata".to_owned(), edge.metadata.clone());
    }
    fact
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
    fn cursors_are_stable_and_reject_malformed_values() {
        assert_eq!(parse_cursor(Some("offset:42")).unwrap(), 42);
        assert!(parse_cursor(Some("42")).is_err());
        assert!(parse_cursor(Some("offset:nope")).is_err());
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
                detail: DetailLevel::Standard,
                token_budget: Some(800),
            })
            .unwrap();
        let context_ms = context_started.elapsed().as_millis();
        println!(
            "large_graph load_ms={load_ms} exact_lookup_ms={lookup_ms} context_ms={context_ms} estimated_tokens={} nodes=10000 edges=30000",
            context.budget.estimated_tokens
        );
        assert_eq!(result.summary["matched"], 1);
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
        }
    }
}
