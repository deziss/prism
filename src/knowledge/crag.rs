//! Corrective Retrieval-Augmented Generation (CRAG)
//!
//! Pipeline: retrieve → evaluate relevance → re-retrieve if below threshold → merge + rank.

use anyhow::Result;

const DEFAULT_THRESHOLD: f32 = 0.4;

#[derive(Debug, Clone)]
pub struct CragResult {
    pub query: String,
    pub content: String,
    pub relevance: f32,
    pub source: CragSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CragSource {
    Direct,
    Corrected,
}

/// Main CRAG entry point: retrieve → evaluate → conditionally re-retrieve.
pub async fn corrective_retrieve(query: &str, threshold: f32) -> Result<Vec<CragResult>> {
    let threshold = if threshold <= 0.0 { DEFAULT_THRESHOLD } else { threshold };

    // Step 1: initial retrieval
    let initial = retrieve_from_graph(query)?;
    let relevance = evaluate_relevance(query, &initial);

    if relevance >= threshold {
        return Ok(initial
            .into_iter()
            .map(|content| CragResult {
                query: query.to_string(),
                content,
                relevance,
                source: CragSource::Direct,
            })
            .collect());
    }

    // Step 2: below threshold — rewrite query + re-retrieve
    let rewritten = corrective_rewrite(query).await;
    let mut corrected = retrieve_from_graph(&rewritten)?;

    // Merge, dedup, keep order
    let mut merged = initial;
    for c in corrected.drain(..) {
        if !merged.contains(&c) {
            merged.push(c);
        }
    }

    let final_relevance = evaluate_relevance(query, &merged);
    Ok(merged
        .into_iter()
        .map(|content| CragResult {
            query: query.to_string(),
            content,
            relevance: final_relevance,
            source: CragSource::Corrected,
        })
        .collect())
}

/// Score average keyword overlap of query terms against result strings (0.0–1.0).
pub fn evaluate_relevance(query: &str, results: &[String]) -> f32 {
    if results.is_empty() {
        return 0.0;
    }
    let query_terms: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if query_terms.is_empty() {
        return 0.0;
    }
    let total: f32 = results
        .iter()
        .map(|result| {
            let rl = result.to_lowercase();
            let matched = query_terms.iter().filter(|t| rl.contains(t.as_str())).count();
            matched as f32 / query_terms.len() as f32
        })
        .sum();
    (total / results.len() as f32).min(1.0)
}

/// Expand query with technical synonyms for broader graph traversal.
pub async fn corrective_rewrite(query: &str) -> String {
    let expanded: Vec<String> = query
        .split_whitespace()
        .flat_map(|w| {
            let synonyms: &[&str] = match w.to_lowercase().as_str() {
                "function" | "fn" => &["function", "fn", "method", "procedure"],
                "class" | "struct" => &["class", "struct", "type", "object"],
                "import" | "use" => &["import", "use", "require", "include"],
                "error" | "err" => &["error", "err", "failure", "exception"],
                "file" | "module" => &["file", "module", "path", "source"],
                "build" | "compile" => &["build", "compile", "cargo", "make"],
                "test" | "spec" => &["test", "spec", "check", "assert"],
                "cache" => &["cache", "store", "index", "lookup"],
                "graph" => &["graph", "node", "edge", "dependency"],
                "memory" => &["memory", "recall", "store", "persist"],
                _ => std::slice::from_ref(&w),
            };
            synonyms.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    expanded.join(" ")
}

fn retrieve_from_graph(query: &str) -> Result<Vec<String>> {
    let entities_path = super::graph_dir().join("entities.jsonl");
    if !entities_path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(entities_path)?;
    let query_lower = query.to_lowercase();
    Ok(content
        .lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let name = v["name"].as_str().unwrap_or("");
            let desc = v["description"].as_str().unwrap_or("");
            if name.to_lowercase().contains(&query_lower)
                || desc.to_lowercase().contains(&query_lower)
            {
                Some(format!("{}: {}", name, desc))
            } else {
                None
            }
        })
        .collect())
}
