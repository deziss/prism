//! MCP server — Model Context Protocol 2024-11-05, JSON-RPC 2.0 transport.
//!
//! Claude Code connects via: claude mcp add prism --transport http http://localhost:3003

use anyhow::Result;
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tracing::info;

use crate::{analytics, encode, memory};

// ── Internal helpers that return String (memory::search prints to stdout) ─────

async fn memory_search_str(query: &str) -> anyhow::Result<String> {
    let palace = memory::MemoryPalace::new(memory::memory_palace_dir_pub())
        .map_err(|e| anyhow::anyhow!(e))?;
    let results = palace.search(query, 10);
    if results.is_empty() {
        Ok(format!("No memories found for: {}", query))
    } else {
        let lines: Vec<String> = results.iter().map(|b| {
            let preview = &b.content[..b.content.len().min(200)];
            format!("[{}] {}: {}", b.layer, b.category, preview)
        }).collect();
        Ok(lines.join("\n"))
    }
}

async fn memory_save_str(key: &str, value: &str) -> anyhow::Result<String> {
    let mut palace = memory::MemoryPalace::new(memory::memory_palace_dir_pub())
        .map_err(|e| anyhow::anyhow!(e))?;
    palace.save(memory::MemoryLayer::Core, value, key);
    Ok(format!("Saved: {}", key))
}

async fn graph_query_str(query: &str) -> anyhow::Result<String> {
    use crate::knowledge::search_graph;
    let results = search_graph(query)?;
    if results.is_empty() {
        Ok(format!("No graph results for: {}", query))
    } else {
        Ok(format!("Knowledge Graph results:\n{}", results.iter().map(|r| format!("  • {}", r)).collect::<Vec<_>>().join("\n")))
    }
}

// ── JSON-RPC 2.0 envelope types ──────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }

    fn into_response_bytes(self) -> (StatusCode, String) {
        (StatusCode::OK, serde_json::to_string(&self).unwrap_or_default())
    }
}

// ── Tool definitions ──────────────────────────────────────────────────────────

fn tools_list() -> Value {
    json!([
        {
            "name": "prism_count_tokens",
            "description": "Count tokens in text. Returns token count and estimated cost for common models.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":  {"type": "string", "description": "Text to count tokens in"},
                    "model": {"type": "string", "description": "Model name (default: gpt-4)", "default": "gpt-4"}
                },
                "required": ["text"]
            }
        },
        {
            "name": "prism_memory_search",
            "description": "Search PRISM Memory Palace for previously stored facts, code snippets, and context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "prism_memory_save",
            "description": "Save a key-value fact to PRISM Memory Palace for future retrieval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "key":   {"type": "string", "description": "Memory key or title"},
                    "value": {"type": "string", "description": "Memory content"}
                },
                "required": ["key", "value"]
            }
        },
        {
            "name": "prism_graph_query",
            "description": "Query the PRISM Knowledge Graph built from your codebase entities and relationships.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Graph query (entity name, relationship, or keyword)"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "prism_toon_encode",
            "description": "Encode a JSON array/object into TOON (Token-Oriented Object Notation) — reduces token count by 25-45%.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "json": {"type": "string", "description": "JSON string to encode"}
                },
                "required": ["json"]
            }
        },
        {
            "name": "prism_compress",
            "description": "Compress a long text prompt using BM25 sentence scoring — reduces token count by 30-50% while preserving key information.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":  {"type": "string", "description": "Text to compress"},
                    "ratio": {"type": "number", "description": "Target compression ratio 0.0-1.0 (default 0.7 = keep 70%)", "default": 0.7}
                },
                "required": ["text"]
            }
        }
    ])
}

// ── Tool execution ────────────────────────────────────────────────────────────

