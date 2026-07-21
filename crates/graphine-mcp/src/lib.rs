use graphine_index::Database;
use graphine_protocol::{GraphineConfig, GraphineError};
use graphine_query::QueryService;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::time::Instant;
use tracing::{error, info_span, warn};

pub struct McpServer {
    database: Database,
    config: GraphineConfig,
}

impl McpServer {
    #[must_use]
    pub const fn new(database: Database, config: GraphineConfig) -> Self {
        Self { database, config }
    }

    /// Handles one JSON-RPC request or notification.
    ///
    /// # Errors
    ///
    /// This method maps operational errors into JSON-RPC responses and does not expose them.
    #[allow(clippy::needless_pass_by_value)]
    pub fn handle_value(&self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str);
        if id.is_none() {
            if matches!(
                method,
                Some("notifications/cancelled" | "notifications/initialized")
            ) {
                return None;
            }
            warn!(?method, "ignored MCP notification");
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        let Some(method) = method else {
            return Some(rpc_error(id, -32600, "invalid request", None));
        };
        let span = info_span!("mcp_request", request_name = method);
        let _guard = span.enter();
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => Ok(initialize_result(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => self.call_tool(&params),
            _ => return Some(rpc_error(id, -32601, "method not found", None)),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => rpc_error(id, -32602, &error.to_string(), Some(error.safe_data())),
        })
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
            "trace_flow" => service.trace_flow(&decode(arguments)?)?,
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
        if request.get("method").and_then(Value::as_str) == Some("shutdown") {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            serde_json::to_writer(&mut output, &json!({"jsonrpc":"2.0","id":id,"result":{}}))?;
            writeln!(output)?;
            output.flush()?;
            break;
        }
        if let Some(response) = server.handle_value(request) {
            serde_json::to_writer(&mut output, &response)?;
            writeln!(output)?;
            output.flush()?;
        }
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, GraphineError> {
    serde_json::from_value(value)
        .map_err(|error| GraphineError::InvalidArgument(format!("malformed arguments: {error}")))
}

fn initialize_result(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2025-06-18");
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
            "Compact active graph summary",
            json!({
                "type":"object","additionalProperties":false,"required":["project"],
                "properties":{"project":{"type":"string"},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "search_symbol",
            "Deterministic lexical symbol search",
            json!({
                "type":"object","additionalProperties":false,"required":["project","query"],
                "properties":{"project":{"type":"string"},"query":{"type":"string"},"kinds":{"type":"array","items":{"type":"string"}},"limit":{"type":"integer","minimum":1},"cursor":{"type":["string","null"]},"detail":{"enum":["summary","standard","detailed","evidence"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "get_symbol_context",
            "Direct inbound and outbound symbol context",
            json!({
                "type":"object","additionalProperties":false,"required":["project","stable_id"],
                "properties":{"project":{"type":"string"},"stable_id":{"type":"string"},"detail":{"enum":["summary","standard","detailed","evidence"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "trace_flow",
            "Bounded direct graph traversal",
            json!({
                "type":"object","additionalProperties":false,"required":["project","start_stable_id"],
                "properties":{"project":{"type":"string"},"start_stable_id":{"type":"string"},"direction":{"enum":["inbound","outbound"]},"edge_kinds":{"type":"array","items":{"type":"string"}},"max_depth":{"type":"integer","minimum":1},"max_nodes":{"type":"integer","minimum":1},"cursor":{"type":["string","null"]},"token_budget":{"type":"integer","minimum":128}}
            }),
        ),
        tool(
            "index_status",
            "Project generation and stale status",
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
        ];
        let edges = vec![SyntheticEdge {
            source_stable_id: "type:example.Controller".to_owned(),
            target_stable_id: "method:example.Controller#create()".to_owned(),
            kind: "DECLARES".to_owned(),
            confidence: Confidence::CompilerResolved,
            provenance: "test".to_owned(),
            metadata: json!({}),
        }];
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
        McpServer::new(database, GraphineConfig::default())
    }

    #[allow(clippy::needless_pass_by_value)]
    fn request(id: i64, method: &str, params: Value) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    }

    #[test]
    fn initialization_and_tool_discovery_are_deterministic() {
        let server = server();
        let init = server
            .handle_value(request(
                1,
                "initialize",
                json!({"protocolVersion":"2025-06-18"}),
            ))
            .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "graphine");
        let first = server
            .handle_value(request(2, "tools/list", json!({})))
            .unwrap();
        let second = server
            .handle_value(request(2, "tools/list", json!({})))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first["result"]["tools"].as_array().unwrap().len(), 5);
        let call = request(
            3,
            "tools/call",
            json!({"name":"search_symbol","arguments":{"project":"mcp-fixture","query":"create"}}),
        );
        assert_eq!(server.handle_value(call.clone()), server.handle_value(call));
    }

    #[test]
    fn every_tool_accepts_a_valid_call() {
        let server = server();
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
                "trace_flow",
                json!({"project":"mcp-fixture","start_stable_id":"type:example.Controller","token_budget":800}),
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
                <= 128
        );
        let invalid = server.handle_value(request(2,"tools/call",json!({"name":"index_status","arguments":{"project":"mcp-fixture","token_budget":64}}))).unwrap();
        assert_eq!(invalid["error"]["data"]["code"], "invalid_token_budget");
        assert!(server.handle_value(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}})).is_none());
    }

    #[test]
    fn stdio_transport_initializes_calls_and_shuts_down() {
        let server = server();
        let input = format!(
            "{}\n{}\n{}\n",
            request(1, "initialize", json!({})),
            request(
                2,
                "tools/call",
                json!({"name":"index_status","arguments":{"project":"mcp-fixture"}})
            ),
            request(3, "shutdown", json!({})),
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
    }
}
