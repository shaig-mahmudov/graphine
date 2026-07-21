use graphine_index::Database;
use graphine_protocol::{GraphineConfig, GraphineError};
use graphine_query::QueryService;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::Mutex;
use std::time::Instant;
use tracing::{error, info_span, warn};

pub struct McpServer {
    database: Database,
    config: GraphineConfig,
    lifecycle: Mutex<Lifecycle>,
}

const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[LATEST_PROTOCOL_VERSION, "2025-06-18", "2025-03-26"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    New,
    AwaitingInitialized,
    Ready,
    ShuttingDown,
}

impl McpServer {
    #[must_use]
    pub const fn new(database: Database, config: GraphineConfig) -> Self {
        Self {
            database,
            config,
            lifecycle: Mutex::new(Lifecycle::New),
        }
    }

    /// Handles one JSON-RPC request or notification.
    ///
    /// # Errors
    ///
    /// This method maps operational errors into JSON-RPC responses and does not expose them.
    #[allow(clippy::needless_pass_by_value)]
    pub fn handle_value(&self, request: Value) -> Option<Value> {
        if !request.is_object() || request.get("jsonrpc") != Some(&Value::String("2.0".to_owned()))
        {
            return Some(rpc_error(Value::Null, -32600, "invalid request", None));
        }
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str);
        if id.is_none() {
            match method {
                Some("notifications/initialized") => {
                    let mut lifecycle = self.lifecycle.lock().ok()?;
                    if *lifecycle == Lifecycle::AwaitingInitialized {
                        *lifecycle = Lifecycle::Ready;
                    }
                    return None;
                }
                Some("notifications/cancelled") => {
                    // Serial STDIO processing cannot interrupt an active call. Cancellation is
                    // deliberately not advertised and late/unknown notifications are ignored.
                    return None;
                }
                Some("exit") => return None,
                _ => {}
            }
            warn!(?method, "ignored MCP notification");
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        if !matches!(id, Value::String(_) | Value::Number(_)) {
            return Some(rpc_error(Value::Null, -32600, "invalid request ID", None));
        }
        let Some(method) = method else {
            return Some(rpc_error(id, -32600, "invalid request", None));
        };
        let span = info_span!("mcp_request", request_name = method);
        let _guard = span.enter();
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(rpc_error(id, -32602, "params must be an object", None));
        }
        let lifecycle = self
            .lifecycle
            .lock()
            .map_or(Lifecycle::ShuttingDown, |value| *value);
        if method != "initialize" && method != "ping" && lifecycle != Lifecycle::Ready {
            return Some(rpc_error(id, -32002, "server is not initialized", None));
        }
        let result = match method {
            "initialize" => self.initialize(&params),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => self.call_tool(&params),
            "shutdown" => {
                if let Ok(mut lifecycle) = self.lifecycle.lock() {
                    *lifecycle = Lifecycle::ShuttingDown;
                }
                Ok(Value::Null)
            }
            _ => return Some(rpc_error(id, -32601, "method not found", None)),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => rpc_error(id, -32602, &error.to_string(), Some(error.safe_data())),
        })
    }

    fn initialize(&self, params: &Value) -> Result<Value, GraphineError> {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GraphineError::InvalidArgument("protocolVersion is required".to_owned())
            })?;
        if !params.get("capabilities").is_some_and(Value::is_object)
            || !params.get("clientInfo").is_some_and(Value::is_object)
        {
            return Err(GraphineError::InvalidArgument(
                "capabilities and clientInfo are required".to_owned(),
            ));
        }
        let mut lifecycle = self.lifecycle.lock().map_err(|_| GraphineError::Internal)?;
        if *lifecycle != Lifecycle::New {
            return Err(GraphineError::InvalidArgument(
                "initialize may only be called once".to_owned(),
            ));
        }
        *lifecycle = Lifecycle::AwaitingInitialized;
        let negotiated = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            LATEST_PROTOCOL_VERSION
        };
        Ok(initialize_result(negotiated))
    }

    fn call_tool(&self, params: &Value) -> Result<Value, GraphineError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| GraphineError::InvalidArgument("tool name is required".to_owned()))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let started = Instant::now();
        let service = QueryService::new(&self.database, &self.config);
        let envelope = match name {
            "get_project_map" => service.get_project_map(&decode(arguments)?)?,
            "search_symbol" => service.search_symbol(&decode(arguments)?)?,
            "get_symbol_context" => service.get_symbol_context(&decode(arguments)?)?,
            "get_endpoint_context" => service.get_endpoint_context(&decode(arguments)?)?,
            "trace_flow" => service.trace_flow(&decode(arguments)?)?,
            "get_evidence" => service.get_evidence(&decode(arguments)?)?,
            "index_status" => service.index_status(&decode(arguments)?)?,
            _ => return Err(GraphineError::InvalidArgument("unknown tool".to_owned())),
        };
        tracing::info!(
            project = envelope.project,
            generation = envelope.generation,
            duration_ms = started.elapsed().as_millis(),
            result_count = envelope.facts.len(),
            estimated_tokens = envelope.budget.estimated_tokens,
            truncated = envelope.budget.truncated,
            "MCP tool complete"
        );
        let text = serde_json::to_string(&envelope).map_err(|_| GraphineError::Internal)?;
        let structured = serde_json::to_value(&envelope).map_err(|_| GraphineError::Internal)?;
        Ok(json!({
            "content": [{"type": "text", "text": text}],
            "structuredContent": structured,
            "isError": false
        }))
    }
}

