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

async fn graph_query_str(query: &str, custom_path: Option<&std::path::Path>) -> anyhow::Result<String> {
    use crate::knowledge::search_graph_with_path;
    let results = search_graph_with_path(query, custom_path)?;
    if results.is_empty() {
        Ok(format!("No graph results for: {}", query))
    } else {
        Ok(format!("Knowledge Graph results:\n{}", results.iter().map(|r| format!("  • {}", r)).collect::<Vec<_>>().join("\n")))
    }
}

async fn graph_explain_str(node: &str, custom_path: Option<&std::path::Path>) -> anyhow::Result<String> {
    let (path, rag) = match crate::knowledge::find_active_graph(custom_path) {
        Some(pair) => pair,
        None => return Ok("No active graph found. Run `prism graph index` or provide a graph path.".to_string()),
    };

    match rag.explain_node(node) {
        Some(exp) => {
            let mut lines = Vec::new();
            lines.push(format!("Node: {} (id: {})", exp.node.label, exp.node.id));
            lines.push(format!("Source Graph: {}", path.display()));
            lines.push(format!("Kind: {} | Path: {}", exp.node.kind, exp.node.path));
            if let Some(comm) = exp.node.community {
                lines.push(format!("Community: {}", comm));
            }
            lines.push(format!("Total Degree: {}", exp.outgoing.len() + exp.incoming.len()));

            if !exp.outgoing.is_empty() {
                lines.push(format!("\nOutgoing Connections ({}):", exp.outgoing.len()));
                for (target, kind, weight) in exp.outgoing.iter().take(15) {
                    lines.push(format!("  --> {} [{}] (w: {:.1}) in {}", target.label, kind, weight, target.path));
                }
            }
            if !exp.incoming.is_empty() {
                lines.push(format!("\nIncoming Connections ({}):", exp.incoming.len()));
                for (source, kind, weight) in exp.incoming.iter().take(15) {
                    lines.push(format!("  <-- {} [{}] (w: {:.1}) in {}", source.label, kind, weight, source.path));
                }
            }
            Ok(lines.join("\n"))
        }
        None => Ok(format!("Node '{}' not found in graph ({})", node, path.display())),
    }
}

async fn graph_path_str(from: &str, to: &str, custom_path: Option<&std::path::Path>) -> anyhow::Result<String> {
    let (path, rag) = match crate::knowledge::find_active_graph(custom_path) {
        Some(pair) => pair,
        None => return Ok("No active graph found.".to_string()),
    };

    match rag.shortest_path(from, to) {
        Some(steps) if steps.is_empty() => Ok(format!("Identical node: '{}' is '{}'.", from, to)),
        Some(steps) => {
            let mut lines = vec![format!("Shortest path in {} ({} hops):", path.display(), steps.len())];
            for (i, (src, rel, tgt)) in steps.iter().enumerate() {
                lines.push(format!("  [{}] {} --[{}]--> {}", i + 1, src.label, rel, tgt.label));
            }
            Ok(lines.join("\n"))
        }
        None => Ok(format!("No path found between '{}' and '{}' in graph.", from, to)),
    }
}

