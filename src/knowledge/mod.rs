mod ast;
pub mod crag;
pub mod graph_rag;

pub use graph_rag::*;

// === GraphRAG-inspired pipeline — entities, relationships, community reports ===

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

/// One hop of a shortest-path result: (from-node, relation label, to-node).
pub type GraphPathStep = (GraphNode, String, GraphNode);

pub fn graph_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prism")
        .join("graph")
}

/// Why no graph is available, when none is.
///
/// Two answers, never conflated. `Disabled` means GraphRAG is off for this agent;
/// `NotFound` that it is on but nothing has been indexed here yet. Reporting the first
/// as the second used to send the user to `prism graph index`, which would have changed
/// nothing — the same argument [`crate::cache::open_error`] makes for the cache store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphUnavailable {
    /// No configuration layer enables GraphRAG. It is a PRISM Hub feature.
    Disabled,
    /// Enabled, but the custom path, `graphify-out/graph.json` and the PRISM default
    /// all missed.
    NotFound,
}

impl GraphUnavailable {
    /// The user-facing explanation, with the remedy that actually applies to each case.
    pub fn message(self) -> String {
        match self {
            GraphUnavailable::Disabled => {
                crate::config::disabled_by_policy("GraphRAG codebase intelligence")
            }
            GraphUnavailable::NotFound => {
                "No graph found. Run `prism graph index` or provide `--graph <path>`.".to_string()
            }
        }
    }
}

/// Whether GraphRAG is enabled, resolved through the full chain: hub policy > project
/// `.prismrc` > global `config.yaml` > default **off**. Re-read per call, so a policy
/// fetched by `prism hub config` applies to the next command without a restart.
pub fn graph_enabled() -> bool {
    crate::config::resolve().graph_enabled()
}

/// Fail with the enrolment hint when GraphRAG is off.
///
/// For the operations that never reach [`find_active_graph`] and so are not covered by
/// its gate: `index`, `import`, `extract`, `export`, `stats`, `search_graph_with_path`,
/// the CRAG fallback in `query_graph`, and the MCP graph tools. `cli::graph` states the
/// same refusal once for all nine subcommands.
pub fn ensure_enabled() -> Result<()> {
    if graph_enabled() {
        return Ok(());
    }
    anyhow::bail!("{}", GraphUnavailable::Disabled.message())
}

/// `Some(reason)` when a graph lookup cannot produce anything — for reporting paths that
/// have already seen a `None` and must say *which* `None` it was.
pub fn graph_unavailable(custom: Option<&Path>) -> Option<GraphUnavailable> {
    active_graph_when(graph_enabled(), custom).err()
}

/// Locate the active graph for a given policy decision: custom path, local
/// `graphify-out/graph.json`, or the PRISM default.
///
/// The gate is a parameter rather than a config read so that both failure modes are
/// reachable in a test without a config file on disk, and so the one policy read per
/// operation happens at a call site that can be seen.
pub fn active_graph_when(
    enabled: bool,
    custom: Option<&Path>,
) -> std::result::Result<(PathBuf, GraphRAG), GraphUnavailable> {
    if !enabled {
        return Err(GraphUnavailable::Disabled);
    }

    if let Some(p) = custom {
        if let Ok(rag) = GraphRAG::load_any(p) {
            return Ok((p.to_path_buf(), rag));
        }
    }

    // 1. Check local graphify-out/graph.json in current directory
    let local_graphify = PathBuf::from("graphify-out/graph.json");
    if local_graphify.exists() {
        if let Ok(rag) = GraphRAG::load_any(&local_graphify) {
            return Ok((local_graphify, rag));
        }
    }

    // 2. Check PRISM standard indexed graph
    let prism_graph = graph_dir().join("codebase_graph.json");
    if prism_graph.exists() {
        if let Ok(rag) = GraphRAG::load_any(&prism_graph) {
            return Ok((prism_graph, rag));
        }
    }

    Err(GraphUnavailable::NotFound)
}

/// Find the active codebase graph: custom path, local graphify-out/graph.json, or PRISM
/// default.
///
/// This is the mandatory first step of every read-side graph operation, which is why the
/// policy gate lives here: one check covers the `prism graph` query subcommands and the
/// `prism_graph_*` MCP tools alike. `None` therefore now means *either* "disabled" or
/// "nothing indexed" — anything that reports to a user must ask [`graph_unavailable`]
/// which, never assume the second.
pub fn find_active_graph(custom: Option<&Path>) -> Option<(PathBuf, GraphRAG)> {
    active_graph_when(graph_enabled(), custom).ok()
}