async fn call_tool(name: &str, arguments: &Value) -> (Value, bool) {
    let text_content = |s: String| json!([{"type": "text", "text": s}]);

    match name {
        "prism_count_tokens" => {
            let text = arguments.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let model = arguments.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4");
            match analytics::count_tokens(text, model) {
                Ok(count) => {
                    let cost_estimate = analytics::estimate_cost(model, count as u32, 0);
                    (text_content(format!(
                        "{} tokens ({} model)\nEstimated input cost: ${:.6}",
                        count, model, cost_estimate
                    )), false)
                }
                Err(e) => (text_content(format!("Error counting tokens: {}", e)), true),
            }
        }

        "prism_memory_search" => {
            let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            match memory_search_str(query).await {
                Ok(results) => (text_content(results), false),
                Err(e) => (text_content(format!("Memory search error: {}", e)), true),
            }
        }

        "prism_memory_save" => {
            let key = arguments.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let value = arguments.get("value").and_then(|v| v.as_str()).unwrap_or("");
            match memory_save_str(key, value).await {
                Ok(msg) => (text_content(msg), false),
                Err(e) => (text_content(format!("Memory save error: {}", e)), true),
            }
        }

        "prism_graph_query" => {
            let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            match graph_query_str(query).await {
                Ok(result) => (text_content(result), false),
                Err(e) => (text_content(format!("Graph query error: {}", e)), true),
            }
        }

        "prism_toon_encode" => {
            let json_str = arguments.get("json").and_then(|v| v.as_str()).unwrap_or("");
            match serde_json::from_str::<Value>(json_str) {
                Ok(v) => match encode::encode_json_to_toon(&v) {
                    Ok(toon) => (text_content(toon), false),
                    Err(e) => (text_content(format!("TOON encode error: {}", e)), true),
                },
                Err(e) => (text_content(format!("Invalid JSON: {}", e)), true),
            }
        }

        "prism_compress" => {
            let text = arguments.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let ratio = arguments.get("ratio").and_then(|v| v.as_f64()).unwrap_or(0.7);
            let result = crate::compress::compress(text, ratio);
            (text_content(format!(
                "Compressed: {} → {} tokens ({:.0}% reduction)\n\n{}",
                result.original_tokens,
                result.compressed_tokens,
                result.savings_pct,
                result.compressed
            )), false)
        }

        _ => (text_content(format!("Unknown tool: {}", name)), true),
    }
}

// ── JSON-RPC 2.0 dispatcher ───────────────────────────────────────────────────

async fn handle_jsonrpc(headers: HeaderMap, body: Bytes) -> Response {
    let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid UTF-8 body").into_response();
        }
    };

    let req: JsonRpcRequest = match serde_json::from_str(body_str) {
        Ok(r) => r,
        Err(e) => {
            let resp = JsonRpcResponse::err(Value::Null, -32700, format!("Parse error: {}", e));
            let (status, body) = resp.into_response_bytes();
            return (status, body).into_response();
        }
    };

    if let Some(ver) = &req.jsonrpc {
        if ver != "2.0" {
            let resp = JsonRpcResponse::err(
                req.id.clone().unwrap_or(Value::Null),
                -32600,
                "Invalid Request: jsonrpc must be '2.0'",
            );
            let (status, body) = resp.into_response_bytes();
            return (status, body).into_response();
        }
    }

    let id = req.id.clone().unwrap_or(Value::Null);

    let (status, body) = match req.method.as_str() {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "prism",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {}
                },
                "instructions": "PRISM — token optimizer. Use prism_count_tokens to estimate cost before expensive operations. Use prism_compress to shrink large prompts. Use prism_memory_save/search for cross-session context."
            });
            JsonRpcResponse::ok(id, result).into_response_bytes()
        }

        "notifications/initialized" => {
            // Acknowledgement — return empty result
            JsonRpcResponse::ok(id, json!({})).into_response_bytes()
        }

        "tools/list" => {
            let result = json!({ "tools": tools_list() });
            JsonRpcResponse::ok(id, result).into_response_bytes()
        }

        "tools/call" => {
            let params = req.params.as_ref().and_then(|p| p.as_object());
            let tool_name = params
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(json!({}));

            if tool_name.is_empty() {
                JsonRpcResponse::err(id, -32602, "Invalid params: 'name' required").into_response_bytes()
            } else {
                let (content, is_error) = call_tool(tool_name, &arguments).await;
                let result = json!({
                    "content": content,
                    "isError": is_error
                });
                JsonRpcResponse::ok(id, result).into_response_bytes()
            }
        }

        "ping" => {
            JsonRpcResponse::ok(id, json!({})).into_response_bytes()
        }

        other => {
            JsonRpcResponse::err(id, -32601, format!("Method not found: {}", other))
                .into_response_bytes()
        }
    };

    // Return with Content-Type: application/json
    axum::response::Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "").into_response())
}

async fn health() -> (StatusCode, &'static str) {
    (StatusCode::OK, r#"{"status":"ok","mcp":true,"version":"2024-11-05"}"#)
}

// ── Server entrypoint ─────────────────────────────────────────────────────────

pub async fn start_mcp_server(port: u16) -> Result<()> {
    let app = Router::new()
        .route("/", post(handle_jsonrpc))
        .route("/mcp", post(handle_jsonrpc))
        .route("/health", any(health));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("PRISM MCP server (JSON-RPC 2.0) on http://{}", addr);
    info!("Add to Claude Code: claude mcp add prism --transport http http://localhost:{}", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
