pub mod crag;
pub mod graph_rag;

pub use graph_rag::*;

// === GraphRAG-inspired pipeline — entities, relationships, community reports ===

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::io::Write as IoWrite;
use std::path::PathBuf;

pub fn graph_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prism")
        .join("graph")
}

pub async fn query_graph(query: &str) -> Result<()> {
    let results = search_graph(query)?;
    if results.is_empty() {
        println!("No graph results for: {query}");
    } else {
        println!("Knowledge Graph results:\n");
        for result in &results {
            println!("  • {}", result);
        }
    }
    Ok(())
}

pub async fn extract_from_source(source: &str) -> Result<()> {
    let content = if source.starts_with("http") {
        let resp = reqwest::get(source).await?;
        resp.text().await?
    } else if std::path::Path::new(source).exists() {
        std::fs::read_to_string(source)?
    } else {
        source.to_string()
    };

    let entities = extract_entities(&content)?;
    let relationships = extract_relationships(&content, &entities)?;

    save_entities(&entities)?;
    save_relationships(&relationships)?;

    let community_reports = build_community_reports(&entities, &relationships)?;
    save_community_reports(&community_reports)?;

    println!(
        "Extracted: {} entities, {} relationships, {} communities",
        entities.len(),
        relationships.len(),
        community_reports.len()
    );
    Ok(())
}

pub async fn export_to_obsidian(output_dir: &str) -> Result<()> {
    let out = PathBuf::from(output_dir);
    std::fs::create_dir_all(&out)?;

    let entities = load_all_entities()?;
    for entity in &entities {
        let filename = slugify(&entity.name) + ".md";
        let path = out.join(&filename);
        let content = format!(
            "---\ntags: [entity, graph]\ntype: {}\n---\n\n# {}\n\n{}\n\n## Connected To\n{}\n",
            entity.entity_type,
            entity.name,
            entity.description,
            entity.connections.iter().map(|c| format!("- [[{}]]", c)).collect::<Vec<_>>().join("\n"),
        );
        std::fs::write(path, content)?;
    }

    let index = format!(
        "---\ntitle: Knowledge Graph Index\n---\n\n# Entity Index\n\n{}\n",
        entities.iter().map(|e| format!("- [[{}]] ({})", e.name, e.entity_type)).collect::<Vec<_>>().join("\n"),
    );
    std::fs::write(out.join("index.md"), index)?;

    println!("Exported to Obsidian: {}", out.display());
    Ok(())
}

pub async fn graph_stats() -> Result<()> {
    use colored::Colorize;

    let entities = load_all_entities()?;
    let relationships = load_all_relationships()?;
    let reports = load_community_reports()?;

    println!("\n  Knowledge Graph Statistics");
    println!("  {}", "═".repeat(40));
    println!("  Entities:           {}", entities.len().to_string().cyan());
    println!("  Relationships:      {}", relationships.len().to_string().cyan());
    println!("  Community reports:  {}", reports.len().to_string().cyan());

    let mut connection_counts: HashMap<&str, usize> = HashMap::new();
    for rel in &relationships {
        *connection_counts.entry(&rel.source).or_default() += 1;
        *connection_counts.entry(&rel.target).or_default() += 1;
    }
    let mut top: Vec<_> = connection_counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\n  Most connected entities:");
    for (name, count) in top.into_iter().take(5) {
        println!("    {} connections  {}", count, name);
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
        if clean.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
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

fn extract_relationships(_text: &str, _entities: &[KnowledgeEntity]) -> Result<Vec<KnowledgeRelationship>> {
    Ok(Vec::new())
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
            summary: format!("A community of {} entities of type '{}'", group.len(), etype),
            entities: group.iter().map(|e| e.name.clone()).collect(),
            findings: vec![format!("Contains {} entities sharing the '{}' type", group.len(), etype)],
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
    std::fs::write(graph_dir().join("reports.json"), serde_json::to_string_pretty(reports)?)?;
    Ok(())
}

fn load_all_entities() -> Result<Vec<KnowledgeEntity>> {
    let path = graph_dir().join("entities.jsonl");
    if !path.exists() { return Ok(Vec::new()); }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn load_all_relationships() -> Result<Vec<KnowledgeRelationship>> {
    let path = graph_dir().join("relationships.jsonl");
    if !path.exists() { return Ok(Vec::new()); }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn load_community_reports() -> Result<Vec<KnowledgeCommunityReport>> {
    let path = graph_dir().join("reports.json");
    if !path.exists() { return Ok(Vec::new()); }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

pub fn search_graph(query: &str) -> Result<Vec<String>> {
    let entities = load_all_entities()?;
    let query_lower = query.to_lowercase();
    Ok(entities
        .iter()
        .filter(|e| {
            e.name.to_lowercase().contains(&query_lower)
                || e.description.to_lowercase().contains(&query_lower)
        })
        .map(|e| format!("{} ({}) — {}", e.name, e.entity_type, e.description))
        .collect())
}