/// Struct-returning graph query, shared by the CLI's `--json` mode and the MCP
/// `prism_graph_query` tool. `None` means no active graph was found (custom path,
/// local `graphify-out/graph.json`, or the PRISM default all missed).
pub fn query_graph_data(
    query: &str,
    top_k: usize,
    custom_path: Option<&Path>,
) -> Option<(PathBuf, Vec<GraphNode>)> {
    let (path, rag) = find_active_graph(custom_path)?;
    let matches = rag.query(query, top_k);
    Some((path, matches))
}

pub async fn query_graph(query: &str, custom_path: Option<&Path>) -> Result<()> {
    // Before the CRAG fallback below, not after: corrective retrieval is part of the
    // same feature, so a disabled graph must not silently answer from it.
    ensure_enabled()?;
    if let Some((path, matches)) = query_graph_data(query, 8, custom_path) {
        if !matches.is_empty() {
            println!("\n  Graph Query Results (Source: {})", path.display());
            println!("  {}", "═".repeat(60));
            for node in &matches {
                let comm_str = node
                    .community
                    .map(|c| format!(" | comm: {}", c))
                    .unwrap_or_default();
                println!(
                    "  • [{}] {} (path: {}{})",
                    node.kind, node.label, node.path, comm_str
                );
            }
            println!();
            return Ok(());
        }
    }

    let crag_results = crag::corrective_retrieve(query, 0.4).await?;
    if crag_results.is_empty() {
        println!("No graph results found for: {query}");
    } else {
        println!("Knowledge Graph results (CRAG retrieved):\n");
        for res in &crag_results {
            let tag = match res.source {
                crag::CragSource::Direct => "direct",
                crag::CragSource::Corrected => "corrected",
            };
            println!("  • [{}] (rel: {:.2}) {}", tag, res.relevance, res.content);
        }
    }
    Ok(())
}

/// Struct-returning node explanation, shared by the CLI's `--json` mode and the MCP
/// `prism_graph_explain` tool.
pub fn explain_node_data(
    node_name: &str,
    custom_path: Option<&Path>,
) -> Option<(PathBuf, NodeExplanationOwned)> {
    let (path, rag) = find_active_graph(custom_path)?;
    let exp = rag.explain_node(node_name)?;
    Some((path, exp.into()))
}

pub async fn explain_node(node_name: &str, custom_path: Option<&Path>) -> Result<()> {
    use colored::Colorize;

    let Some((path, exp)) = explain_node_data(node_name, custom_path) else {
        match graph_unavailable(custom_path) {
            Some(why) => println!("{}", why.message()),
            None => println!("Node '{}' not found in graph.", node_name),
        }
        return Ok(());
    };

    println!("\n  Node: {}", exp.node.label.green().bold());
    println!("  {}", "═".repeat(50));
    println!("  Source Graph:  {}", path.display());
    println!("  ID:            {}", exp.node.id.cyan());
    println!("  Kind:          {}", exp.node.kind);
    println!("  Location:      {}", exp.node.path);
    if let Some(comm) = exp.node.community {
        println!("  Community:     {}", comm);
    }
    println!(
        "  Degree:        {}",
        exp.outgoing.len() + exp.incoming.len()
    );

    if !exp.outgoing.is_empty() {
        println!("\n  Outgoing Connections ({}):", exp.outgoing.len());
        for (target, kind, weight) in exp.outgoing.iter().take(15) {
            println!(
                "    --> {} [{}] (w: {:.1}) in {}",
                target.label.yellow(),
                kind,
                weight,
                target.path
            );
        }
    }

    if !exp.incoming.is_empty() {
        println!("\n  Incoming Connections ({}):", exp.incoming.len());
        for (source, kind, weight) in exp.incoming.iter().take(15) {
            println!(
                "    <-- {} [{}] (w: {:.1}) in {}",
                source.label.cyan(),
                kind,
                weight,
                source.path
            );
        }
    }
    println!();
    Ok(())
}

/// Struct-returning shortest path, shared by the CLI's `--json` mode and the MCP
/// `prism_graph_path` tool.
pub fn shortest_path_data(
    from: &str,
    to: &str,
    custom_path: Option<&Path>,
) -> Option<(PathBuf, Vec<GraphPathStep>)> {
    let (path, rag) = find_active_graph(custom_path)?;
    let steps = rag.shortest_path(from, to)?;
    Some((path, steps))
}

