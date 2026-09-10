// PRISM memory.rs — 3-layer Memory Palace (Recall → Core → Archive)
// Persistence layer: JSONL for each memory level

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::cache::SemanticCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MemoryLayer {
    Recall,
    Core,
    Archive,
}

impl std::fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryLayer::Recall => write!(f, "recall"),
            MemoryLayer::Core => write!(f, "core"),
            MemoryLayer::Archive => write!(f, "archive"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBlock {
    pub id: String,
    pub layer: MemoryLayer,
    pub content: String,
    pub category: String,
    pub keywords: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub access_count: u64,
    pub related_blocks: Vec<String>,
    pub version: u32,
}

impl MemoryBlock {
    pub fn new(layer: MemoryLayer, content: &str, category: &str) -> Self {
        let now = Utc::now().to_rfc3339();
        let trimmed = if content.len() > 10_000 {
            &content[..10_000]
        } else {
            content
        };
        Self {
            id: format!("mem-{}", Utc::now().timestamp_millis()),
            layer,
            content: trimmed.to_string(),
            category: category.to_string(),
            keywords: Self::extract_keywords(content),
            created_at: now.clone(),
            updated_at: now,
            access_count: 0,
            related_blocks: Vec::new(),
            version: 1,
        }
    }

    fn extract_keywords(content: &str) -> Vec<String> {
        let lower = content.to_lowercase();
        let words: Vec<&str> = lower
            .split_whitespace()
            .filter(|w| w.len() > 3 && !is_stop_word(w))
            .collect();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        words
            .into_iter()
            .filter(|w| seen.insert(w.to_string()))
            .map(String::from)
            .collect()
    }
}

fn is_stop_word(w: &str) -> bool {
    matches!(
        w,
        "this"
            | "that"
            | "with"
            | "from"
            | "have"
            | "has"
            | "had"
            | "been"
            | "were"
            | "being"
            | "would"
            | "could"
            | "should"
            | "there"
            | "their"
            | "they"
            | "them"
            | "then"
            | "than"
            | "into"
            | "over"
            | "such"
            | "make"
            | "like"
            | "just"
            | "some"
            | "what"
            | "when"
            | "where"
            | "which"
            | "while"
            | "only"
            | "also"
            | "does"
            | "even"
            | "each"
    )
}

pub struct MemoryPalace {
    data_dir: PathBuf,
    pub recall_blocks: Vec<MemoryBlock>,
    pub core_blocks: Vec<MemoryBlock>,
    pub archive_blocks: Vec<MemoryBlock>,
    pub semantic_cache: SemanticCache,
}

impl MemoryPalace {
    pub fn new(data_dir: PathBuf) -> std::io::Result<Self> {
        for layer in &["recall", "core", "archive"] {
            let d = data_dir.join("sessions").join(layer);
            fs::create_dir_all(&d)?;
        }
        let mut palace = MemoryPalace {
            data_dir: data_dir.clone(),
            recall_blocks: Self::load_layer(&data_dir, "recall")?,
            core_blocks: Self::load_layer(&data_dir, "core")?,
            archive_blocks: Self::load_layer(&data_dir, "archive")?,
            semantic_cache: SemanticCache::new(data_dir)?,
        };
        palace.load_cache()?;
        Ok(palace)
    }

    pub fn save(&mut self, layer: MemoryLayer, content: &str, category: &str) {
        let mut block = MemoryBlock::new(layer, content, category);
        let all_blocks: Vec<MemoryBlock> = self.all_blocks();
        for existing in &all_blocks {
            let shared: usize = block
                .keywords
                .iter()
                .filter(|kw| existing.keywords.contains(kw))
                .count();
            if shared >= 2 {
                block.related_blocks.push(existing.id.clone());
            }
        }
        match layer {
            MemoryLayer::Recall => self.recall_blocks.push(block.clone()),
            MemoryLayer::Core => self.core_blocks.push(block.clone()),
            MemoryLayer::Archive => self.archive_blocks.push(block.clone()),
        }
        self.save_layer(layer, &block);
    }

    pub fn search(&self, query: &str, max_results: usize) -> Vec<MemoryBlock> {
        let query_words: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let all_blocks: Vec<MemoryBlock> = self.all_blocks();
        let mut scored: Vec<(usize, MemoryBlock)> = Vec::new();
        for block in &all_blocks {
            let score: usize = block
                .keywords
                .iter()
                .filter(|kw| query_words.contains(kw))
                .count();
            if score > 0 {
                scored.push((score, block.clone()));
            }
        }
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        let results: Vec<MemoryBlock> = scored
            .into_iter()
            .take(max_results)
            .map(|(_, b)| b)
            .collect();

        if results.is_empty() {
            // Return cached entries as memory blocks
            self.semantic_cache
                .find_similar(query, max_results)
                .into_iter()
                .map(|hit| MemoryBlock {
                    id: format!("_cached_{}", hit.entry.key_hash),
                    layer: MemoryLayer::Core,
                    content: hit.entry.response,
                    category: "cache".to_string(),
                    keywords: Vec::new(),
                    created_at: Utc::now().to_rfc3339(),
                    updated_at: Utc::now().to_rfc3339(),
                    access_count: 0,
                    related_blocks: Vec::new(),
                    version: 0,
                })
                .collect()
        } else {
            results
        }
    }

    pub fn recommend_layer(&self, content: &str) -> MemoryLayer {
        let keywords = MemoryBlock::extract_keywords(content);
        let unique_count = keywords.len();
        let depth = content.len();
        if unique_count > 30 || depth > 5000 {
            MemoryLayer::Archive
        } else if unique_count > 15 {
            MemoryLayer::Core
        } else {
            MemoryLayer::Recall
        }
    }

    pub fn evict_recall(&mut self, max_blocks: usize) {
        if self.recall_blocks.len() > max_blocks {
            self.recall_blocks
                .drain(0..(self.recall_blocks.len() - max_blocks));
        }
    }

    pub fn evict_core(&mut self, max_blocks: usize) {
        if self.core_blocks.len() > max_blocks {
            self.core_blocks.clear();
        }
    }

    pub fn all_blocks(&self) -> Vec<MemoryBlock> {
        let mut all: Vec<MemoryBlock> = self
            .recall_blocks
            .iter()
            .chain(self.core_blocks.iter())
            .chain(self.archive_blocks.iter())
            .cloned()
            .collect();
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        all
    }

    pub fn count(&self, layer: MemoryLayer) -> usize {
        match layer {
            MemoryLayer::Recall => self.recall_blocks.len(),
            MemoryLayer::Core => self.core_blocks.len(),
            MemoryLayer::Archive => self.archive_blocks.len(),
        }
    }

    pub fn export_jsonl(&self) -> String {
        let all = self.all_blocks();
        all.into_iter()
            .map(|b| serde_json::to_string(&b).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn load_cache(&mut self) -> std::io::Result<()> {
        for dir in &["recall", "core", "archive"] {
            let p = self.data_dir.join("cache").join(dir);
            if p.exists() {
                // could load entries here
            }
        }
        Ok(())
    }

    fn load_layer(
        data_dir: &std::path::Path,
        layer: &str,
    ) -> Result<Vec<MemoryBlock>, std::io::Error> {
        let mut blocks = Vec::new();
        let path = data_dir.join("sessions").join(layer).join("blocks.jsonl");
        if let Ok(s) = fs::read_to_string(&path) {
            for line in s.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(block) = serde_json::from_str::<MemoryBlock>(line) {
                    blocks.push(block);
                }
            }
        }
        Ok(blocks)
    }

    fn save_layer(&self, layer: MemoryLayer, block: &MemoryBlock) {
        let layer_name = match layer {
            MemoryLayer::Recall => "recall",
            MemoryLayer::Core => "core",
            MemoryLayer::Archive => "archive",
        };
        let path = self.data_dir.join("sessions").join(layer_name);
        let jsonl_path = path.join("blocks.jsonl");
        let json = serde_json::to_string(block).unwrap_or_default();
        append_to_file(&jsonl_path, &format!("{}\n", json)).ok();
    }
}

fn append_to_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}

// --- module-level async API for cli.rs / mcp.rs ---

fn memory_palace_dir() -> std::path::PathBuf {
    crate::prism_data_dir()
}

pub fn memory_palace_dir_pub() -> std::path::PathBuf {
    memory_palace_dir()
}

/// Struct-returning search, shared by the CLI's `--json` mode and the MCP
/// `prism_memory_search` tool — the single place that touches `MemoryPalace` for a
/// search, so both callers see identical results.
pub fn search_blocks(query: &str, max_results: usize) -> anyhow::Result<Vec<MemoryBlock>> {
    let palace = MemoryPalace::new(memory_palace_dir()).map_err(|e| anyhow::anyhow!(e))?;
    Ok(palace.search(query, max_results))
}

pub async fn search(query: &str) -> anyhow::Result<()> {
    let results = search_blocks(query, 10)?;
    if results.is_empty() {
        println!("No memories found for: {query}");
    } else {
        for block in &results {
            let preview = &block.content[..block.content.len().min(200)];
            println!("[{}] {}: {}", block.layer, block.category, preview);
        }
    }
    Ok(())
}

pub async fn save(key: &str, value: &str) -> anyhow::Result<()> {
    let mut palace = MemoryPalace::new(memory_palace_dir()).map_err(|e| anyhow::anyhow!(e))?;
    palace.save(MemoryLayer::Core, value, key);
    println!("Saved memory: {key}");
    Ok(())
}

pub async fn list() -> anyhow::Result<()> {
    let palace = MemoryPalace::new(memory_palace_dir()).map_err(|e| anyhow::anyhow!(e))?;
    let all = palace.all_blocks();
    if all.is_empty() {
        println!("Memory palace is empty.");
    } else {
        println!("Memory Palace — {} blocks:", all.len());
        for block in all.iter().take(20) {
            let preview = &block.content[..block.content.len().min(100)];
            println!("  [{}] {}", block.layer, preview);
        }
        if all.len() > 20 {
            println!("  … {} more", all.len() - 20);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    pub recall: usize,
    pub core: usize,
    pub archive: usize,
}

/// Struct-returning stats, shared by the CLI's `--json` mode and the MCP
/// `prism_memory_stats` tool.
pub fn stats_data() -> anyhow::Result<MemoryStats> {
    let palace = MemoryPalace::new(memory_palace_dir()).map_err(|e| anyhow::anyhow!(e))?;
    Ok(MemoryStats {
        recall: palace.count(MemoryLayer::Recall),
        core: palace.count(MemoryLayer::Core),
        archive: palace.count(MemoryLayer::Archive),
    })
}

pub async fn stats() -> anyhow::Result<()> {
    let stats = stats_data()?;
    println!("Memory Palace Statistics:");
    println!("  Recall:  {} blocks", stats.recall);
    println!("  Core:    {} blocks", stats.core);
    println!("  Archive: {} blocks", stats.archive);
    Ok(())
}

pub async fn compact() -> anyhow::Result<()> {
    let mut palace = MemoryPalace::new(memory_palace_dir()).map_err(|e| anyhow::anyhow!(e))?;
    let before = palace.count(MemoryLayer::Recall);
    palace.evict_recall(100);
    let after = palace.count(MemoryLayer::Recall);
    println!("Memory compacted: recall {} → {} blocks", before, after);
    Ok(())
}
