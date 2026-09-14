/// Graphify NetworkX graph.json format representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphifyJson {
    #[serde(default)]
    pub directed: bool,
    #[serde(default)]
    pub multigraph: bool,
    #[serde(default)]
    pub nodes: Vec<GraphifyNode>,
    #[serde(default)]
    pub links: Vec<GraphifyLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphifyNode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub source_location: Option<String>,
    #[serde(default)]
    pub community: Option<usize>,
    #[serde(default)]
    pub norm_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphifyLink {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub weight: Option<f32>,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub source_location: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeExplanation<'a> {
    pub node: GraphNode,
    pub outgoing: Vec<(&'a GraphNode, EdgeKind, f32)>,
    pub incoming: Vec<(&'a GraphNode, EdgeKind, f32)>,
}

/// Owned, `Serialize`-able copy of [`NodeExplanation`]. `NodeExplanation` borrows from
/// the `GraphRAG` it was produced by, which cannot cross an `async fn`/function boundary
/// — this is what lets `knowledge::explain_node_data` hand the same computation to both
/// the CLI printer and the MCP tool without either reimplementing `explain_node`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeExplanationOwned {
    pub node: GraphNode,
    pub outgoing: Vec<(GraphNode, EdgeKind, f32)>,
    pub incoming: Vec<(GraphNode, EdgeKind, f32)>,
}

impl From<NodeExplanation<'_>> for NodeExplanationOwned {
    fn from(e: NodeExplanation<'_>) -> Self {
        NodeExplanationOwned {
            node: e.node,
            outgoing: e
                .outgoing
                .into_iter()
                .map(|(n, k, w)| (n.clone(), k, w))
                .collect(),
            incoming: e
                .incoming
                .into_iter()
                .map(|(n, k, w)| (n.clone(), k, w))
                .collect(),
        }
    }
}

// PRISM knowledge/graph_rag.rs — GraphRAG pipeline for cross-file codebase understanding
// Builds a dependency graph from source files and runs retrieval-augmented generation queries

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use walkdir::WalkDir;

use petgraph::graph::DiGraph;

/// A graph node representing a source file or code entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub path: String,
    pub kind: GraphNodeKind,
    pub token_count: usize,
    pub community: Option<usize>,
    pub embeddings: Vec<f32>,
}

/// Node type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphNodeKind {
    File,
    Function,
    Class,
    Import,
    Symbol,
}

impl std::fmt::Display for GraphNodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                GraphNodeKind::File => "file",
                GraphNodeKind::Function => "func",
                GraphNodeKind::Class => "class",
                GraphNodeKind::Import => "import",
                GraphNodeKind::Symbol => "symbol",
            }
        )
    }
}

/// A graph edge representing a dependency or import relationship
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: usize,
    pub target: usize,
    pub kind: EdgeKind,
    pub weight: f32,
}

/// Edge relationship type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Imports,
    Calls,
    References,
    Inherits,
    Implements,
    Depends,
    Contains,
}

impl std::fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                EdgeKind::Imports => "imports",
                EdgeKind::Calls => "calls",
                EdgeKind::References => "references",
                EdgeKind::Inherits => "inherits",
                EdgeKind::Implements => "implements",
                EdgeKind::Depends => "depends",
                EdgeKind::Contains => "contains",
            }
        )
    }
}

/// The main GraphRAG structure
#[derive(Debug, Serialize, Deserialize)]
pub struct GraphRAG {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    #[serde(skip, default = "default_digraph")]
    pub graph: DiGraph<GraphNode, GraphEdge>,
}

fn default_digraph() -> DiGraph<GraphNode, GraphEdge> {
    DiGraph::new()
}

impl GraphRAG {
    /// Build a GraphRAG from a list of nodes and edges
    pub fn build<P: AsRef<Path>>(_root: P, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Self {
        let mut rag = GraphRAG {
            nodes,
            edges,
            graph: default_digraph(),
        };

        rag.rebuild_graph();

        rag
    }

    /// Rebuild petgraph internal DiGraph from nodes and edges
    pub fn rebuild_graph(&mut self) {
        self.graph.clear();
        let mut node_map = std::collections::HashMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            let idx = self.graph.add_node(node.clone());
            node_map.insert(i, idx);
        }
        for edge in &self.edges {
            if let (Some(&si), Some(&ti)) = (node_map.get(&edge.source), node_map.get(&edge.target))
            {
                if si != ti {
                    self.graph.add_edge(si, ti, edge.clone());
                }
            }
        }
        self.detect_communities();
    }