pub async fn shortest_path(from: &str, to: &str, custom_path: Option<&Path>) -> Result<()> {
    use colored::Colorize;

    if let Some(why) = graph_unavailable(custom_path) {
        println!("{}", why.message());
        return Ok(());
    }

    println!(
        "\n  Finding dependency path: {} ➔ {}",
        from.cyan(),
        to.green()
    );
    if let Some((path, _)) = find_active_graph(custom_path) {
        println!("  Source Graph: {}", path.display());
    }
    println!("  {}", "═".repeat(50));

    match shortest_path_data(from, to, custom_path) {
        Some((_, steps)) if steps.is_empty() => {
            println!("  Identical node: '{}' is '{}'.", from, to);
        }
        Some((_, steps)) => {
            println!("  Shortest path ({} hops):\n", steps.len());
            for (i, (src, rel, tgt)) in steps.iter().enumerate() {
                println!(
                    "    [{}] {}  --[{}]-->  {}",
                    i + 1,
                    src.label.cyan(),
                    rel.yellow(),
                    tgt.label.green()
                );
            }
            println!();
        }
        None => {
            println!(
                "  No path found between '{}' and '{}' in graph.\n",
                from, to
            );
        }
    }
    Ok(())
}

/// Struct-returning god-nodes listing, shared by the CLI's `--json` mode and the MCP
/// `prism_graph_god_nodes` tool.
pub fn god_nodes_data(
    top: usize,
    custom_path: Option<&Path>,
) -> Option<(PathBuf, Vec<(GraphNode, usize)>)> {
    let (path, rag) = find_active_graph(custom_path)?;
    let hubs = rag
        .god_nodes(top)
        .into_iter()
        .map(|(n, d)| (n.clone(), d))
        .collect();
    Some((path, hubs))
}

pub async fn god_nodes(top: usize, custom_path: Option<&Path>) -> Result<()> {
    use colored::Colorize;

    let Some((path, hubs)) = god_nodes_data(top, custom_path) else {
        println!(
            "{}",
            graph_unavailable(custom_path)
                .unwrap_or(GraphUnavailable::NotFound)
                .message()
        );
        return Ok(());
    };

    println!(
        "\n  God Nodes / Architectural Hubs (Source: {})",
        path.display()
    );
    println!("  {}", "═".repeat(60));

    for (i, (node, degree)) in hubs.iter().enumerate() {
        let comm_str = node
            .community
            .map(|c| format!("comm: {}", c))
            .unwrap_or_else(|| "none".to_string());
        println!(
            "  {:2}. {:<28} {:>3} edges  [{}] in {} ({})",
            i + 1,
            node.label.cyan().bold(),
            degree.to_string().yellow(),
            node.kind,
            node.path,
            comm_str
        );
    }
    println!();
    Ok(())
}

pub async fn import_graph(path: &Path) -> Result<()> {
    ensure_enabled()?;
    println!("Importing graph from: {}", path.display());
    let rag = GraphRAG::load_any(path)?;
    std::fs::create_dir_all(graph_dir())?;
    let target = graph_dir().join("codebase_graph.json");
    let json = serde_json::to_string_pretty(&rag)?;
    std::fs::write(&target, json)?;
    println!(
        "Successfully imported and activated {} nodes and {} edges into: {}",
        rag.nodes.len(),
        rag.edges.len(),
        target.display()
    );
    Ok(())
}

pub async fn index_codebase<P: AsRef<Path>>(dir: P) -> Result<()> {
    ensure_enabled()?;
    let root = dir.as_ref();
    println!("Scanning and indexing codebase at: {}", root.display());
    let rag = GraphRAG::build_from_dir(root);
    std::fs::create_dir_all(graph_dir())?;
    let path = graph_dir().join("codebase_graph.json");
    let json = serde_json::to_string_pretty(&rag)?;
    std::fs::write(&path, json)?;
    println!(
        "Indexed codebase into GraphRAG: {} nodes, {} dependency edges (saved to: {})",
        rag.nodes.len(),
        rag.edges.len(),
        path.display()
    );
    Ok(())
}

pub async fn extract_from_source(source: &str) -> Result<()> {
    ensure_enabled()?;
    let content = std::fs::read_to_string(source)?;
    let entities = extract_entities(&content)?;
    let relationships = extract_relationships(&content, &entities)?;
    let reports = build_community_reports(&entities, &relationships)?;

    save_entities(&entities)?;
    save_relationships(&relationships)?;
    save_community_reports(&reports)?;

    println!(
        "Extracted {} entities, {} relationships, {} community reports from {}",
        entities.len(),
        relationships.len(),
        reports.len(),
        source
    );
    Ok(())
}