async fn graph_god_nodes_str(top: usize, custom_path: Option<&std::path::Path>) -> anyhow::Result<String> {
    let (path, rag) = match crate::knowledge::find_active_graph(custom_path) {
        Some(pair) => pair,
        None => return Ok("No active graph found.".to_string()),
    };

    let hubs = rag.god_nodes(top);
    let mut lines = vec![format!("God Nodes / Architectural Hubs (Source: {}):", path.display())];
    for (i, (node, degree)) in hubs.iter().enumerate() {
        lines.push(format!("  {:2}. {:<25} {:>3} edges [{}] in {}", i + 1, node.label, degree, node.kind, node.path));
    }
    Ok(lines.join("\n"))
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
            "name": "prism_read_file",
            "description": "Intelligent 7-mode file reader. Modes: 'skeleton' (AST signatures, omits bodies, 70-85% savings), 'map' (outline), 'clean' (no comments), 'diff' (git diff against HEAD), 'lines' (range N-M), 'cached' (~15 token receipt if unchanged), 'full'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to file"},
                    "mode": {"type": "string", "description": "Mode: skeleton, map, clean, diff, lines, cached, full", "default": "skeleton"},
                    "lines": {"type": "string", "description": "Line range (e.g. 10-50) for lines mode"},
                    "line_numbers": {"type": "boolean", "description": "Include line numbers", "default": false}
                },
                "required": ["path"]
            }
        },
        {
            "name": "prism_filter_cmd",
            "description": "Filter raw shell/CLI command output using 65+ RTK-compatible filters (git, cargo, pytest, tsc, docker, k8s, etc.).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Original command (e.g. 'git status' or 'cargo test')"},
                    "output":  {"type": "string", "description": "Raw stdout/stderr output"}
                },
                "required": ["command", "output"]
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
            "name": "prism_memory_stats",
            "description": "View Memory Palace statistics across Recall, Core, and Archive tiers.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "prism_graph_query",
            "description": "Query the PRISM Knowledge Graph or any existing Graphify graph (graphify-out/graph.json) using Corrective RAG (CRAG).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Graph query (entity name, relationship, function, or keyword)"},
                    "graph": {"type": "string", "description": "Optional path to existing graph.json (e.g. 'graphify-out/graph.json')"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "prism_graph_explain",
            "description": "Explain a codebase node/symbol and inspect its incoming/outgoing dependencies (compatible with Graphify and PRISM graphs).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node": {"type": "string", "description": "Node name, symbol, or file path to explain"},
                    "graph": {"type": "string", "description": "Optional path to existing graph.json"}
                },
                "required": ["node"]
            }
        },
        {
            "name": "prism_graph_path",
            "description": "Find the shortest dependency call/import path between two nodes in the codebase graph.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": {"type": "string", "description": "Source node or symbol"},
                    "to": {"type": "string", "description": "Target node or symbol"},
                    "graph": {"type": "string", "description": "Optional path to existing graph.json"}
                },
                "required": ["from", "to"]
            }
        },
        {
            "name": "prism_graph_god_nodes",
            "description": "List the most connected architectural hub nodes in the graph (degree centrality).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "top": {"type": "integer", "description": "Top N nodes to return (default: 10)", "default": 10},
                    "graph": {"type": "string", "description": "Optional path to existing graph.json"}
                }
            }
        },
        {
            "name": "prism_graph_import",
            "description": "Import and activate any existing Graphify (graphify-out/graph.json) or NetworkX graph into PRISM.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to graph.json"}
                },
                "required": ["path"]
            }
        },
        {
            "name": "prism_graph_index",
            "description": "Index a codebase directory into the PRISM GraphRAG dependency graph (extracts files, functions, types, and imports).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path to index (default: '.')", "default": "."}
                }
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
        },
        {
            "name": "prism_cache_save",
            "description": "Store a prompt-response pair into the PRISM Semantic Cache and TurboVec ANN vector index.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Prompt text to cache"},
                    "response": {"type": "string", "description": "Response text to cache"},
                    "model": {"type": "string", "description": "Model name (default: gpt-4)", "default": "gpt-4"}
                },
                "required": ["prompt", "response"]
            }
        },
        {
            "name": "prism_cache_lookup",
            "description": "Query the PRISM Semantic Cache using TurboVec ANN search for similar past prompts and responses.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Prompt or query string"},
                    "limit": {"type": "integer", "description": "Max results to return (default: 3)", "default": 3}
                },
                "required": ["query"]
            }
        },
        {
            "name": "prism_analytics_summary",
            "description": "Get a summary of PRISM token savings, cost economics, and semantic cache status.",
            "inputSchema": {
                "type": "object",
                "properties": {}
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
            let graph_path = arguments.get("graph").and_then(|v| v.as_str()).map(std::path::Path::new);
            match graph_query_str(query, graph_path).await {
                Ok(result) => (text_content(result), false),
                Err(e) => (text_content(format!("Graph query error: {}", e)), true),
            }
        }

        "prism_graph_explain" => {
            let node = arguments.get("node").and_then(|v| v.as_str()).unwrap_or("");
            let graph_path = arguments.get("graph").and_then(|v| v.as_str()).map(std::path::Path::new);
            match graph_explain_str(node, graph_path).await {
                Ok(result) => (text_content(result), false),
                Err(e) => (text_content(format!("Graph explain error: {}", e)), true),
            }
        }

        "prism_graph_path" => {
            let from = arguments.get("from").and_then(|v| v.as_str()).unwrap_or("");
            let to = arguments.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let graph_path = arguments.get("graph").and_then(|v| v.as_str()).map(std::path::Path::new);
            match graph_path_str(from, to, graph_path).await {
                Ok(result) => (text_content(result), false),
                Err(e) => (text_content(format!("Graph path error: {}", e)), true),
            }
        }

        "prism_graph_god_nodes" => {
            let top = arguments.get("top").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let graph_path = arguments.get("graph").and_then(|v| v.as_str()).map(std::path::Path::new);
            match graph_god_nodes_str(top, graph_path).await {
                Ok(result) => (text_content(result), false),
                Err(e) => (text_content(format!("Graph god nodes error: {}", e)), true),
            }
        }

        "prism_graph_import" => {
            let path_str = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let path = std::path::Path::new(path_str);
            match crate::knowledge::import_graph(path).await {
                Ok(_) => (text_content(format!("Successfully imported and activated graph from: {}", path_str)), false),
                Err(e) => (text_content(format!("Graph import error: {}", e)), true),
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

        "prism_read_file" => {
            let path_str = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mode_str = arguments.get("mode").and_then(|v| v.as_str()).unwrap_or("skeleton");
            let lines_opt = arguments.get("lines").and_then(|v| v.as_str());
            let line_numbers = arguments.get("line_numbers").and_then(|v| v.as_bool()).unwrap_or(false);

            let mode = if let Some(range) = lines_opt {
                crate::reader::parse_lines_range(range).unwrap_or(crate::reader::ReadMode::Skeleton)
            } else {
                mode_str.parse::<crate::reader::ReadMode>().unwrap_or(crate::reader::ReadMode::Skeleton)
            };

            match crate::reader::read_file(std::path::Path::new(path_str), mode, line_numbers) {
                Ok(out) => {
                    let header = format!(
                        "// Mode: {} | Tokens: {} -> {} ({:.1}% saved)\n\n",
                        out.mode_used, out.original_tokens, out.returned_tokens, out.savings_pct
                    );
                    (text_content(format!("{}{}", header, out.content)), false)
                }
                Err(e) => (text_content(format!("Read error: {}", e)), true),
            }
        }

        "prism_filter_cmd" => {
            let cmd_str = arguments.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let output_str = arguments.get("output").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = cmd_str.split_whitespace().collect();
            let cmd = parts.first().copied().unwrap_or("");
            let args: Vec<String> = parts.iter().skip(1).map(|s| s.to_string()).collect();
            let filtered = crate::filter::filter_output(output_str, cmd, &args);
            (text_content(filtered.into_owned()), false)
        }

        "prism_memory_stats" => {
            match memory::MemoryPalace::new(memory::memory_palace_dir_pub()) {
                Ok(palace) => {
                    let stats_str = format!(
                        "Memory Palace Statistics:\n  Recall:  {} blocks\n  Core:    {} blocks\n  Archive: {} blocks",
                        palace.count(memory::MemoryLayer::Recall),
                        palace.count(memory::MemoryLayer::Core),
                        palace.count(memory::MemoryLayer::Archive),
                    );
                    (text_content(stats_str), false)
                }
                Err(e) => (text_content(format!("Memory stats error: {}", e)), true),
            }
        }

        "prism_graph_index" => {
            let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            match crate::knowledge::index_codebase(std::path::Path::new(path)).await {
                Ok(_) => (text_content(format!("Successfully indexed codebase at '{}'", path)), false),
                Err(e) => (text_content(format!("Indexing error: {}", e)), true),
            }
        }

        "prism_cache_save" => {
            let prompt = arguments.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            let response = arguments.get("response").and_then(|v| v.as_str()).unwrap_or("");
            let model = arguments.get("model").and_then(|v| v.as_str()).unwrap_or("mcp");
            if prompt.is_empty() || response.is_empty() {
                (text_content("prompt and response are required".to_string()), true)
            } else {
                crate::cache::cache_response(prompt, response, model);
                (text_content(format!("Cached response in TurboVec for prompt: {}", prompt)), false)
            }
        }

        "prism_cache_lookup" => {
            let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = arguments.get("limit").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let results = crate::cache::lookup_similar(query, limit);
            if results.is_empty() {
                (text_content(format!("No cached entries found for: {}", query)), false)
            } else {
                let formatted = results.iter().map(|e| {
                    format!("[Cache: {} | model: {}] {}", e.key_hash, e.model.as_deref().unwrap_or("unknown"), e.response)
                }).collect::<Vec<_>>().join("\n---\n");
                (text_content(formatted), false)
            }
        }

        "prism_analytics_summary" => {
            let stats = crate::cache::get_cache_stats();
            let report = format!(
                "PRISM Analytics Summary:\n  Cache Entries: {}\n  Cache Location: {}",
                stats.total_entries, stats.sled_path
            );
            (text_content(report), false)
        }

        _ => (text_content(format!("Unknown tool: {}", name)), true),
    }
}

// ── JSON-RPC 2.0 dispatcher ───────────────────────────────────────────────────

async fn handle_jsonrpc(_headers: HeaderMap, body: Bytes) -> Response {
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