    /// Build a real dependency GraphRAG by scanning files in a directory
    pub fn build_from_dir<P: AsRef<Path>>(root: P) -> Self {
        let root_path = root.as_ref();
        let mut nodes: Vec<GraphNode> = Vec::new();
        let mut edges: Vec<GraphEdge> = Vec::new();
        let mut file_indices: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        let walker = WalkDir::new(root_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                !name.starts_with('.')
                    && name != "target"
                    && name != "node_modules"
                    && name != "dist"
                    && name != "vendor"
            });

        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(
                ext,
                "rs" | "py" | "ts" | "js" | "tsx" | "jsx" | "go" | "c" | "cpp" | "h"
            ) {
                continue;
            }

            let rel_path = path
                .strip_prefix(root_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            let file_node_idx = nodes.len();
            file_indices.insert(rel_path.clone(), file_node_idx);

            let content = std::fs::read_to_string(path).unwrap_or_default();
            let token_count = content.len() / 4;

            nodes.push(GraphNode {
                id: format!("file:{}", rel_path),
                label: rel_path.clone(),
                path: rel_path.clone(),
                kind: GraphNodeKind::File,
                token_count,
                community: None,
                embeddings: Vec::new(),
            });

            // Extract declarations and imports.
            //
            // `ScanState` carries block-comment and docstring state across lines, so
            // commented-out code no longer becomes graph nodes — a false symbol is
            // indistinguishable from a real one at query time.
            let mut scan = ScanState::default();
            for line in content.lines() {
                let trimmed = line.trim();
                if !scan.is_code(line) {
                    continue;
                }

                let decl = parse_declaration(trimmed);

                if let Some((DeclKind::Function, ref fn_name)) = decl {
                    let fn_name = fn_name.as_str();
                    {
                        let fn_idx = nodes.len();
                        nodes.push(GraphNode {
                            id: format!("fn:{}:{}", rel_path, fn_name),
                            label: fn_name.to_string(),
                            path: rel_path.clone(),
                            kind: GraphNodeKind::Function,
                            token_count: 50,
                            community: None,
                            embeddings: Vec::new(),
                        });
                        edges.push(GraphEdge {
                            source: file_node_idx,
                            target: fn_idx,
                            kind: EdgeKind::References,
                            weight: 1.0,
                        });
                    }
                }

                // Structs / Classes / traits / interfaces
                if let Some((DeclKind::Type, ref type_name)) = decl {
                    let type_name = type_name.as_str();
                    {
                        let type_idx = nodes.len();
                        nodes.push(GraphNode {
                            id: format!("type:{}:{}", rel_path, type_name),
                            label: type_name.to_string(),
                            path: rel_path.clone(),
                            kind: GraphNodeKind::Class,
                            token_count: 40,
                            community: None,
                            embeddings: Vec::new(),
                        });
                        edges.push(GraphEdge {
                            source: file_node_idx,
                            target: type_idx,
                            kind: EdgeKind::References,
                            weight: 1.0,
                        });
                    }
                }

                // Imports
                let is_import = trimmed.starts_with("use ")
                    || trimmed.starts_with("import ")
                    || trimmed.starts_with("from ");

                if is_import {
                    let target_module = trimmed
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .trim_end_matches(';');
                    if !target_module.is_empty() {
                        let import_idx = nodes.len();
                        nodes.push(GraphNode {
                            id: format!("import:{}:{}", rel_path, target_module),
                            label: target_module.to_string(),
                            path: rel_path.clone(),
                            kind: GraphNodeKind::Import,
                            token_count: 10,
                            community: None,
                            embeddings: Vec::new(),
                        });
                        edges.push(GraphEdge {
                            source: file_node_idx,
                            target: import_idx,
                            kind: EdgeKind::Imports,
                            weight: 0.8,
                        });
                    }
                }
            }
        }

