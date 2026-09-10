//! MCP server — rewritten on `rmcp` 3.2.0 (the official Rust SDK), targeting protocol
//! 2026-07-28.
//!
//! Two transports, one implementation:
//!   - `prism mcp --stdio` — a local Claude Code / same-machine agent should prefer
//!     this; prism never had a stdio transport before this rewrite.
//!   - `prism mcp --port <p>` — streamable HTTP, for the hub's remote MCP client.
//!
//! **Security.** The old server bound `0.0.0.0` unconditionally with zero
//! authentication — `prism_read_file` reachable from the whole LAN by default. This
//! rewrite binds `127.0.0.1` unless `--bind` explicitly asks for something wider, and
//! *requires* a bearer token (the hub agent token from `prism hub enroll`, or
//! `--auth-token`) whenever bound off-loopback; the server refuses to start otherwise.
//!
//! All 17 tools share their bodies with the CLI's `--json` mode through the
//! struct-returning functions in `analytics`/`memory`/`knowledge`/`cache` (Phase 1) —
//! the `*_str` helpers below are the one formatting layer both this file and the CLI
//! built on top of them use.

use anyhow::{Context, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::{
        io::stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tracing::info;

use crate::{analytics, encode, memory};

// ── Internal helpers that return String — the shared formatting layer ─────────

async fn memory_search_str(query: &str) -> anyhow::Result<String> {
    let results = memory::search_blocks(query, 10)?;
    if results.is_empty() {
        Ok(format!("No memories found for: {}", query))
    } else {
        let lines: Vec<String> = results
            .iter()
            .map(|b| {
                let preview = &b.content[..b.content.len().min(200)];
                format!("[{}] {}: {}", b.layer, b.category, preview)
            })
            .collect();
        Ok(lines.join("\n"))
    }
}

async fn memory_save_str(key: &str, value: &str) -> anyhow::Result<String> {
    let mut palace = memory::MemoryPalace::new(memory::memory_palace_dir_pub())
        .map_err(|e| anyhow::anyhow!(e))?;
    palace.save(memory::MemoryLayer::Core, value, key);
    Ok(format!("Saved: {}", key))
}

async fn graph_query_str(
    query: &str,
    custom_path: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    if let Some((path, matches)) = crate::knowledge::query_graph_data(query, 8, custom_path) {
        if !matches.is_empty() {
            let mut lines = vec![format!("Graph Query Results (Source: {}):", path.display())];
            lines.extend(
                matches
                    .iter()
                    .map(|n| format!("  • [{}] {} (path: {})", n.kind, n.label, n.path)),
            );
            return Ok(lines.join("\n"));
        }
    }
    use crate::knowledge::search_graph_with_path;
    let results = search_graph_with_path(query, custom_path)?;
    if results.is_empty() {
        Ok(format!("No graph results for: {}", query))
    } else {
        Ok(format!(
            "Knowledge Graph results:\n{}",
            results
                .iter()
                .map(|r| format!("  • {}", r))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

async fn graph_explain_str(
    node: &str,
    custom_path: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    let Some((path, exp)) = crate::knowledge::explain_node_data(node, custom_path) else {
        return Ok(match crate::knowledge::graph_unavailable(custom_path) {
            Some(why) => why.message(),
            None => format!("Node '{}' not found in graph.", node),
        });
    };

    let mut lines = Vec::new();
    lines.push(format!("Node: {} (id: {})", exp.node.label, exp.node.id));
    lines.push(format!("Source Graph: {}", path.display()));
    lines.push(format!("Kind: {} | Path: {}", exp.node.kind, exp.node.path));
    if let Some(comm) = exp.node.community {
        lines.push(format!("Community: {}", comm));
    }
    lines.push(format!(
        "Total Degree: {}",
        exp.outgoing.len() + exp.incoming.len()
    ));

    if !exp.outgoing.is_empty() {
        lines.push(format!("\nOutgoing Connections ({}):", exp.outgoing.len()));
        for (target, kind, weight) in exp.outgoing.iter().take(15) {
            lines.push(format!(
                "  --> {} [{}] (w: {:.1}) in {}",
                target.label, kind, weight, target.path
            ));
        }
    }
    if !exp.incoming.is_empty() {
        lines.push(format!("\nIncoming Connections ({}):", exp.incoming.len()));
        for (source, kind, weight) in exp.incoming.iter().take(15) {
            lines.push(format!(
                "  <-- {} [{}] (w: {:.1}) in {}",
                source.label, kind, weight, source.path
            ));
        }
    }
    Ok(lines.join("\n"))
}

async fn graph_path_str(
    from: &str,
    to: &str,
    custom_path: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    match crate::knowledge::shortest_path_data(from, to, custom_path) {
        Some((_, steps)) if steps.is_empty() => {
            Ok(format!("Identical node: '{}' is '{}'.", from, to))
        }
        Some((path, steps)) => {
            let mut lines = vec![format!(
                "Shortest path in {} ({} hops):",
                path.display(),
                steps.len()
            )];
            for (i, (src, rel, tgt)) in steps.iter().enumerate() {
                lines.push(format!(
                    "  [{}] {} --[{}]--> {}",
                    i + 1,
                    src.label,
                    rel,
                    tgt.label
                ));
            }
            Ok(lines.join("\n"))
        }
        None => Ok(match crate::knowledge::graph_unavailable(custom_path) {
            Some(why) => why.message(),
            None => format!("No path found between '{}' and '{}' in graph.", from, to),
        }),
    }
}

async fn graph_god_nodes_str(
    top: usize,
    custom_path: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    let Some((path, hubs)) = crate::knowledge::god_nodes_data(top, custom_path) else {
        return Ok(crate::knowledge::graph_unavailable(custom_path)
            .unwrap_or(crate::knowledge::GraphUnavailable::NotFound)
            .message());
    };

    let mut lines = vec![format!(
        "God Nodes / Architectural Hubs (Source: {}):",
        path.display()
    )];
    for (i, (node, degree)) in hubs.iter().enumerate() {
        lines.push(format!(
            "  {:2}. {:<25} {:>3} edges [{}] in {}",
            i + 1,
            node.label,
            degree,
            node.kind,
            node.path
        ));
    }
    Ok(lines.join("\n"))
}

// ── Tool request parameter shapes ──────────────────────────────────────────────
// One struct per tool that takes arguments. `JsonSchema` (re-exported from rmcp, so
// this always matches whatever schemars version the SDK itself was built against)
// drives the `inputSchema` the client sees; `Deserialize` drives the actual parse.

fn default_model() -> String {
    "gpt-4".to_string()
}
fn default_read_mode() -> String {
    "skeleton".to_string()
}
fn default_top() -> usize {
    10
}
fn default_ratio() -> f64 {
    0.7
}
fn default_cache_model() -> String {
    "mcp".to_string()
}
fn default_limit() -> usize {
    3
}
fn default_index_path() -> String {
    ".".to_string()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CountTokensRequest {
    /// Text to count tokens in
    pub text: String,
    /// Model name (default: gpt-4)
    #[serde(default = "default_model")]
    pub model: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadFileRequest {
    /// Path to file
    pub path: String,
    /// Mode: skeleton, map, clean, diff, lines, cached, full
    #[serde(default = "default_read_mode")]
    pub mode: String,
    /// Line range (e.g. 10-50) for lines mode
    #[serde(default)]
    pub lines: Option<String>,
    /// Include line numbers
    #[serde(default)]
    pub line_numbers: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FilterCmdRequest {
    /// Original command (e.g. 'git status' or 'cargo test')
    pub command: String,
    /// Raw stdout/stderr output
    pub output: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MemorySearchRequest {
    /// Search query
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MemorySaveRequest {
    /// Memory key or title
    pub key: String,
    /// Memory content
    pub value: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphQueryRequest {
    /// Graph query (entity name, relationship, function, or keyword)
    pub query: String,
    /// Optional path to existing graph.json (e.g. 'graphify-out/graph.json')
    #[serde(default)]
    pub graph: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphExplainRequest {
    /// Node name, symbol, or file path to explain
    pub node: String,
    #[serde(default)]
    pub graph: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphPathRequest {
    /// Source node or symbol
    pub from: String,
    /// Target node or symbol
    pub to: String,
    #[serde(default)]
    pub graph: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphGodNodesRequest {
    /// Top N nodes to return (default: 10)
    #[serde(default = "default_top")]
    pub top: usize,
    #[serde(default)]
    pub graph: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphImportRequest {
    /// Path to graph.json
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphIndexRequest {
    /// Directory path to index (default: '.')
    #[serde(default = "default_index_path")]
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ToonEncodeRequest {
    /// JSON string to encode
    pub json: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompressRequest {
    /// Text to compress
    pub text: String,
    /// Target compression ratio 0.0-1.0 (default 0.7 = keep 70%)
    #[serde(default = "default_ratio")]
    pub ratio: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CacheSaveRequest {
    /// Prompt text to cache
    pub prompt: String,
    /// Response text to cache
    pub response: String,
    #[serde(default = "default_cache_model")]
    pub model: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CacheLookupRequest {
    /// Prompt or query string
    pub query: String,
    /// Max results to return (default: 3)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

// ── Server ──────────────────────────────────────────────────────────────────────

fn text_ok(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s.into())]))
}

fn text_err(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(s.into())]))
}

/// The MCP form of the GraphRAG policy gate.
///
/// `Some(_)` short-circuits the tool as an *error* carrying the enrolment hint. It has
/// to be an error, not empty text: an agent reads "No graph results" as a fact about
/// the codebase and moves on, whereas a disabled feature is a fact about this machine
/// that the operator can fix.
fn graph_gate() -> Option<Result<CallToolResult, McpError>> {
    crate::knowledge::ensure_enabled()
        .err()
        .map(|e| text_err(e.to_string()))
}

/// The MCP form of the semantic-cache gate, covering both the disabled and the
/// store-locked reason. Same argument: an agent must not read either as "nothing
/// cached".
fn cache_gate() -> Option<Result<CallToolResult, McpError>> {
    crate::cache::unavailable().map(|why| text_err(why.message()))
}

#[derive(Clone)]
pub struct PrismMcpServer {
    // Read by the #[tool_handler]-generated ServerHandler::list_tools/call_tool through
    // macro-expanded code the dead-code lint doesn't trace back to this field — verified
    // functionally instead: a live server correctly lists and executes all 17 tools.
    #[allow(dead_code)]
    tool_router: ToolRouter<PrismMcpServer>,
}

impl Default for PrismMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl PrismMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Count tokens in text. Returns token count and estimated cost for common models."
    )]
    async fn prism_count_tokens(
        &self,
        Parameters(req): Parameters<CountTokensRequest>,
    ) -> Result<CallToolResult, McpError> {
        match analytics::count_tokens(&req.text, &req.model) {
            Ok(count) => {
                let cost = analytics::estimate_cost(&req.model, count as u32, 0);
                text_ok(format!(
                    "{} tokens ({} model)\nEstimated input cost: ${:.6}",
                    count, req.model, cost
                ))
            }
            Err(e) => text_err(format!("Error counting tokens: {e}")),
        }
    }

    #[tool(
        description = "Intelligent 7-mode file reader. Modes: 'skeleton' (AST signatures, omits bodies, 70-85% savings), 'map' (outline), 'clean' (no comments), 'diff' (git diff against HEAD), 'lines' (range N-M), 'cached' (~15 token receipt if unchanged), 'full'."
    )]
    async fn prism_read_file(
        &self,
        Parameters(req): Parameters<ReadFileRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mode = if let Some(range) = &req.lines {
            crate::reader::parse_lines_range(range).unwrap_or(crate::reader::ReadMode::Skeleton)
        } else {
            req.mode
                .parse::<crate::reader::ReadMode>()
                .unwrap_or(crate::reader::ReadMode::Skeleton)
        };
        match crate::reader::read_file(std::path::Path::new(&req.path), mode, req.line_numbers) {
            Ok(out) => {
                let header = format!(
                    "// Mode: {} | Tokens: {} -> {} ({:.1}% saved)\n\n",
                    out.mode_used, out.original_tokens, out.returned_tokens, out.savings_pct
                );
                text_ok(format!("{header}{}", out.content))
            }
            Err(e) => text_err(format!("Read error: {e}")),
        }
    }

    #[tool(
        description = "Filter raw shell/CLI command output using 65+ RTK-compatible filters (git, cargo, pytest, tsc, docker, k8s, etc.)."
    )]
    async fn prism_filter_cmd(
        &self,
        Parameters(req): Parameters<FilterCmdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let parts: Vec<&str> = req.command.split_whitespace().collect();
        let cmd = parts.first().copied().unwrap_or("");
        let args: Vec<String> = parts.iter().skip(1).map(|s| s.to_string()).collect();
        let filtered = crate::filter::filter_output(&req.output, cmd, &args);
        text_ok(filtered.into_owned())
    }

    #[tool(
        description = "Search PRISM Memory Palace for previously stored facts, code snippets, and context."
    )]
    async fn prism_memory_search(
        &self,
        Parameters(req): Parameters<MemorySearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        match memory_search_str(&req.query).await {
            Ok(s) => text_ok(s),
            Err(e) => text_err(format!("Memory search error: {e}")),
        }
    }

    #[tool(description = "Save a key-value fact to PRISM Memory Palace for future retrieval.")]
    async fn prism_memory_save(
        &self,
        Parameters(req): Parameters<MemorySaveRequest>,
    ) -> Result<CallToolResult, McpError> {
        match memory_save_str(&req.key, &req.value).await {
            Ok(s) => text_ok(s),
            Err(e) => text_err(format!("Memory save error: {e}")),
        }
    }

    #[tool(description = "View Memory Palace statistics across Recall, Core, and Archive tiers.")]
    async fn prism_memory_stats(&self) -> Result<CallToolResult, McpError> {
        match memory::stats_data() {
            Ok(stats) => text_ok(format!(
                "Memory Palace Statistics:\n  Recall:  {} blocks\n  Core:    {} blocks\n  Archive: {} blocks",
                stats.recall, stats.core, stats.archive
            )),
            Err(e) => text_err(format!("Memory stats error: {e}")),
        }
    }

    #[tool(
        description = "Query the PRISM Knowledge Graph or any existing Graphify graph (graphify-out/graph.json) using Corrective RAG (CRAG)."
    )]
    async fn prism_graph_query(
        &self,
        Parameters(req): Parameters<GraphQueryRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = graph_gate() {
            return off;
        }
        let graph_path = req.graph.as_deref().map(std::path::Path::new);
        match graph_query_str(&req.query, graph_path).await {
            Ok(s) => text_ok(s),
            Err(e) => text_err(format!("Graph query error: {e}")),
        }
    }

    #[tool(
        description = "Explain a codebase node/symbol and inspect its incoming/outgoing dependencies (compatible with Graphify and PRISM graphs)."
    )]
    async fn prism_graph_explain(
        &self,
        Parameters(req): Parameters<GraphExplainRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = graph_gate() {
            return off;
        }
        let graph_path = req.graph.as_deref().map(std::path::Path::new);
        match graph_explain_str(&req.node, graph_path).await {
            Ok(s) => text_ok(s),
            Err(e) => text_err(format!("Graph explain error: {e}")),
        }
    }

    #[tool(
        description = "Find the shortest dependency call/import path between two nodes in the codebase graph."
    )]
    async fn prism_graph_path(
        &self,
        Parameters(req): Parameters<GraphPathRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = graph_gate() {
            return off;
        }
        let graph_path = req.graph.as_deref().map(std::path::Path::new);
        match graph_path_str(&req.from, &req.to, graph_path).await {
            Ok(s) => text_ok(s),
            Err(e) => text_err(format!("Graph path error: {e}")),
        }
    }

    #[tool(
        description = "List the most connected architectural hub nodes in the graph (degree centrality)."
    )]
    async fn prism_graph_god_nodes(
        &self,
        Parameters(req): Parameters<GraphGodNodesRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = graph_gate() {
            return off;
        }
        let graph_path = req.graph.as_deref().map(std::path::Path::new);
        match graph_god_nodes_str(req.top, graph_path).await {
            Ok(s) => text_ok(s),
            Err(e) => text_err(format!("Graph god nodes error: {e}")),
        }
    }

    #[tool(
        description = "Import and activate any existing Graphify (graphify-out/graph.json) or NetworkX graph into PRISM."
    )]
    async fn prism_graph_import(
        &self,
        Parameters(req): Parameters<GraphImportRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = graph_gate() {
            return off;
        }
        match crate::knowledge::import_graph(std::path::Path::new(&req.path)).await {
            Ok(_) => text_ok(format!(
                "Successfully imported and activated graph from: {}",
                req.path
            )),
            Err(e) => text_err(format!("Graph import error: {e}")),
        }
    }

    #[tool(
        description = "Index a codebase directory into the PRISM GraphRAG dependency graph (extracts files, functions, types, and imports)."
    )]
    async fn prism_graph_index(
        &self,
        Parameters(req): Parameters<GraphIndexRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = graph_gate() {
            return off;
        }
        match crate::knowledge::index_codebase(std::path::Path::new(&req.path)).await {
            Ok(_) => text_ok(format!("Successfully indexed codebase at '{}'", req.path)),
            Err(e) => text_err(format!("Indexing error: {e}")),
        }
    }

    #[tool(
        description = "Encode a JSON array/object into TOON (Token-Oriented Object Notation) — reduces token count by 25-45%."
    )]
    async fn prism_toon_encode(
        &self,
        Parameters(req): Parameters<ToonEncodeRequest>,
    ) -> Result<CallToolResult, McpError> {
        match serde_json::from_str::<serde_json::Value>(&req.json) {
            Ok(v) => match encode::encode_json_to_toon(&v) {
                Ok(toon) => text_ok(toon),
                Err(e) => text_err(format!("TOON encode error: {e}")),
            },
            Err(e) => text_err(format!("Invalid JSON: {e}")),
        }
    }

    #[tool(
        description = "Compress a long text prompt using BM25 sentence scoring — reduces token count by 30-50% while preserving key information."
    )]
    async fn prism_compress(
        &self,
        Parameters(req): Parameters<CompressRequest>,
    ) -> Result<CallToolResult, McpError> {
        let result = crate::compress::compress(&req.text, req.ratio);
        text_ok(format!(
            "Compressed: {} → {} tokens ({:.0}% reduction)\n\n{}",
            result.original_tokens, result.compressed_tokens, result.savings_pct, result.compressed
        ))
    }

    #[tool(
        description = "Store a prompt-response pair into the PRISM Semantic Cache and TurboVec ANN vector index."
    )]
    async fn prism_cache_save(
        &self,
        Parameters(req): Parameters<CacheSaveRequest>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(off) = cache_gate() {
            return off;
        }
        if req.prompt.is_empty() || req.response.is_empty() {
            return text_err("prompt and response are required");
        }
        crate::cache::cache_response(&req.prompt, &req.response, &req.model);
        text_ok(format!(
            "Cached response in TurboVec for prompt: {}",
            req.prompt
        ))
    }

    #[tool(
        description = "Query the PRISM Semantic Cache using TurboVec ANN search for similar past prompts and responses."
    )]
    async fn prism_cache_lookup(
        &self,
        Parameters(req): Parameters<CacheLookupRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Neither a disabled cache nor an unreachable one is an empty one; an agent
        // must not read either as "nothing cached".
        if let Some(off) = cache_gate() {
            return off;
        }
        let results = crate::cache::lookup_similar(&req.query, req.limit);
        if results.is_empty() {
            text_ok(format!("No cached entries found for: {}", req.query))
        } else {
            let formatted = results
                .iter()
                .map(|hit| {
                    format!(
                        "[Cache: {} | {:.0}% match | model: {}] {}",
                        hit.entry.key_hash,
                        hit.score * 100.0,
                        hit.entry.model.as_deref().unwrap_or("unknown"),
                        hit.entry.response
                    )
                })
                .collect::<Vec<_>>()
                .join("\n---\n");
            text_ok(formatted)
        }
    }

    #[tool(
        description = "Get a summary of PRISM token savings, cost economics, and semantic cache status."
    )]
    async fn prism_analytics_summary(&self) -> Result<CallToolResult, McpError> {
        // `gain` and analytics are free, so this tool still answers — but it reports the
        // cache's actual state rather than `Cache Entries: 0`, which would read as an
        // empty cache when the cache is simply not enabled here.
        let cache_line = match crate::cache::unavailable() {
            Some(crate::cache::Unavailable::Disabled) => {
                "  Semantic cache: disabled (a PRISM Hub feature — `prism hub enroll` enables it)"
                    .to_string()
            }
            Some(crate::cache::Unavailable::Unopenable(e)) => {
                format!("  Semantic cache: unreachable ({e})")
            }
            None => {
                let stats = crate::cache::get_cache_stats();
                format!(
                    "  Cache Entries: {}\n  Cache Location: {}",
                    stats.total_entries, stats.sled_path
                )
            }
        };
        text_ok(format!("PRISM Analytics Summary:\n{cache_line}"))
    }
}

