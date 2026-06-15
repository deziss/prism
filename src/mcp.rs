//! MCP server — Model Context Protocol implementation.
//!
//! Exposes PRISM tools to any MCP-compatible client (Claude Code, Continue, etc.)

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{any, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::knowledge;
use crate::memory;

pub async fn start_mcp_server(port: u16) -> Result<()> {
    let state = Arc::new(RwLock::new(McpState::new()));

    let app = Router::new()
        .route("/mcp/v1/tools", post(list_tools))
        .route("/mcp/v1/call", post(call_tool))
        .route("/health", any(health))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("PRISM MCP server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

struct McpState {
    tools: Vec<McpTool>,
}

impl McpState {
    fn new() -> Self {
        Self {
            tools: vec![
                McpTool {
                    name: "prism_memory_search".into(),
                    description: "Search PRISM Memory Palace".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {"query": {"type": "string", "description": "Search query"}},
                        "required": ["query"]
                    }),
                },
                McpTool {
                    name: "prism_memory_save".into(),
                    description: "Save a fact to PRISM Memory Palace".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "Memory key"},
                            "value": {"type": "string", "description": "Memory value"}
                        },
                        "required": ["key", "value"]
                    }),
                },
                McpTool {
                    name: "prism_graph_query".into(),
                    description: "Query PRISM Knowledge Graph".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {"query": {"type": "string", "description": "Graph query"}},
                        "required": ["query"]
                    }),
                },
                McpTool {
                    name: "prism_toon_encode".into(),
                    description: "Encode JSON to TOON format".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {"json": {"type": "string", "description": "JSON string"}},
                        "required": ["json"]
                    }),
                },
                McpTool {
                    name: "prism_count_tokens".into(),
                    description: "Count tokens in text".into(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "text": {"type": "string", "description": "Text to count"},
                            "model": {"type": "string", "default": "gpt-4"}
                        },
                        "required": ["text"]
                    }),
                },
            ],
        }
    }
}

#[derive(Serialize, Deserialize)]
struct McpTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize)]
struct CallRequest {
    tool: String,
    arguments: serde_json::Value,
}

async fn list_tools(State(_state): State<Arc<RwLock<McpState>>>) -> (StatusCode, String) {
    let tools = vec![
        serde_json::json!({"name":"prism_memory_search","description":"Search PRISM Memory Palace","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}),
        serde_json::json!({"name":"prism_memory_save","description":"Save a fact to memory","inputSchema":{"type":"object","properties":{"key":{"type":"string"},"value":{"type":"string"}},"required":["key","value"]}}),
        serde_json::json!({"name":"prism_graph_query","description":"Query knowledge graph","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}),
        serde_json::json!({"name":"prism_toon_encode","description":"Encode JSON to TOON","inputSchema":{"type":"object","properties":{"json":{"type":"string"}},"required":["json"]}}),
        serde_json::json!({"name":"prism_count_tokens","description":"Count tokens","inputSchema":{"type":"object","properties":{"text":{"type":"string"},"model":{"type":"string","default":"gpt-4"}},"required":["text"]}}),
    ];
    (StatusCode::OK, serde_json::to_string(&tools).unwrap())
}

async fn call_tool(
    State(_state): State<Arc<RwLock<McpState>>>,
    body: String,
) -> (StatusCode, String) {
    let req: CallRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, format!(r#"{{"error": "{}"}}"#, e)),
    };

    let result = match req.tool.as_str() {
        "prism_memory_search" => {
            let query = req.arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            match memory::search(query).await {
                Ok(_) => serde_json::json!({"result": "searched"}),
                Err(e) => serde_json::json!({"error": e.to_string()}),
            }
        }
        "prism_memory_save" => {
            let key = req.arguments.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let value = req.arguments.get("value").and_then(|v| v.as_str()).unwrap_or("");
            match memory::save(key, value).await {
                Ok(_) => serde_json::json!({"result": "saved"}),
                Err(e) => serde_json::json!({"error": e.to_string()}),
            }
        }
        "prism_graph_query" => {
            let query = req.arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            match knowledge::query_graph(query).await {
                Ok(_) => serde_json::json!({"result": "queried"}),
                Err(e) => serde_json::json!({"error": e.to_string()}),
            }
        }
        "prism_toon_encode" => {
            let json_str = req.arguments.get("json").and_then(|v| v.as_str()).unwrap_or("");
            match serde_json::from_str::<serde_json::Value>(json_str) {
                Ok(v) => match crate::encode::encode_json_to_toon(&v) {
                    Ok(toon) => serde_json::json!({"result": toon}),
                    Err(e) => serde_json::json!({"error": e.to_string()}),
                },
                Err(e) => serde_json::json!({"error": e.to_string()}),
            }
        }
        "prism_count_tokens" => {
            let text = req.arguments.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let model = req.arguments.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4");
            match crate::analytics::count_tokens(text, model) {
                Ok(count) => serde_json::json!({"tokens": count}),
                Err(e) => serde_json::json!({"error": e.to_string()}),
            }
        }
        _ => serde_json::json!({"error": format!("Unknown tool: {}", req.tool)}),
    };

    (StatusCode::OK, result.to_string())
}

async fn health() -> (StatusCode, String) {
    (StatusCode::OK, r#"{"status": "ok", "mcp": true}"#.to_string())
}