        Self::build(root_path, nodes, edges)
    }

    /// Run a GraphRAG query across the codebase
    pub fn query(&self, query: &str, top_k: usize) -> Vec<GraphNode> {
        let query_tokens = Self::tokenize(query);
        let mut scored: Vec<(f64, &GraphNode)> = self
            .nodes
            .iter()
            .map(|node| {
                let node_tokens = Self::tokenize(&node.label);
                let path_tokens = Self::tokenize(&node.path);
                let overlap: usize = query_tokens
                    .iter()
                    .filter(|t| node_tokens.contains(t) || path_tokens.contains(t))
                    .count();
                let score = if overlap > 0 {
                    overlap as f64 + (1.0 / (node.label.len() as f64 + 1.0))
                } else {
                    0.0
                };
                (score, node)
            })
            .filter(|(s, _)| *s > 0.0)
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(top_k)
            .map(|(_, node)| node.clone())
            .collect()
    }

    /// Retrieve nodes that form a connected component around a seed node
    pub fn retrieve_connected(&self, seed_id: &str, depth: usize) -> Vec<String> {
        let mut frontier: Vec<String> = vec![seed_id.to_string()];
        let mut visited: HashSet<String> = HashSet::new();

        for _ in 0..depth {
            let next_frontier: Vec<String> = frontier
                .iter()
                .flat_map(|id| {
                    self.edges.iter().filter_map(|edge| {
                        let source_id = self.nodes.get(edge.source).map(|n| n.id.clone())?;
                        let target_id = self.nodes.get(edge.target).map(|n| n.id.clone())?;
                        if source_id == *id {
                            Some(target_id)
                        } else if target_id == *id {
                            Some(source_id)
                        } else {
                            None
                        }
                    })
                })
                .filter(|nid| !visited.contains(nid))
                .collect();
            for nid in next_frontier.iter() {
                visited.insert(nid.clone());
            }
            frontier = next_frontier;
        }

        let mut sorted_result = frontier;
        sorted_result.sort();
        sorted_result
    }

    /// Detect communities using simple connected-component analysis
    fn detect_communities(&mut self) {
        let nodes = self.graph.node_indices().collect::<Vec<_>>();
        let mut community_id = 0usize;
        let mut community_assignment: Vec<Option<usize>> = vec![None; self.nodes.len()];

        for idx in &nodes {
            let i = idx.index();
            if community_assignment.get(i).copied().flatten().is_some() {
                continue;
            }
            let mut queue = vec![*idx];
            while let Some(node_idx) = queue.pop() {
                let ni = node_idx.index();
                if community_assignment.get(ni).copied().flatten().is_some() {
                    continue;
                }
                community_assignment[ni] = Some(community_id);
                if let Some(n) = self.nodes.get_mut(ni) {
                    n.community = Some(community_id);
                }
                for neighbor in self.graph.neighbors(node_idx) {
                    let ni2 = neighbor.index();
                    if community_assignment.get(ni2).copied().flatten().is_none() {
                        queue.push(neighbor);
                    }
                }
            }
            community_id += 1;
        }
    }

    /// Export the graph as JSON
    pub fn export_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Convert a Graphify NetworkX JSON graph into PRISM GraphRAG
    pub fn from_graphify(g: &GraphifyJson) -> Self {
        let mut nodes = Vec::with_capacity(g.nodes.len());
        let mut id_map = std::collections::HashMap::with_capacity(g.nodes.len());

        for (idx, gn) in g.nodes.iter().enumerate() {
            id_map.insert(gn.id.clone(), idx);
            let path = gn.source_file.clone().unwrap_or_else(|| gn.label.clone());
            let kind = if gn.label.ends_with("()") || gn.label.starts_with('.') {
                GraphNodeKind::Function
            } else if gn.file_type.as_deref() == Some("code")
                && (path == gn.label || gn.label.contains('.'))
            {
                GraphNodeKind::File
            } else if gn
                .label
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
                && !gn.label.contains("()")
            {
                GraphNodeKind::Class
            } else {
                GraphNodeKind::Symbol
            };

            nodes.push(GraphNode {
                id: gn.id.clone(),
                label: gn.label.clone(),
                path,
                kind,
                token_count: 50,
                community: gn.community,
                embeddings: Vec::new(),
            });
        }

        let mut edges = Vec::with_capacity(g.links.len());
        for link in &g.links {
            if let (Some(&source_idx), Some(&target_idx)) =
                (id_map.get(&link.source), id_map.get(&link.target))
            {
                let kind = match link.relation.as_deref() {
                    Some("contains") => EdgeKind::Contains,
                    Some("calls") | Some("method") => EdgeKind::Calls,
                    Some("imports") => EdgeKind::Imports,
                    _ => EdgeKind::References,
                };
                edges.push(GraphEdge {
                    source: source_idx,
                    target: target_idx,
                    kind,
                    weight: link.weight.unwrap_or(1.0),
                });
            }
        }

        Self::build(".", nodes, edges)
    }

    /// Transparently load either a native PRISM GraphRAG file or a Graphify NetworkX graph.json file
    pub fn load_any<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let p = path.as_ref();
        let raw = std::fs::read_to_string(p)?;

        // Try PRISM native GraphRAG first
        if let Ok(mut rag) = serde_json::from_str::<GraphRAG>(&raw) {
            rag.rebuild_graph();
            return Ok(rag);
        }

        // Try Graphify format
        if let Ok(graphify) = serde_json::from_str::<GraphifyJson>(&raw) {
            return Ok(Self::from_graphify(&graphify));
        }

        anyhow::bail!("Unrecognized graph format in: {}", p.display())
    }

    /// Explain a node by finding its attributes, incoming connections, and outgoing connections
    pub fn explain_node(&self, name_or_id: &str) -> Option<NodeExplanation<'_>> {
        let name_lower = name_or_id.to_lowercase();
        let (node_idx, node) = self
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| {
                n.label.to_lowercase() == name_lower || n.id.to_lowercase() == name_lower
            })
            .or_else(|| {
                self.nodes.iter().enumerate().find(|(_, n)| {
                    n.label.to_lowercase().contains(&name_lower)
                        || n.path.to_lowercase().contains(&name_lower)
                })
            })?;

        let mut outgoing = Vec::new();
        let mut incoming = Vec::new();

        for edge in &self.edges {
            if edge.source == node_idx && edge.target < self.nodes.len() {
                outgoing.push((&self.nodes[edge.target], edge.kind, edge.weight));
            } else if edge.target == node_idx && edge.source < self.nodes.len() {
                incoming.push((&self.nodes[edge.source], edge.kind, edge.weight));
            }
        }

        Some(NodeExplanation {
            node: node.clone(),
            outgoing,
            incoming,
        })
    }

    /// Find shortest dependency path between two symbols/nodes using BFS
    pub fn shortest_path(
        &self,
        from: &str,
        to: &str,
    ) -> Option<Vec<(GraphNode, String, GraphNode)>> {
        let from_lower = from.to_lowercase();
        let to_lower = to.to_lowercase();

        let (from_idx, _) = self.nodes.iter().enumerate().find(|(_, n)| {
            n.label.to_lowercase() == from_lower
                || n.id.to_lowercase() == from_lower
                || n.label.to_lowercase().contains(&from_lower)
        })?;

        let (to_idx, _) = self.nodes.iter().enumerate().find(|(_, n)| {
            n.label.to_lowercase() == to_lower
                || n.id.to_lowercase() == to_lower
                || n.label.to_lowercase().contains(&to_lower)
        })?;

        if from_idx == to_idx {
            return Some(Vec::new());
        }

        let mut adj: Vec<Vec<(usize, EdgeKind)>> = vec![Vec::new(); self.nodes.len()];
        for edge in &self.edges {
            if edge.source < self.nodes.len() && edge.target < self.nodes.len() {
                adj[edge.source].push((edge.target, edge.kind));
            }
        }

        let mut queue = std::collections::VecDeque::new();
        let mut visited = std::collections::HashSet::new();
        let mut parent = std::collections::HashMap::new();

        queue.push_back(from_idx);
        visited.insert(from_idx);

        let mut found = false;
        while let Some(curr) = queue.pop_front() {
            if curr == to_idx {
                found = true;
                break;
            }
            for &(next, kind) in &adj[curr] {
                if !visited.contains(&next) {
                    visited.insert(next);
                    parent.insert(next, (curr, kind));
                    queue.push_back(next);
                }
            }
        }

        if !found {
            return None;
        }

        let mut path_steps = Vec::new();
        let mut curr = to_idx;
        while let Some(&(prev, kind)) = parent.get(&curr) {
            path_steps.push((
                self.nodes[prev].clone(),
                format!("{}", kind),
                self.nodes[curr].clone(),
            ));
            curr = prev;
            if curr == from_idx {
                break;
            }
        }
        path_steps.reverse();
        Some(path_steps)
    }

    /// List most connected architectural hub nodes in the graph (degree centrality)
    pub fn god_nodes(&self, top_k: usize) -> Vec<(&GraphNode, usize)> {
        let mut degrees = vec![0usize; self.nodes.len()];
        for edge in &self.edges {
            if edge.source < self.nodes.len() {
                degrees[edge.source] += 1;
            }
            if edge.target < self.nodes.len() {
                degrees[edge.target] += 1;
            }
        }

        let mut scored: Vec<(&GraphNode, usize)> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n, degrees[i]))
            .collect();

        scored.sort_by_key(|a| std::cmp::Reverse(a.1));
        scored.truncate(top_k);
        scored
    }

    fn tokenize(text: &str) -> Vec<String> {
        let lower = text.to_lowercase();
        lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }
}