#[tool_handler]
impl ServerHandler for PrismMcpServer {
    fn get_info(&self) -> ServerInfo {
        // NOT Implementation::from_build_env(): that helper's env!() calls are baked in
        // where it's *defined* (inside the rmcp crate itself), so it always reports
        // "rmcp"/rmcp's own version rather than ours. env!("CARGO_PKG_VERSION") here,
        // in prism's own source, resolves against prism's Cargo.toml instead.
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("prism", env!("CARGO_PKG_VERSION")))
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .with_instructions(
                "PRISM — token optimizer. Use prism_count_tokens to estimate cost before \
                 expensive operations. Use prism_compress to shrink large prompts. Use \
                 prism_memory_save/search for cross-session context."
                    .to_string(),
            )
    }
}

// ── Auth middleware (required off-loopback, optional on it) ───────────────────

async fn require_bearer(
    axum::extract::State(expected): axum::extract::State<Arc<str>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|got| got == expected.as_ref());
    if ok {
        next.run(req).await
    } else {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            "unauthorized — missing or invalid bearer token",
        )
            .into_response()
    }
}

async fn health() -> (axum::http::StatusCode, &'static str) {
    (
        axum::http::StatusCode::OK,
        concat!(
            r#"{"status":"ok","mcp":true,"transport":"streamable-http","version":""#,
            env!("CARGO_PKG_VERSION"),
            r#""}"#
        ),
    )
}

