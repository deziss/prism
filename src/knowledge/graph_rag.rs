// PRISM knowledge/graph_rag.rs — GraphRAG pipeline for cross-file codebase understanding
// Builds a dependency graph from source files and runs retrieval-augmented generation queries

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[cfg(feature = "graphrag")]
use petgraph::graph::{DiGraph, NodeIndex};

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
        write!(f, "{}", match self {
            GraphNodeKind::File => "file",
            GraphNodeKind::Function => "func",
            GraphNodeKind::Class => "class",
            GraphNodeKind::Import => "import",
            GraphNodeKind::Symbol => "symbol",
        })
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Imports,
    Calls,
    References,
    Inherits,
    Implements,
    Depends,
}

impl std::fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            EdgeKind::Imports => "imports",
            EdgeKind::Calls => "calls",
            EdgeKind::References => "references",
            EdgeKind::Inherits => "inherits",
            EdgeKind::Implements => "implements",
            EdgeKind::Depends => "depends",
        })
    }
}

/// The main GraphRAG structure
#[derive(Debug, Serialize, Deserialize)]
pub struct GraphRAG {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    #[cfg(feature = "graphrag")]
    pub graph: DiGraph<GraphNode, GraphEdge>,
}

impl GraphRAG {
    /// Build a GraphRAG from a directory of source files
    pub fn build<P: AsRef<std::path::Path>>(root: P, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Self {
        let rag = GraphRAG {
            nodes,
            edges,
            #[cfg(feature = "graphrag")]
            graph: DiGraph::new(),
        };

        #[cfg(feature = "graphrag")]
        {
            let mut node_map = std::collections::HashMap::new();
            for node in &rag.nodes {
                let idx = rag.graph.add_node(node.clone());
                node_map.insert(node.id.clone(), idx);
            }
            for edge in &rag.edges {
                let source_id = &rag.nodes[edge.source].id;
                let target_id = &rag.nodes[edge.target].id;
                if let (Some(&si), Some(&ti)) = (node_map.get(source_id), node_map.get(target_id)) {
                    if si != ti {
                        rag.graph.add_edge(si, ti, edge.clone());
                    }
                }
            }
            rag.detect_communities();
        }

        rag
    }

    /// Run a GraphRAG query across the codebase
    pub fn query(&self, query: &str, top_k: usize) -> Vec<GraphNode> {
        let query_tokens = Self::tokenize(query);
        let scored: Vec<(f64, usize, &GraphNode)> = self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                let node_tokens = Self::tokenize(&node.label);
                let overlap: usize = query_tokens.iter().filter(|t| node_tokens.contains(t)).count();
                let recency = 1.0 / (i as f64 + 1.0);
                (overlap as f64 * recency, i, node)
            })
            .collect();

        let mut scored_vec: Vec<(f64, usize, GraphNode)> = scored
            .into_iter()
            .map(|(score, idx, node)| (score, idx, node.clone()))
            .collect();
        scored_vec.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored_vec.into_iter().take(top_k).map(|(_, _, node)| node).collect()
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
                        let source_id = self.nodes[edge.source].id.clone();
                        let target_id = self.nodes[edge.target].id.clone();
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

        let result: Vec<String> = frontier.iter().map(|s| s.clone()).collect();
        let mut sorted_result = result;
        sorted_result.sort();
        sorted_result
    }

    /// Detect communities using simple connected-component analysis
    #[cfg(feature = "graphrag")]
    fn detect_communities(&mut self) {
        let nodes = self.graph.node_indices().collect::<Vec<_>>();
        let mut community_id = 0usize;
        let mut community_assignment: Vec<Option<usize>> = vec![None; self.nodes.len()];

        for idx in &nodes {
            let i = idx.index();
            if community_assignment.get(i).copied().flatten().is_some() { continue; }
            let mut queue = vec![idx.clone()];
            while let Some(node_idx) = queue.pop() {
                let ni = node_idx.index();
                if community_assignment.get(ni).copied().flatten().is_some() { continue; }
                community_assignment[ni] = Some(community_id);
                self.nodes.get_mut(ni).map(|n| n.community = Some(community_id));
                for neighbor in self.graph.neighbors(node_idx) {
                    let ni2 = neighbor.index();
                    if !community_assignment.get(ni2).copied().flatten().is_some() {
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

    fn tokenize(text: &str) -> Vec<String> {
        let lower = text.to_lowercase();
        lower.split_whitespace().map(String::from).collect()
    }
}