// ─── declaration extraction ──────────────────────────────────────────────────

/// What a source line declares, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclKind {
    Function,
    Type,
}

/// Tracks whether the scanner is inside a block comment or a docstring.
///
/// Extraction used to run over raw lines, so `// fn foo()` in a comment and
/// `"def parse("` in a string literal both became graph nodes. A symbol graph whose
/// nodes include commented-out code is worse than a smaller accurate one: the false
/// entries are indistinguishable from real ones at query time.
#[derive(Default)]
pub(crate) struct ScanState {
    in_block_comment: bool,
    in_docstring: bool,
}

impl ScanState {
    /// Advance past `line` and report whether its code content should be inspected.
    ///
    /// Deliberately line-granular rather than a real lexer: the aim is to stop
    /// obviously-inert text becoming nodes, not to tokenize four languages correctly.
    pub(crate) fn is_code(&mut self, line: &str) -> bool {
        let t = line.trim();

        if self.in_block_comment {
            if t.contains("*/") {
                self.in_block_comment = false;
            }
            return false;
        }
        if self.in_docstring {
            if t.contains("\"\"\"") || t.contains("'''") {
                self.in_docstring = false;
            }
            return false;
        }

        // A docstring or block comment that opens and closes on one line is inert but
        // does not change state.
        let opens_block = t.contains("/*") && !t.contains("*/");
        if opens_block {
            self.in_block_comment = true;
            return false;
        }
        let triple = t.matches("\"\"\"").count() + t.matches("'''").count();
        if triple == 1 {
            self.in_docstring = true;
            return false;
        }

        if t.is_empty()
            || t.starts_with("//")
            || t.starts_with('#')
            || t.starts_with("/*")
            || t.starts_with('*')
        {
            return false;
        }
        true
    }
}