// ── Server entrypoints ────────────────────────────────────────────────────────

/// `prism mcp --stdio` — the transport a local Claude Code / same-machine agent
/// should prefer. No network surface at all.
async fn serve_stdio() -> Result<()> {
    info!("PRISM MCP server on stdio");
    let service = PrismMcpServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// `prism mcp --port <p> [--bind <addr>] [--auth-token <token>]` — streamable HTTP,
/// for the hub's remote MCP client. Loopback by default; going wider requires an
/// explicit `--bind` and a bearer token (`--auth-token`, or the hub agent token from
/// `prism hub enroll`) — the server refuses to start otherwise.
async fn serve_http(port: u16, bind: &str, auth_token: Option<String>) -> Result<()> {
    let bind_ip: IpAddr = bind
        .parse()
        .with_context(|| format!("invalid --bind address: {bind}"))?;
    let off_loopback = !bind_ip.is_loopback();

    let token = auth_token.or_else(|| crate::hub::load_credentials().map(|c| c.agent_token));
    if off_loopback && token.is_none() {
        anyhow::bail!(
            "refusing to bind {bind} (not loopback) without a bearer token — pass \
             --auth-token <token>, or run `prism hub enroll` first. An unauthenticated \
             tool server (prism_read_file included) must not be reachable on the LAN."
        );
    }

    let ct = tokio_util::sync::CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(PrismMcpServer::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
    );

    let mut router = axum::Router::new().nest_service("/mcp", service);
    if let Some(token) = token {
        let state: Arc<str> = Arc::from(token.as_str());
        router = router.layer(axum::middleware::from_fn_with_state(state, require_bearer));
        info!("PRISM MCP server: bearer auth required on /mcp");
    } else {
        info!(
            "PRISM MCP server: no auth token configured (loopback-only bind, so this is not a LAN exposure)"
        );
    }
    router = router.route("/health", axum::routing::any(health));

    let addr = SocketAddr::new(bind_ip, port);
    info!("PRISM MCP server (streamable HTTP, protocol 2026-07-28) on http://{addr}");
    if off_loopback {
        info!(
            "Add to the hub: a remote MCP client pointed at http://{addr}/mcp with the bearer token above"
        );
    } else {
        info!("Add to Claude Code: claude mcp add prism --transport http http://{addr}/mcp");
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            ct.cancel();
        })
        .await?;
    Ok(())
}

pub async fn start_mcp_server(
    port: u16,
    stdio: bool,
    bind: String,
    auth_token: Option<String>,
) -> Result<()> {
    if stdio {
        serve_stdio().await
    } else {
        serve_http(port, &bind, auth_token).await
    }
}