/// Runs newline-delimited JSON-RPC over STDIO-compatible streams until EOF or `shutdown`.
///
/// # Errors
///
/// Returns an I/O error if the transport cannot read or write.
pub fn run_stdio<R: BufRead, W: Write>(
    server: &McpServer,
    mut input: R,
    mut output: W,
) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(parse_error) => {
                error!(?parse_error, "malformed MCP JSON");
                let response = rpc_error(Value::Null, -32700, "parse error", None);
                serde_json::to_writer(&mut output, &response)?;
                writeln!(output)?;
                output.flush()?;
                continue;
            }
        };
        let exit = request.get("method").and_then(Value::as_str) == Some("exit");
        if let Some(response) = server.handle_value(request) {
            serde_json::to_writer(&mut output, &response)?;
            writeln!(output)?;
            output.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, GraphineError> {
    serde_json::from_value(value)
        .map_err(|error| GraphineError::InvalidArgument(format!("malformed arguments: {error}")))
}

fn initialize_result(protocol_version: &str) -> Value {
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "graphine", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "Graphine serves the active local Java graph. Analysis is explicit; MCP reads never execute Maven or trigger indexing. Compiler-resolved relationships include dispatch metadata, while unresolved bindings remain diagnostics."
    })
}

#[allow(clippy::needless_pass_by_value)]
fn rpc_error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let Some(data) = data {
        error
            .as_object_mut()
            .expect("object literal")
            .insert("data".to_owned(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "get_project_map",
            "Returns a compact application map with major packages and Spring role counts. Use first when orienting in a project.",
            json!({
                "type":"object","additionalProperties":false,"required":["project"],
                "properties":{"project":{"type":"string"},"scope":{"enum":["application"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "search_symbol",
            "Finds likely symbols or routes with deterministic lexical and structural ranking. Use to disambiguate a human name before requesting context.",
            json!({
                "type":"object","additionalProperties":false,"required":["project","query"],
                "properties":{"project":{"type":"string"},"query":{"type":"string"},"kinds":{"type":"array","items":{"type":"string"}},"framework_roles":{"type":"array","items":{"type":"string"}},"module":{"type":"string"},"package_prefix":{"type":"string"},"limit":{"type":"integer","minimum":1},"cursor":{"type":["string","null"]},"detail":{"enum":["summary","standard","detailed","evidence"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "get_symbol_context",
            "Returns semantically grouped callers, callees, injections, data access, routes, events, tests, and evidence for one symbol.",
            json!({
                "type":"object","additionalProperties":false,"required":["project","stable_id"],
                "properties":{"project":{"type":"string"},"stable_id":{"type":"string"},"include":{"type":"array","items":{"enum":["callers","callees","injections","data_access","routes","events","tests"]}},"depth":{"type":"integer","minimum":1},"cursor":{"type":["string","null"]},"detail":{"enum":["summary","standard","detailed","evidence"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "get_endpoint_context",
            "Returns static Spring flow, data access, validation, events, dependencies, and evidence for one HTTP endpoint. Use before reading controller or service files.",
            json!({
                "type":"object","additionalProperties":false,"required":["project"],
                "properties":{"project":{"type":"string"},"route_stable_id":{"type":"string"},"method":{"type":"string"},"path":{"type":"string"},"controller_method_stable_id":{"type":"string"},"max_depth":{"type":"integer","minimum":1},"include":{"type":"array","items":{"enum":["validation","dependencies","data_access","events","configuration"]}},"suppression_policy":{"enum":["agent-default-v1","none"]},"cursor":{"type":["string","null"]},"token_budget":{"type":"integer","minimum":128}},
                "oneOf":[{"required":["route_stable_id"]},{"required":["method","path"]},{"required":["controller_method_stable_id"]}]
            }),
        ),
        tool(
            "trace_flow",
            "Returns bounded grouped graph paths when symbol or endpoint context is insufficient. Supports application-only shortest or bounded all-path traversal.",
            json!({
                "type":"object","additionalProperties":false,"required":["project","start_stable_id"],
                "properties":{"project":{"type":"string"},"start_stable_id":{"type":"string"},"direction":{"enum":["inbound","outbound"]},"edge_kinds":{"type":"array","items":{"type":"string"}},"edge_groups":{"type":"array","items":{"enum":["calls","data_access","events","dependencies","routes"]}},"node_kinds":{"type":"array","items":{"type":"string"}},"application_only":{"type":"boolean"},"suppress_external":{"type":"boolean"},"mode":{"enum":["shortest_path","all_paths_bounded"]},"max_depth":{"type":"integer","minimum":1},"max_nodes":{"type":"integer","minimum":1},"max_paths":{"type":"integer","minimum":1},"cursor":{"type":["string","null"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "get_evidence",
            "Returns bounded line-numbered source snippets for Graphine evidence IDs or validated repository-relative ranges. Use only to verify selected claims.",
            json!({
                "type":"object","additionalProperties":false,"required":["project","references"],
                "properties":{"project":{"type":"string"},"references":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"file":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}},"oneOf":[{"required":["id"]},{"required":["file","start_line","end_line"]}]}},"context_lines":{"type":"integer","minimum":0,"maximum":20},"max_total_lines":{"type":"integer","minimum":1},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "index_status",
            "Returns generation freshness, analyzer capabilities, diagnostic counts, source sets, exclusions, and unsupported areas without listing every diagnostic.",
            json!({
                "type":"object","additionalProperties":false,"required":["project"],
                "properties":{"project":{"type":"string"},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
    ]
}

#[allow(clippy::needless_pass_by_value)]
fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": input_schema})
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphine_protocol::{Confidence, SyntheticEdge, SyntheticGraph, SyntheticNode};
    use std::fs;

    fn server() -> McpServer {
        let root = std::env::temp_dir().join(format!("graphine-mcp-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Controller.java"), "class Controller {}\n").unwrap();
        let mut database = Database::open_in_memory().unwrap();
        database
            .register_project(&root, Some("mcp-fixture"), &[])
            .unwrap();
        let nodes = vec![
            SyntheticNode {
                stable_id: "type:example.Controller".to_owned(),
                kind: "TYPE".to_owned(),
                qualified_name: "example.Controller".to_owned(),
                simple_name: None,
                module_name: Some("app".to_owned()),
                package_name: Some("example".to_owned()),
                file_path: Some("Controller.java".to_owned()),
                start_line: Some(1),
                end_line: Some(1),
                confidence: Confidence::CompilerResolved,
                provenance: "test".to_owned(),
                metadata: json!({}),
                unresolved: Vec::new(),
            },
            SyntheticNode {
                stable_id: "method:example.Controller#create()".to_owned(),
                kind: "METHOD".to_owned(),
                qualified_name: "example.Controller#create()".to_owned(),
                simple_name: Some("create".to_owned()),
                module_name: Some("app".to_owned()),
                package_name: Some("example".to_owned()),
                file_path: Some("Controller.java".to_owned()),
                start_line: Some(1),
                end_line: Some(1),
                confidence: Confidence::CompilerResolved,
                provenance: "test".to_owned(),
                metadata: json!({}),
                unresolved: Vec::new(),
            },
            SyntheticNode {
                stable_id: "route:POST:/create".to_owned(),
                kind: "ROUTE".to_owned(),
                qualified_name: "POST /create".to_owned(),
                simple_name: Some("create".to_owned()),
                module_name: Some("app".to_owned()),
                package_name: Some("example".to_owned()),
                file_path: Some("Controller.java".to_owned()),
                start_line: Some(1),
                end_line: Some(1),
                confidence: Confidence::FrameworkResolved,
                provenance: "test".to_owned(),
                metadata: json!({"http_method":"POST","path":"/create","handler_parameters":[]}),
                unresolved: Vec::new(),
            },
        ];
        let edges = vec![
            SyntheticEdge {
                source_stable_id: "type:example.Controller".to_owned(),
                target_stable_id: "method:example.Controller#create()".to_owned(),
                kind: "DECLARES".to_owned(),
                confidence: Confidence::CompilerResolved,
                provenance: "test".to_owned(),
                metadata: json!({}),
                occurrences: Vec::new(),
            },
            SyntheticEdge {
                source_stable_id: "type:example.Controller".to_owned(),
                target_stable_id: "route:POST:/create".to_owned(),
                kind: "EXPOSES_ROUTE".to_owned(),
                confidence: Confidence::FrameworkResolved,
                provenance: "test".to_owned(),
                metadata: json!({}),
                occurrences: Vec::new(),
            },
            SyntheticEdge {
                source_stable_id: "route:POST:/create".to_owned(),
                target_stable_id: "method:example.Controller#create()".to_owned(),
                kind: "HANDLED_BY".to_owned(),
                confidence: Confidence::FrameworkResolved,
                provenance: "test".to_owned(),
                metadata: json!({}),
                occurrences: Vec::new(),
            },
        ];
        database
            .load_synthetic(
                "mcp-fixture",
                &SyntheticGraph {
                    project: "mcp-fixture".to_owned(),
                    nodes,
                    edges,
                },
            )
            .unwrap();
        let config = GraphineConfig {
            maximum_result_count: 1,
            ..GraphineConfig::default()
        };
        McpServer::new(database, config)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request(id: i64, method: &str, params: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    }

    fn initialize_params(version: &str) -> Value {
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "graphine-test", "version": "1"}
        })
    }

    fn initialize(server: &McpServer) {
        let response = server
            .handle_value(request(
                999,
                "initialize",
                initialize_params(LATEST_PROTOCOL_VERSION),
            ))
            .unwrap();
        assert_eq!(
            response["result"]["protocolVersion"],
            LATEST_PROTOCOL_VERSION
        );
        assert!(
            server
                .handle_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .is_none()
        );
    }

    #[test]
    fn initialization_and_tool_discovery_are_deterministic() {
        let server = server();
        let init = server
            .handle_value(request(1, "initialize", initialize_params("2025-06-18")))
            .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "graphine");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        server.handle_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let first = server
            .handle_value(request(2, "tools/list", json!({})))
            .unwrap();
        let second = server
            .handle_value(request(2, "tools/list", json!({})))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first["result"]["tools"].as_array().unwrap().len(), 7);
        let context_schema = first["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "get_symbol_context")
            .unwrap();
        assert!(context_schema["inputSchema"]["properties"]["cursor"].is_object());
        let call = request(
            3,
            "tools/call",
            json!({"name":"search_symbol","arguments":{"project":"mcp-fixture","query":"create"}}),
        );
        assert_eq!(server.handle_value(call.clone()), server.handle_value(call));
    }

    #[test]
    fn symbol_context_cursor_is_accepted_and_bound_to_symbol_and_generation() {
        let mut server = server();
        initialize(&server);
        let first = server.handle_value(request(1,"tools/call",json!({"name":"get_symbol_context","arguments":{"project":"mcp-fixture","stable_id":"type:example.Controller","token_budget":128}}))).unwrap();
        let cursor = first["result"]["structuredContent"]["pagination"]["cursor"]
            .as_str()
            .unwrap()
            .to_owned();
        let resumed = server.handle_value(request(2,"tools/call",json!({"name":"get_symbol_context","arguments":{"project":"mcp-fixture","stable_id":"type:example.Controller","cursor":cursor,"token_budget":800}}))).unwrap();
        assert_eq!(resumed["result"]["isError"], false);

        let mismatched = server.handle_value(request(3,"tools/call",json!({"name":"get_symbol_context","arguments":{"project":"mcp-fixture","stable_id":"method:example.Controller#create()","cursor":cursor}}))).unwrap();
        assert_eq!(mismatched["error"]["data"]["code"], "invalid_cursor");

        let graph = SyntheticGraph {
            project: "mcp-fixture".to_owned(),
            nodes: vec![SyntheticNode {
                stable_id: "type:example.Controller".to_owned(),
                kind: "TYPE".to_owned(),
                qualified_name: "example.Controller".to_owned(),
                simple_name: None,
                module_name: None,
                package_name: Some("example".to_owned()),
                file_path: Some("Controller.java".to_owned()),
                start_line: Some(1),
                end_line: Some(1),
                confidence: Confidence::CompilerResolved,
                provenance: "test".to_owned(),
                metadata: json!({}),
                unresolved: Vec::new(),
            }],
            edges: Vec::new(),
        };
        server
            .database
            .load_synthetic("mcp-fixture", &graph)
            .unwrap();
        let stale = server.handle_value(request(4,"tools/call",json!({"name":"get_symbol_context","arguments":{"project":"mcp-fixture","stable_id":"type:example.Controller","cursor":cursor}}))).unwrap();
        assert_eq!(stale["error"]["data"]["code"], "invalid_cursor");
    }

    #[test]
    fn every_tool_accepts_a_valid_call() {
        let server = server();
        initialize(&server);
        let calls = [
            (
                "get_project_map",
                json!({"project":"mcp-fixture","token_budget":800}),
            ),
            (
                "search_symbol",
                json!({"project":"mcp-fixture","query":"create","token_budget":800}),
            ),
            (
                "get_symbol_context",
                json!({"project":"mcp-fixture","stable_id":"type:example.Controller","token_budget":800}),
            ),
            (
                "get_endpoint_context",
                json!({"project":"mcp-fixture","method":"POST","path":"/create","token_budget":800}),
            ),
            (
                "trace_flow",
                json!({"project":"mcp-fixture","start_stable_id":"type:example.Controller","token_budget":800}),
            ),
            (
                "get_evidence",
                json!({"project":"mcp-fixture","references":[{"file":"Controller.java","start_line":1,"end_line":1}],"token_budget":800}),
            ),
            (
                "index_status",
                json!({"project":"mcp-fixture","token_budget":800}),
            ),
        ];
        for (index, (name, arguments)) in calls.into_iter().enumerate() {
            let response = server
                .handle_value(request(
                    i64::try_from(index).unwrap(),
                    "tools/call",
                    json!({"name":name,"arguments":arguments}),
                ))
                .unwrap();
            assert_eq!(response["result"]["isError"], false, "{name}: {response}");
            assert!(
                response["result"]["structuredContent"]["budget"]["estimated_tokens"]
                    .as_u64()
                    .unwrap()
                    <= 800
            );
        }
    }

    #[test]
    fn invalid_projects_and_malformed_arguments_map_to_safe_errors() {
        let server = server();
        initialize(&server);
        let missing = server
            .handle_value(request(
                1,
                "tools/call",
                json!({"name":"index_status","arguments":{"project":"missing"}}),
            ))
            .unwrap();
        assert_eq!(missing["error"]["data"]["code"], "project_not_found");
        assert!(
            !missing
                .to_string()
                .contains(std::env::temp_dir().to_string_lossy().as_ref())
        );
        let malformed = server
            .handle_value(request(
                2,
                "tools/call",
                json!({"name":"search_symbol","arguments":{"project":3}}),
            ))
            .unwrap();
        assert_eq!(malformed["error"]["data"]["code"], "invalid_argument");
    }

    #[test]
    fn token_budgets_and_cancellation_are_handled() {
        let server = server();
        initialize(&server);
        let response = server.handle_value(request(1,"tools/call",json!({"name":"get_symbol_context","arguments":{"project":"mcp-fixture","stable_id":"type:example.Controller","token_budget":128}}))).unwrap();
        assert!(
            response["result"]["structuredContent"]["budget"]["truncated"]
                .as_bool()
                .is_some()
        );
        assert!(
            response["result"]["structuredContent"]["budget"]["estimated_tokens"]
                .as_u64()
                .unwrap()
                <= 128,
            "{response}"
        );
        let invalid = server.handle_value(request(2,"tools/call",json!({"name":"index_status","arguments":{"project":"mcp-fixture","token_budget":64}}))).unwrap();
        assert_eq!(invalid["error"]["data"]["code"], "invalid_token_budget");
        assert!(server.handle_value(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}})).is_none());
    }

    #[test]
    fn stdio_transport_initializes_calls_and_shuts_down() {
        let server = server();
        let input = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            request(1, "initialize", initialize_params(LATEST_PROTOCOL_VERSION)),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            request(
                2,
                "tools/call",
                json!({"name":"index_status","arguments":{"project":"mcp-fixture"}})
            ),
            request(3, "shutdown", json!({})),
            json!({"jsonrpc":"2.0","method":"exit"}),
        );
        let mut output = Vec::new();
        run_stdio(&server, input.as_bytes(), &mut output).unwrap();
        let lines: Vec<_> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "graphine");
        assert!(lines[2]["result"].is_null());
    }

    #[test]
    fn protocol_validation_lifecycle_and_batches_are_explicit() {
        let server = server();
        let wrong_jsonrpc = server
            .handle_value(json!({"jsonrpc":"1.0","id":1,"method":"initialize","params":initialize_params(LATEST_PROTOCOL_VERSION)}))
            .unwrap();
        assert_eq!(wrong_jsonrpc["error"]["code"], -32600);
        let premature = server
            .handle_value(request(2, "tools/list", json!({})))
            .unwrap();
        assert_eq!(premature["error"]["code"], -32002);
        let malformed_id = server
            .handle_value(json!({"jsonrpc":"2.0","id":{},"method":"initialize","params":initialize_params(LATEST_PROTOCOL_VERSION)}))
            .unwrap();
        assert_eq!(malformed_id["error"]["code"], -32600);
        let batch = server
            .handle_value(json!([request(
                3,
                "initialize",
                initialize_params(LATEST_PROTOCOL_VERSION)
            )]))
            .unwrap();
        assert_eq!(batch["error"]["code"], -32600);

        let negotiated = server
            .handle_value(request(4, "initialize", initialize_params("2099-01-01")))
            .unwrap();
        assert_eq!(
            negotiated["result"]["protocolVersion"],
            LATEST_PROTOCOL_VERSION
        );
        let before_ready = server
            .handle_value(request(5, "tools/list", json!({})))
            .unwrap();
        assert_eq!(before_ready["error"]["code"], -32002);
        server.handle_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        assert!(
            server
                .handle_value(request(6, "tools/list", json!({})))
                .unwrap()["result"]["tools"]
                .is_array()
        );
    }
}