/// Strip Rust visibility and modifier keywords from the front of a declaration.
///
/// `pub(crate) fn`, `pub(super) async fn`, `pub const unsafe fn` and
/// `extern "C" fn` were all invisible to the old `starts_with("pub fn ")` test —
/// `pub(crate) fn` alone accounts for a large share of a real Rust codebase.
fn strip_modifiers(mut t: &str) -> &str {
    loop {
        let before = t;
        for kw in [
            "export default ",
            "export ",
            "public ",
            "private ",
            "protected ",
            "static ",
            "async ",
            "const ",
            "unsafe ",
            "default ",
            "abstract ",
            "final ",
            "override ",
        ] {
            if let Some(rest) = t.strip_prefix(kw) {
                t = rest.trim_start();
            }
        }
        // `pub`, `pub(crate)`, `pub(super)`, `pub(in path::to)`
        if let Some(rest) = t.strip_prefix("pub") {
            let rest = rest.trim_start();
            if let Some(open) = rest.strip_prefix('(') {
                if let Some(close) = open.find(')') {
                    t = open[close + 1..].trim_start();
                }
            } else if rest.starts_with("fn ")
                || rest.starts_with("struct ")
                || rest.starts_with("enum ")
                || rest.starts_with("trait ")
                || rest.starts_with("type ")
                || rest.starts_with("union ")
                || rest.starts_with("const ")
                || rest.starts_with("async ")
                || rest.starts_with("unsafe ")
                || rest.starts_with("extern ")
            {
                t = rest;
            }
        }
        if let Some(rest) = t.strip_prefix("extern ") {
            // `extern "C" fn`
            let rest = rest.trim_start();
            t = match rest
                .strip_prefix('"')
                .and_then(|r| r.find('"').map(|i| &r[i + 1..]))
            {
                Some(after) => after.trim_start(),
                None => rest,
            };
        }
        if t == before {
            return t;
        }
    }
}

/// A valid identifier, stopping at the first character that cannot be part of one.
fn ident(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len());
    &s[..end]
}