pub async fn export_to_obsidian(output_dir: &str) -> Result<()> {
    ensure_enabled()?;
    let entities = load_all_entities()?;
    let relationships = load_all_relationships()?;
    let out = PathBuf::from(output_dir);
    std::fs::create_dir_all(&out)?;

    for entity in &entities {
        let path = out.join(format!("{}.md", slugify(&entity.name)));
        let connected: Vec<&str> = relationships
            .iter()
            .filter(|r| r.source == entity.name)
            .map(|r| r.target.as_str())
            .collect();

        let content = format!(
            "---\ntitle: {}\ntype: {}\n---\n\n# {}\n\n{}\n\n## Related\n{}\n",
            entity.name,
            entity.entity_type,
            entity.name,
            entity.description,
            connected
                .iter()
                .map(|c| format!("- [[{}]]", c))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        std::fs::write(path, content)?;
    }

    let index = format!(
        "---\ntitle: Knowledge Graph Index\n---\n\n# Entity Index\n\n{}\n",
        entities
            .iter()
            .map(|e| format!("- [[{}]] ({})", e.name, e.entity_type))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    std::fs::write(out.join("index.md"), index)?;

    println!("Exported to Obsidian: {}", out.display());
    Ok(())
}

pub async fn graph_stats(custom_path: Option<&Path>) -> Result<()> {
    use colored::Colorize;

    ensure_enabled()?;
    let entities = load_all_entities()?;
    let relationships = load_all_relationships()?;
    let reports = load_community_reports()?;

    println!("\n  Knowledge Graph Statistics");
    println!("  {}", "═".repeat(40));
    println!(
        "  Entities:           {}",
        entities.len().to_string().cyan()
    );
    println!(
        "  Relationships:      {}",
        relationships.len().to_string().cyan()
    );
    println!("  Community reports:  {}", reports.len().to_string().cyan());

    if let Some((path, rag)) = find_active_graph(custom_path) {
        println!(
            "  Active Graph:       {}",
            path.display().to_string().yellow()
        );
        println!(
            "  Codebase Nodes:     {}",
            rag.nodes.len().to_string().cyan()
        );
        println!(
            "  Codebase Edges:     {}",
            rag.edges.len().to_string().cyan()
        );
    }

    let mut connection_counts: HashMap<&str, usize> = HashMap::new();
    for rel in &relationships {
        *connection_counts.entry(&rel.source).or_default() += 1;
        *connection_counts.entry(&rel.target).or_default() += 1;
    }
    let mut top: Vec<_> = connection_counts.into_iter().collect();
    top.sort_by_key(|a| std::cmp::Reverse(a.1));
    if !top.is_empty() {
        println!("\n  Most connected entities:");
        for (name, count) in top.into_iter().take(5) {
            println!("    {} connections  {}", count, name);
        }
    }
    Ok(())
}

// --- types ---
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct KnowledgeEntity {
    pub name: String,
    pub entity_type: String,
    pub description: String,
    pub properties: HashMap<String, String>,
    pub connections: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct KnowledgeRelationship {
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub weight: f32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct KnowledgeCommunityReport {
    pub title: String,
    pub summary: String,
    pub entities: Vec<String>,
    pub findings: Vec<String>,
}

// --- internal ---
fn extract_entities(text: &str) -> Result<Vec<KnowledgeEntity>> {
    let mut entities = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for word in text.split_whitespace() {
        let clean: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
        if clean
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
            && clean.len() > 2
            && !seen.contains(&clean)
        {
            seen.insert(clean.clone());
            entities.push(KnowledgeEntity {
                name: clean.clone(),
                entity_type: "concept".to_string(),
                description: format!("Extracted from context around '{}'", clean),
                properties: HashMap::new(),
                connections: Vec::new(),
                created_at: chrono::Utc::now(),
            });
        }
    }
    Ok(entities)
}

fn extract_relationships(
    _text: &str,
    entities: &[KnowledgeEntity],
) -> Result<Vec<KnowledgeRelationship>> {
    let mut rels = Vec::new();
    for i in 0..entities.len() {
        for j in (i + 1)..entities.len().min(i + 4) {
            rels.push(KnowledgeRelationship {
                source: entities[i].name.clone(),
                target: entities[j].name.clone(),
                relation_type: "related_to".to_string(),
                weight: 1.0,
            });
        }
    }
    Ok(rels)
}

fn build_community_reports(
    entities: &[KnowledgeEntity],
    _relationships: &[KnowledgeRelationship],
) -> Result<Vec<KnowledgeCommunityReport>> {
    let mut by_type: HashMap<&str, Vec<&KnowledgeEntity>> = HashMap::new();
    for entity in entities {
        by_type.entry(&entity.entity_type).or_default().push(entity);
    }
    Ok(by_type
        .into_iter()
        .map(|(etype, group)| KnowledgeCommunityReport {
            title: format!("{} Community", etype),
            summary: format!(
                "A community of {} entities of type '{}'",
                group.len(),
                etype
            ),
            entities: group.iter().map(|e| e.name.clone()).collect(),
            findings: vec![format!(
                "Contains {} entities sharing the '{}' type",
                group.len(),
                etype
            )],
        })
        .collect())
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

fn save_entities(entities: &[KnowledgeEntity]) -> Result<()> {
    std::fs::create_dir_all(graph_dir())?;
    let path = graph_dir().join("entities.jsonl");
    for entity in entities {
        let line = serde_json::to_string(entity)? + "\n";
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(line.as_bytes())?;
    }
    Ok(())
}

fn save_relationships(relationships: &[KnowledgeRelationship]) -> Result<()> {
    std::fs::create_dir_all(graph_dir())?;
    let path = graph_dir().join("relationships.jsonl");
    for rel in relationships {
        let line = serde_json::to_string(rel)? + "\n";
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?
            .write_all(line.as_bytes())?;
    }
    Ok(())
}

fn save_community_reports(reports: &[KnowledgeCommunityReport]) -> Result<()> {
    std::fs::create_dir_all(graph_dir())?;
    std::fs::write(
        graph_dir().join("reports.json"),
        serde_json::to_string_pretty(reports)?,
    )?;
    Ok(())
}

fn load_all_entities() -> Result<Vec<KnowledgeEntity>> {
    let path = graph_dir().join("entities.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn load_all_relationships() -> Result<Vec<KnowledgeRelationship>> {
    let path = graph_dir().join("relationships.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn load_community_reports() -> Result<Vec<KnowledgeCommunityReport>> {
    let path = graph_dir().join("reports.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

pub fn search_graph(query: &str) -> Result<Vec<String>> {
    search_graph_with_path(query, None)
}

pub fn search_graph_with_path(query: &str, custom_path: Option<&Path>) -> Result<Vec<String>> {
    // The entity store below is read directly, bypassing `find_active_graph`, so this
    // path needs its own gate.
    ensure_enabled()?;
    let mut results = Vec::new();
    let entities = load_all_entities().unwrap_or_default();
    let query_lower = query.to_lowercase();
    for e in &entities {
        if e.name.to_lowercase().contains(&query_lower)
            || e.description.to_lowercase().contains(&query_lower)
        {
            results.push(format!(
                "{} ({}) — {}",
                e.name, e.entity_type, e.description
            ));
        }
    }

    if let Some((path, rag)) = find_active_graph(custom_path) {
        let label = path.file_name().and_then(|n| n.to_str()).unwrap_or("Graph");
        for node in rag.query(query, 5) {
            results.push(format!(
                "[{} {}] {} (path: {})",
                label, node.kind, node.label, node.path
            ));
        }
    }

    Ok(results)
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// The failure this gate had to avoid: both chokepoints return `Option`, so gating
    /// naively makes "GraphRAG is off" look identical to "you have not indexed
    /// anything". They are different problems with different fixes, so they are
    /// different values.
    #[test]
    fn a_disabled_graph_is_not_reported_as_a_missing_one() {
        let disabled = active_graph_when(false, None);
        assert_eq!(
            disabled.err(),
            Some(GraphUnavailable::Disabled),
            "policy off must report Disabled, whatever is on disk"
        );

        let missing = active_graph_when(true, Some(Path::new("/nonexistent/graph.json")));
        assert_ne!(
            missing.err(),
            Some(GraphUnavailable::Disabled),
            "an enabled graph that simply is not there must never read as Disabled"
        );

        // …and the two say different things to the user.
        let off = GraphUnavailable::Disabled.message();
        let absent = GraphUnavailable::NotFound.message();
        assert_ne!(off, absent);
        assert!(
            off.contains("disabled") && off.contains("prism hub enroll"),
            "{off}"
        );
        assert!(
            absent.contains("prism graph index") && !absent.contains("hub"),
            "the fix for an unindexed graph is indexing, not enrolling: {absent}"
        );
    }

    /// A disabled gate must not touch the store at all — no directory probe, no load —
    /// so `Disabled` is returned even where a perfectly good graph exists.
    #[test]
    fn the_gate_short_circuits_before_any_lookup() {
        let repo_graph = Path::new("graphify-out/graph.json");
        if repo_graph.exists() {
            assert_eq!(
                active_graph_when(false, Some(repo_graph)).err(),
                Some(GraphUnavailable::Disabled),
                "an explicit --graph path must not defeat the gate"
            );
            assert!(
                active_graph_when(true, Some(repo_graph)).is_ok(),
                "…and the same path loads fine once enabled"
            );
        }
    }
}
