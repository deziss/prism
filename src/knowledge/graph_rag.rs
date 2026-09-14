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

            // Declarations come from a real parse where a grammar exists, and from the
            // line matcher otherwise. The parse is strictly better — it sees signatures
            // that span lines, and it knows a `fn` inside a string literal is text —
            // but it only covers the six languages with a grammar compiled in, so the
            // matcher still carries C, Ruby, Java, PHP, shell and the rest.
            let parsed: Option<Vec<(DeclKind, String)>> = super::ast::Lang::from_extension(ext)
                .and_then(|l| super::ast::extract(&content, l));
            let parsed_ok = parsed.is_some();

            if let Some(decls) = parsed {
                for (kind, name) in decls {
                    let idx = nodes.len();
                    nodes.push(GraphNode {
                        id: match kind {
                            DeclKind::Function => format!("fn:{}:{}", rel_path, name),
                            DeclKind::Type => format!("type:{}:{}", rel_path, name),
                        },
                        label: name,
                        path: rel_path.clone(),
                        kind: match kind {
                            DeclKind::Function => GraphNodeKind::Function,
                            DeclKind::Type => GraphNodeKind::Class,
                        },
                        token_count: match kind {
                            DeclKind::Function => 50,
                            DeclKind::Type => 40,
                        },
                        community: None,
                        embeddings: Vec::new(),
                    });
                    edges.push(GraphEdge {
                        source: file_node_idx,
                        target: idx,
                        kind: EdgeKind::References,
                        weight: 1.0,
                    });
                }
            }

            // Imports are still read line-wise even when the file was parsed: the
            // grammars model imports very differently from one another, and the graph
            // only needs the module string.
            //
            // `ScanState` carries block-comment and docstring state across lines, so
            // commented-out code does not become a node — a false symbol is
            // indistinguishable from a real one at query time.
            let mut scan = ScanState::default();
            for line in content.lines() {
                let trimmed = line.trim();
                if !scan.is_code(line) {
                    continue;
                }

                // Skip the matcher's declarations entirely when the parse already
                // supplied them, or every symbol in a parsed file would be added twice.
                let decl = if parsed_ok {
                    None
                } else {
                    parse_declaration(trimmed)
                };

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

        Self::link_imports_to_files(&nodes, &mut edges, &file_indices);

        Self::build(root_path, nodes, edges)
    }

    /// Turn import nodes into file→file edges wherever the imported module resolves to
    /// a file that is also in the index.
    ///
    /// Without this the graph has **no cross-file structure at all**: an import became a
    /// leaf node hanging off the file that declared it, so every file was its own
    /// connected component. Indexing prism's own `src/` gave 42 components for 42 files,
    /// which is why community detection could only ever recover the `path` field. A
    /// "cross-file dependency graph" whose files do not reference each other is a set of
    /// per-file symbol lists.
    ///
    /// Resolution is by suffix match on the module path, which is what can be done
    /// without a build system: `crate::filter::common` matches `src/filter/common.rs`,
    /// `./util` matches `src/util.ts`, and `std::collections` matches nothing, correctly
    /// — an import of something outside the tree has no file node to point at.
    fn link_imports_to_files(
        nodes: &[GraphNode],
        edges: &mut Vec<GraphEdge>,
        file_indices: &std::collections::HashMap<String, usize>,
    ) {
        // Index files by their path stem segments so a suffix match is a lookup rather
        // than a scan over every file for every import.
        let mut by_suffix: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (path, idx) in file_indices {
            let stem = path
                .rsplit_once('.')
                .map(|(head, _)| head)
                .unwrap_or(path)
                .replace('\\', "/");
            let segments: Vec<&str> = stem.split('/').filter(|s| !s.is_empty()).collect();
            // Register every suffix: "filter/common", "common" — longest wins on insert
            // order, and a collision simply keeps the first, which is the shallower file.
            for start in 0..segments.len() {
                by_suffix.entry(segments[start..].join("/")).or_insert(*idx);
            }
            // `mod.rs` is addressed by its directory name, not its file name.
            if segments.last() == Some(&"mod") && segments.len() >= 2 {
                by_suffix
                    .entry(segments[..segments.len() - 1].join("/"))
                    .or_insert(*idx);
            }
        }

        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        // Only import nodes, and only those whose declaring file we can identify.
        for (i, node) in nodes.iter().enumerate() {
            if node.kind != GraphNodeKind::Import {
                continue;
            }
            let Some(&from) = file_indices.get(&node.path) else {
                continue;
            };
            let Some(to) = resolve_module(&node.label, &by_suffix) else {
                continue;
            };
            if to == from || !seen.insert((from, to)) {
                continue;
            }
            edges.push(GraphEdge {
                source: from,
                target: to,
                kind: EdgeKind::Imports,
                weight: 1.0,
            });
            let _ = i;
        }
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

    /// Group nodes into communities by label propagation over the undirected graph.
    ///
    /// This used to be connected components that followed **outgoing** edges only
    /// (`neighbors`, not `neighbors_undirected`). On a graph whose edges run
    /// file → symbol, reachability from a file is exactly that file's own symbols, so
    /// every "community" was a single file and the result carried no information the
    /// `path` field did not already have. Indexing prism's own `src/` produced 42
    /// communities for 42 files.
    ///
    /// Label propagation instead: each node repeatedly adopts the label most common
    /// among its neighbours, which lets a group form across files when imports connect
    /// them — the thing a community is supposed to tell you.
    ///
    /// Deterministic, which matters because the graph is persisted and diffed: nodes
    /// are visited in index order rather than the shuffled order the classic algorithm
    /// uses, and ties break towards the lowest label. Bounded iterations because label
    /// propagation is not guaranteed to converge — oscillating pairs are a known
    /// failure mode, and a fixed ceiling is cheaper than detecting it.
    fn detect_communities(&mut self) {
        const MAX_ROUNDS: usize = 20;

        let n = self.nodes.len();
        if n == 0 {
            return;
        }

        // Seed every node with its own label.
        let mut labels: Vec<usize> = (0..n).collect();

        // Propagate over **files only**, across import edges.
        //
        // Running it over every node does not work on this shape of graph: a file has
        // dozens of symbol children and one or two imports, so the children outvote the
        // imports every round and each file keeps its own label. Measured on prism's
        // own `src/` — 228 file→file import edges and still only one community spanning
        // more than one file.
        //
        // A community here means a group of modules that depend on each other, so files
        // are the right unit; symbols inherit their file's label below. Adjacency comes
        // from petgraph rather than a second walk over `self.edges`, so centrality,
        // traversal and communities all read the same topology.
        let is_file: Vec<bool> = self
            .nodes
            .iter()
            .map(|x| x.kind == GraphNodeKind::File)
            .collect();
        let adjacency: Vec<Vec<usize>> = self
            .graph
            .node_indices()
            .map(|idx| {
                let i = idx.index();
                if !is_file.get(i).copied().unwrap_or(false) {
                    return Vec::new();
                }
                let mut ns: Vec<usize> = self
                    .graph
                    .neighbors_undirected(idx)
                    .map(|x| x.index())
                    .filter(|x| *x < n && is_file.get(*x).copied().unwrap_or(false))
                    .collect();
                ns.sort_unstable();
                ns.dedup();
                ns
            })
            .collect();

        for _ in 0..MAX_ROUNDS {
            let mut changed = false;
            for i in 0..n.min(adjacency.len()) {
                let neighbours = &adjacency[i];
                if neighbours.is_empty() {
                    continue;
                }
                let mut counts: std::collections::BTreeMap<usize, usize> =
                    std::collections::BTreeMap::new();
                for &j in neighbours {
                    *counts.entry(labels[j]).or_insert(0) += 1;
                }
                // BTreeMap iterates in ascending key order, so `max_by_key` on the count
                // alone would take the *last* maximum; compare on (count, Reverse(label))
                // to land on the lowest label instead. Determinism is the point.
                let best = counts
                    .iter()
                    .max_by_key(|(label, count)| (**count, std::cmp::Reverse(**label)))
                    .map(|(label, _)| *label);
                if let Some(best) = best {
                    if best != labels[i] {
                        labels[i] = best;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // Symbols take their file's label: a function belongs to whatever module group
        // its file belongs to, and giving it a label of its own would report as many
        // communities as there are symbols.
        let file_label_by_path: std::collections::HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, node)| node.kind == GraphNodeKind::File && *i < labels.len())
            .map(|(i, node)| (node.path.as_str(), labels[i]))
            .collect();
        let inherited: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                if node.kind == GraphNodeKind::File {
                    labels.get(i).copied().unwrap_or(i)
                } else {
                    file_label_by_path
                        .get(node.path.as_str())
                        .copied()
                        .unwrap_or_else(|| labels.get(i).copied().unwrap_or(i))
                }
            })
            .collect();

        // Renumber to a dense 0..k so the ids mean "community 0, 1, 2", not "whichever
        // node index happened to win".
        let mut dense: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
        for (i, node) in self.nodes.iter_mut().enumerate() {
            let raw = inherited.get(i).copied().unwrap_or(i);
            let next = dense.len();
            let id = *dense.entry(raw).or_insert(next);
            node.community = Some(id);
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
        // Degree comes from the petgraph graph rather than a second pass over
        // `self.edges`. The two were parallel representations of the same topology
        // kept in step by hand, and `rebuild_graph` is what makes the graph the
        // authoritative one — counting from the edge vector meant centrality could
        // silently disagree with traversal.
        let mut degrees = vec![0usize; self.nodes.len()];
        for idx in self.graph.node_indices() {
            let i = idx.index();
            if i < degrees.len() {
                degrees[i] = self.graph.neighbors_undirected(idx).count();
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

/// Map an import's module string to a file node, by suffix.
///
/// Handles the forms that actually appear: Rust `crate::a::b` / `super::b` / `self::b`,
/// JS/TS `./a/b` and `../a/b`, Python `a.b.c`, Go `"pkg/a/b"`. Anything that resolves
/// outside the indexed tree — `std::collections`, `react` — yields `None`, which is the
/// right answer rather than a missing edge to paper over.
fn resolve_module(
    module: &str,
    by_suffix: &std::collections::HashMap<String, usize>,
) -> Option<usize> {
    let cleaned = module
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == ';' || c == ',');
    // Rust brings in several names at once: `use crate::a::{b, c}` — the module is the
    // part before the brace.
    let cleaned = cleaned
        .split('{')
        .next()
        .unwrap_or(cleaned)
        .trim_end_matches(':');

    let normalised = cleaned.replace("::", "/").replace('.', "/");
    let segments: Vec<&str> = normalised
        .split('/')
        .map(str::trim)
        // `*` is a glob import (`use crate::filter::common::*`), not a path segment —
        // leaving it in made every glob import unresolvable, which was 227 of the 228
        // imports in this repo.
        .filter(|s| {
            !s.is_empty()
                && *s != "crate"
                && *s != "self"
                && *s != "super"
                && *s != "@"
                && *s != "*"
        })
        .collect();
    if segments.is_empty() {
        return None;
    }

    // Try every contiguous run, longest first.
    //
    // Both ends have to move. Trailing segments are usually *items* rather than
    // modules — `use crate::vector::TurboVecIndex` names a type, and only
    // `crate::vector` is a file — while leading segments are crate or package
    // qualifiers that do not appear in the path. Matching only whole suffixes resolved
    // one import in this repo; allowing the tail to be dropped resolves the rest.
    for start in 0..segments.len() {
        for end in (start + 1..=segments.len()).rev() {
            if let Some(&idx) = by_suffix.get(&segments[start..end].join("/")) {
                return Some(idx);
            }
        }
    }
    None
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

#[cfg(test)]
mod community_tests {
    use super::*;

    /// Communities are propagated over **files** joined by imports, so the fixtures
    /// model files: one path each, so no two share a label by inheritance.
    fn node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: id.to_string(),
            path: format!("{id}.rs"),
            kind: GraphNodeKind::File,
            token_count: 1,
            community: None,
            embeddings: Vec::new(),
        }
    }

    fn symbol(id: &str, path: &str) -> GraphNode {
        GraphNode {
            path: path.to_string(),
            kind: GraphNodeKind::Function,
            ..node(id)
        }
    }

    fn edge(a: usize, b: usize) -> GraphEdge {
        GraphEdge {
            source: a,
            target: b,
            kind: EdgeKind::References,
            weight: 1.0,
        }
    }

    /// `rebuild_graph` calls `detect_communities` itself, so building is enough.
    fn graph_of(n: usize, edges: Vec<GraphEdge>) -> GraphRAG {
        GraphRAG::build(".", (0..n).map(|i| node(&format!("n{i}"))).collect(), edges)
    }

    /// Two clusters joined by nothing must not share a community.
    #[test]
    fn disconnected_clusters_get_different_communities() {
        // 0-1-2   and   3-4-5
        let g = graph_of(6, vec![edge(0, 1), edge(1, 2), edge(3, 4), edge(4, 5)]);

        let a = g.nodes[0].community.unwrap();
        assert_eq!(g.nodes[1].community.unwrap(), a);
        assert_eq!(g.nodes[2].community.unwrap(), a);

        let b = g.nodes[3].community.unwrap();
        assert_eq!(g.nodes[4].community.unwrap(), b);
        assert_ne!(a, b, "unconnected clusters must not merge");
    }

    /// The bug this replaced: edges ran one way, so following only outgoing edges made
    /// every file its own community. An undirected walk groups them.
    #[test]
    fn a_community_can_span_nodes_reachable_only_backwards() {
        // 1 -> 0 <- 2 : nothing is reachable *from* 0, but all three are one group.
        let g = graph_of(3, vec![edge(1, 0), edge(2, 0)]);

        let c = g.nodes[0].community.unwrap();
        assert_eq!(g.nodes[1].community.unwrap(), c);
        assert_eq!(g.nodes[2].community.unwrap(), c);
    }

    #[test]
    fn community_ids_are_dense_from_zero() {
        let g = graph_of(4, vec![edge(0, 1), edge(2, 3)]);

        let mut ids: Vec<usize> = g.nodes.iter().filter_map(|n| n.community).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, vec![0, 1], "ids should be 0..k, got {ids:?}");
    }

    /// The graph is persisted and diffed, so the same input must give the same ids.
    #[test]
    fn assignment_is_deterministic() {
        let edges = || vec![edge(0, 1), edge(1, 2), edge(3, 4)];
        let first: Vec<_> = graph_of(5, edges())
            .nodes
            .iter()
            .map(|n| n.community)
            .collect();
        let second: Vec<_> = graph_of(5, edges())
            .nodes
            .iter()
            .map(|n| n.community)
            .collect();

        assert_eq!(first, second);
    }

    #[test]
    fn an_isolated_node_still_gets_a_community() {
        let g = graph_of(3, vec![edge(0, 1)]);

        assert!(
            g.nodes[2].community.is_some(),
            "no node may be left unlabelled"
        );
    }

    #[test]
    fn an_empty_graph_does_not_panic() {
        let _ = graph_of(0, vec![]);
    }

    /// A symbol belongs to whatever group its file belongs to. Labelling symbols
    /// individually would report as many communities as there are functions.
    #[test]
    fn symbols_inherit_their_files_community() {
        let nodes = vec![
            node("a"),           // 0: file a.rs
            node("b"),           // 1: file b.rs
            symbol("f", "a.rs"), // 2: function in a.rs
            symbol("g", "b.rs"), // 3: function in b.rs
        ];
        // a.rs imports b.rs; each file also contains its own symbol.
        let g = GraphRAG::build(".", nodes, vec![edge(0, 1), edge(0, 2), edge(1, 3)]);

        assert_eq!(
            g.nodes[2].community, g.nodes[0].community,
            "symbol should follow a.rs"
        );
        assert_eq!(
            g.nodes[3].community, g.nodes[1].community,
            "symbol should follow b.rs"
        );
        // And the two files, joined by an import, ended up together.
        assert_eq!(g.nodes[0].community, g.nodes[1].community);
    }
}