/// Parse one line of code into the declaration it introduces, if any.
///
/// Covers what the previous prefix matching missed and what a real codebase is mostly
/// made of: Rust visibility forms and `async`/`const`/`unsafe`/`extern` modifiers,
/// Python `async def`, Go methods with a receiver, TypeScript `export`/`export default`,
/// and `const name = (…) =>` arrow functions.
pub(crate) fn parse_declaration(line: &str) -> Option<(DeclKind, String)> {
    let raw = line.trim();
    let t = strip_modifiers(raw);

    // Rust / Python / Go / JS function forms.
    for kw in ["fn ", "def ", "func ", "function "] {
        if let Some(rest) = t.strip_prefix(kw) {
            let rest = rest.trim_start();
            // Go method: `func (r *Repo) Name(` — the receiver precedes the name.
            let rest = if rest.starts_with('(') {
                match rest.find(')') {
                    Some(i) => rest[i + 1..].trim_start(),
                    None => rest,
                }
            } else {
                rest
            };
            let name = ident(rest);
            if !name.is_empty() {
                return Some((DeclKind::Function, name.to_string()));
            }
        }
    }

    // Type forms.
    for kw in [
        "struct ",
        "enum ",
        "trait ",
        "class ",
        "interface ",
        "union ",
        "type ",
    ] {
        if let Some(rest) = t.strip_prefix(kw) {
            let name = ident(rest.trim_start());
            if !name.is_empty() {
                return Some((DeclKind::Type, name.to_string()));
            }
        }
    }

    // `const name = (args) => …` / `let name = async (…) => …`.
    //
    // Checked against the *original* line rather than the modifier-stripped one:
    // `strip_modifiers` removes a leading `const` so that Rust's `pub const fn` parses,
    // which would otherwise make every arrow binding invisible here. Matched last so a
    // Rust `const fn` is still a function rather than a binding.
    let binding = raw
        .strip_prefix("export default ")
        .or_else(|| raw.strip_prefix("export "))
        .unwrap_or(raw);
    for kw in ["const ", "let ", "var "] {
        if let Some(rest) = binding.strip_prefix(kw) {
            let name = ident(rest.trim_start());
            if name.is_empty() {
                continue;
            }
            let after = &rest.trim_start()[name.len()..];
            if after.trim_start().starts_with('=') && t.contains("=>") {
                return Some((DeclKind::Function, name.to_string()));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_rag_query_and_connected() {
        let nodes = vec![
            GraphNode {
                id: "file:main.rs".to_string(),
                label: "main.rs".to_string(),
                path: "src/main.rs".to_string(),
                kind: GraphNodeKind::File,
                token_count: 100,
                community: None,
                embeddings: Vec::new(),
            },
            GraphNode {
                id: "fn:main.rs:run_proxy".to_string(),
                label: "run_proxy".to_string(),
                path: "src/main.rs".to_string(),
                kind: GraphNodeKind::Function,
                token_count: 50,
                community: None,
                embeddings: Vec::new(),
            },
        ];

        let edges = vec![GraphEdge {
            source: 0,
            target: 1,
            kind: EdgeKind::References,
            weight: 1.0,
        }];

        let rag = GraphRAG::build(".", nodes, edges);
        let results = rag.query("run_proxy", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].label, "run_proxy");

        let connected = rag.retrieve_connected("file:main.rs", 1);
        assert!(connected.contains(&"fn:main.rs:run_proxy".to_string()));
    }

    #[test]
    fn test_graphify_json_loading_and_path() {
        let json_str = r#"{
            "directed": true,
            "multigraph": false,
            "nodes": [
                {"id": "mod_a", "label": "module_a.rs", "file_type": "code"},
                {"id": "fn_b", "label": "do_work()", "file_type": "code"},
                {"id": "fn_c", "label": "finalize()", "file_type": "code"}
            ],
            "links": [
                {"source": "mod_a", "target": "fn_b", "relation": "contains"},
                {"source": "fn_b", "target": "fn_c", "relation": "calls"}
            ]
        }"#;

        let graphify: GraphifyJson = serde_json::from_str(json_str).unwrap();
        let rag = GraphRAG::from_graphify(&graphify);
        assert_eq!(rag.nodes.len(), 3);
        assert_eq!(rag.edges.len(), 2);

        let exp = rag.explain_node("module_a.rs").unwrap();
        assert_eq!(exp.outgoing.len(), 1);
        assert_eq!(exp.outgoing[0].0.label, "do_work()");

        let path = rag.shortest_path("module_a.rs", "finalize()").unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].0.label, "module_a.rs");
        assert_eq!(path[1].2.label, "finalize()");
    }
}

