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

/// Tools refused to an off-loopback caller.
///
/// The hub drives the Memory and Graph pages over MCP and needs neither of these.
/// `prism_read_file` reads any path this user can read, and `prism_filter_cmd` runs a
/// command. Exposing either to satisfy a stats page is privilege nobody asked for, so
/// the remote surface is narrowed rather than trusted to a single bearer token.
const REMOTE_DENIED_TOOLS: &[&str] = &["prism_read_file", "prism_filter_cmd"];

#[derive(Clone)]
pub struct PrismMcpServer {
    // Read by the #[tool_handler]-generated ServerHandler::list_tools/call_tool through
    // macro-expanded code the dead-code lint doesn't trace back to this field — verified
    // functionally instead: a live server correctly lists and executes all 17 tools.
    #[allow(dead_code)]
    tool_router: ToolRouter<PrismMcpServer>,
    /// True when this server is bound off-loopback, i.e. every caller is remote.
    remote: bool,
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
            remote: false,
        }
    }

    /// Refuse filesystem- and process-reaching tools, for a server bound off-loopback.
    pub fn with_remote_restrictions(mut self, remote: bool) -> Self {
        self.remote = remote;
        self
    }

    /// `Some(err)` when this tool is not available to the current caller.
    fn refuse_if_remote(&self, tool: &str) -> Option<Result<CallToolResult, McpError>> {
        if self.remote && REMOTE_DENIED_TOOLS.contains(&tool) {
            return Some(text_err(format!(
                "{tool} is not available to remote callers. This PRISM MCP server is \
                 bound off-loopback, where it exposes only the memory and graph tools. \
                 Run the tool on the agent machine instead."
            )));
        }
        None
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
        if let Some(refusal) = self.refuse_if_remote("prism_read_file") {
            return refusal;
        }
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
        if let Some(refusal) = self.refuse_if_remote("prism_filter_cmd") {
            return refusal;
        }
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

/// Which bearer token, if any, guards the HTTP transport.
///
/// Blank is not a token. Each source is filtered independently rather than
/// `explicit.or(stored)` on the raw values, so an empty `--auth-token ""` — what a
/// shell produces from an unset variable — falls through to an enrolled agent token
/// instead of shadowing it with nothing. That is the documented behaviour: the flag,
/// *or* the credential from `prism hub enroll`.
fn effective_token(explicit: Option<String>, stored: Option<String>) -> Option<String> {
    let non_blank = |t: &String| !t.trim().is_empty();
    explicit
        .filter(non_blank)
        .or_else(|| stored.filter(non_blank))
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

    // A blank token is treated as no token at all. This matters because the usual way
    // to pass one is a shell expansion — the hub's image runs
    // `prism mcp --bind 0.0.0.0 --auth-token "$PRISM_MCP_TOKEN"` — and an unset or
    // empty variable yields `--auth-token ""`, i.e. `Some("")`. Testing only
    // `is_none()` would let that through and bind the LAN behind a bearer token that
    // every request trivially matches, which is worse than no auth because the log
    // then claims auth is required.
    // Preference order: an explicit --auth-token, then the dedicated MCP token minted
    // at enrolment, and only then the agent token.
    //
    // The dedicated token exists so MCP access can be revoked on its own: these tools
    // read the developer's machine, and rotating that credential must not force a
    // re-enrolment or take telemetry ingestion down with it. The agent token stays as
    // a fallback for agents enrolled before the dedicated one existed.
    let creds = crate::hub::load_credentials();
    let token = effective_token(
        auth_token,
        creds
            .as_ref()
            .and_then(|c| c.mcp_token.clone())
            .or_else(|| creds.as_ref().map(|c| c.agent_token.clone())),
    );
    if off_loopback && token.is_none() {
        anyhow::bail!(
            "refusing to bind {bind} (not loopback) without a bearer token — pass \
             --auth-token <token>, or run `prism hub enroll` first. An unauthenticated \
             tool server (prism_read_file included) must not be reachable on the LAN."
        );
    }

    // Off-loopback callers get the memory and graph tools only.
    //
    // The hub drives the Memory and Graph pages and needs nothing else, while
    // `prism_read_file` reads any path this user can read. Exposing arbitrary file
    // read to satisfy a stats page is privilege nobody asked for, so the remote
    // surface is narrowed rather than trusted to a single bearer token.
    let remote_only = off_loopback;
    let ct = tokio_util::sync::CancellationToken::new();

    // Allow the address we are actually bound to as a Host.
    //
    // rmcp validates the Host header as DNS-rebinding protection and allows only
    // loopback by default, so a server bound to the docker bridge answered every
    // authenticated request with "Forbidden: Host header is not allowed". The auth
    // layer was fine; this sits behind it, and only a request over the wire shows it.
    //
    // Adding the bind address does not weaken the check: it still rejects a Host this
    // server was never asked to answer on, which is what the protection is for.
    let mut cfg = StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token());
    if off_loopback {
        let host = bind_ip.to_string();
        cfg = cfg.with_allowed_hosts(vec![host.clone(), format!("{host}:{port}")]);
    }

    let service = StreamableHttpService::new(
        move || Ok(PrismMcpServer::new().with_remote_restrictions(remote_only)),
        LocalSessionManager::default().into(),
        cfg,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A remote server must refuse the tools that reach the machine.
    ///
    /// The hub needs memory and graph to render two pages; it never needs arbitrary
    /// file read. Binding off-loopback to serve those pages should not also publish
    /// `prism_read_file` to whatever can reach the port.
    #[test]
    fn remote_servers_refuse_filesystem_tools() {
        let remote = PrismMcpServer::new().with_remote_restrictions(true);
        assert!(remote.refuse_if_remote("prism_read_file").is_some());
        assert!(remote.refuse_if_remote("prism_filter_cmd").is_some());
        // The tools the hub actually drives stay available.
        for allowed in [
            "prism_memory_search",
            "prism_memory_stats",
            "prism_graph_query",
            "prism_graph_god_nodes",
        ] {
            assert!(
                remote.refuse_if_remote(allowed).is_none(),
                "{allowed} must remain available to the hub"
            );
        }
    }

    /// Loopback keeps the full tool set — this is the local agent's own MCP server.
    #[test]
    fn local_servers_keep_every_tool() {
        let local = PrismMcpServer::new();
        assert!(local.refuse_if_remote("prism_read_file").is_none());
        assert!(local.refuse_if_remote("prism_filter_cmd").is_none());
    }

    use super::effective_token;

    /// The hub's image runs `prism mcp --bind 0.0.0.0 --auth-token "$PRISM_MCP_TOKEN"`,
    /// so an unset or empty variable arrives as `Some("")`, not `None`. The
    /// off-loopback guard tested `is_none()`, which that value satisfies — so it would
    /// have bound the LAN behind a bearer token every request matches, while logging
    /// "bearer auth required". That is worse than no auth, because it reads as
    /// protected.
    #[test]
    fn a_blank_token_is_not_a_token() {
        assert_eq!(effective_token(Some(String::new()), None), None);
        assert_eq!(effective_token(Some("   ".into()), None), None);
        assert_eq!(effective_token(None, Some(String::new())), None);
        assert_eq!(effective_token(None, None), None);
    }

    #[test]
    fn a_real_token_from_either_source_is_used() {
        assert_eq!(
            effective_token(Some("flag".into()), None).as_deref(),
            Some("flag")
        );
        assert_eq!(
            effective_token(None, Some("enrolled".into())).as_deref(),
            Some("enrolled")
        );
        // The explicit flag wins when both are real.
        assert_eq!(
            effective_token(Some("flag".into()), Some("enrolled".into())).as_deref(),
            Some("flag")
        );
    }

    /// Each source is filtered independently, so an empty flag does not shadow a real
    /// enrolled credential: `--auth-token ""` behaves like omitting the flag, which is
    /// exactly the container case.
    #[test]
    fn an_empty_flag_falls_through_to_the_enrolled_token() {
        assert_eq!(
            effective_token(Some(String::new()), Some("enrolled".into())).as_deref(),
            Some("enrolled")
        );
    }
}