#[cfg(test)]
mod decl_tests {
    use super::*;

    fn func(line: &str) -> Option<String> {
        match parse_declaration(line) {
            Some((DeclKind::Function, n)) => Some(n),
            _ => None,
        }
    }
    fn typ(line: &str) -> Option<String> {
        match parse_declaration(line) {
            Some((DeclKind::Type, n)) => Some(n),
            _ => None,
        }
    }

    /// Every one of these was missed by the old `starts_with("pub fn ")` matching.
    #[test]
    fn rust_visibility_and_modifier_forms_are_found() {
        for (line, want) in [
            ("pub fn simple()", "simple"),
            ("fn bare()", "bare"),
            ("pub(crate) fn scoped()", "scoped"),
            ("pub(super) fn parent()", "parent"),
            ("pub(in crate::a) fn deep()", "deep"),
            ("pub async fn fetch()", "fetch"),
            ("async fn inner()", "inner"),
            ("pub const fn constant()", "constant"),
            ("pub unsafe fn danger()", "danger"),
            ("unsafe extern \"C\" fn ffi()", "ffi"),
            ("    fn indented_method(&self)", "indented_method"),
        ] {
            assert_eq!(func(line).as_deref(), Some(want), "line: {line}");
        }
    }

    #[test]
    fn python_go_and_typescript_forms_are_found() {
        for (line, want) in [
            ("def handler(request):", "handler"),
            ("async def ahandler(request):", "ahandler"),
            ("func Plain() error {", "Plain"),
            ("func (r *Repo) Method() error {", "Method"),
            ("function classic() {", "classic"),
            ("export function exported() {", "exported"),
            ("export default function def() {", "def"),
            ("export async function fetchAll() {", "fetchAll"),
            ("const arrow = (a, b) => a + b", "arrow"),
            (
                "export const exportedArrow = async () => {}",
                "exportedArrow",
            ),
        ] {
            assert_eq!(func(line).as_deref(), Some(want), "line: {line}");
        }
    }

    #[test]
    fn type_declarations_are_found_across_languages() {
        for (line, want) in [
            ("pub struct Config {", "Config"),
            ("struct Inner<T> {", "Inner"),
            ("pub(crate) enum Kind {", "Kind"),
            ("pub trait Filter {", "Filter"),
            ("class Service {", "Service"),
            ("export interface Props {", "Props"),
            ("type Alias = u32;", "Alias"),
        ] {
            assert_eq!(typ(line).as_deref(), Some(want), "line: {line}");
        }
    }

    #[test]
    fn a_rust_const_fn_is_a_function_not_a_binding() {
        assert_eq!(
            func("pub const fn width() -> usize { 16 }").as_deref(),
            Some("width")
        );
    }

    #[test]
    fn a_plain_binding_is_not_a_declaration() {
        assert_eq!(parse_declaration("const MAX = 10;"), None);
        assert_eq!(parse_declaration("let total = compute();"), None);
        assert_eq!(parse_declaration("x.function_call();"), None);
    }

    /// The other half of the old bug: commented-out and quoted code became nodes.
    #[test]
    fn comments_and_docstrings_are_not_code() {
        let mut scan = ScanState::default();
        let src = [
            "// fn commented_out() {}",
            "# def python_comment():",
            "/* block start",
            "fn inside_block_comment() {}",
            "*/",
            "fn real_one() {}",
        ];
        let found: Vec<String> = src
            .iter()
            .filter(|l| scan.is_code(l))
            .filter_map(|l| func(l))
            .collect();

        assert_eq!(found, vec!["real_one".to_string()]);
    }

    #[test]
    fn a_python_docstring_hides_its_contents() {
        let mut scan = ScanState::default();
        let src = [
            "\"\"\"Module docs.",
            "def not_a_real_function():",
            "\"\"\"",
            "def actual():",
        ];
        let found: Vec<String> = src
            .iter()
            .filter(|l| scan.is_code(l))
            .filter_map(|l| func(l))
            .collect();

        assert_eq!(found, vec!["actual".to_string()]);
    }
}
