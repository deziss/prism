// PRISM cache.rs — Semantic cache: sled persistence + TurboVec ANN similarity search

use crate::vector::TurboVecIndex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use chrono::Utc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key_hash: String,
    pub prompt_hash: String,
    pub response: String,
    pub embedding: Option<Vec<f32>>,
    pub accessed_at: u64,
    pub last_saved: u64,
    pub access_count: u64,
}

pub struct SemanticCache {
    data_dir: PathBuf,
    sled_db: sled::Tree,
    entries: HashMap<String, CacheEntry>,
    turbo: TurboVecIndex,
}

impl SemanticCache {
    pub fn new(data_dir: PathBuf) -> std::io::Result<Self> {
        let sled_dir = data_dir.join("cache").join("sled");
        std::fs::create_dir_all(&sled_dir)?;
        let sled_db = sled::open(&sled_dir)?.open_tree("cache")?;
        let mut cache = SemanticCache {
            data_dir: data_dir.join("cache"),
            sled_db,
            entries: HashMap::new(),
            turbo: TurboVecIndex::new(),
        };
        cache.load_entries()?;
        Ok(cache)
    }

    pub fn get(&self, prompt: &str) -> Option<String> {
        let hash = Self::hash_input(prompt);
        self.entries.get(&hash).map(|e| e.response.clone())
    }

    pub fn put(&mut self, prompt: &str, response: &str) {
        let hash = Self::hash_input(prompt);
        let timestamp = Utc::now().timestamp() as u64;
        let emb = Self::pseudo_embedding(prompt);
        let entry = CacheEntry {
            key_hash: hash.clone(),
            prompt_hash: hash.clone(),
            response: response.to_string(),
            embedding: Some(emb.clone()),
            accessed_at: timestamp,
            last_saved: timestamp,
            access_count: 1,
        };
        self.turbo.add(&hash, &emb);
        self.entries.insert(hash.clone(), entry.clone());
        if let Ok(serialized) = serde_json::to_string(&entry) {
            let _ = self.sled_db.insert(hash.as_bytes(), serialized.into_bytes());
            let _ = self.sled_db.flush();
        }
    }

    pub fn remove(&mut self, prompt: &str) {
        let hash = Self::hash_input(prompt);
        self.turbo.remove(&hash);
        self.entries.remove(&hash);
        let _ = self.sled_db.remove(hash.as_bytes());
    }

    /// Find top-n similar entries using TurboVec ANN search.
    pub fn find_similar(&self, query: &str, top_n: usize) -> Vec<CacheEntry> {
        let query_emb = Self::pseudo_embedding(query);
        self.turbo
            .search(&query_emb, top_n)
            .into_iter()
            .filter_map(|hash| self.entries.get(&hash).cloned())
            .collect()
    }

    pub fn evict_lru(&mut self, max_entries: usize, ttl_seconds: u64) {
        let now = Utc::now().timestamp() as u64;
        let stale: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| now - e.accessed_at >= ttl_seconds)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &stale {
            let _ = self.sled_db.remove(key.as_bytes());
            self.turbo.remove(key);
            self.entries.remove(key);
        }
        if self.entries.len() > max_entries {
            let excess = self.entries.len() - max_entries;
            let mut sorted: Vec<(String, u64)> = self
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v.accessed_at))
                .collect();
            sorted.sort_by_key(|(_, t)| *t);
            for (key, _) in sorted.into_iter().take(excess) {
                let _ = self.sled_db.remove(key.as_bytes());
                self.turbo.remove(&key);
                self.entries.remove(&key);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn flush(&self) -> std::io::Result<usize> {
        self.sled_db
            .flush()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    fn hash_input(input: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// 16-dim normalized pseudo-embedding (dim=16 matches TURBO_DIM).
    fn pseudo_embedding(input: &str) -> Vec<f32> {
        let mut emb = vec![0.0f32; 16];
        for (i, &b) in input.as_bytes().iter().enumerate() {
            emb[i % 16] += b as f32 * 0.0039;
        }
        let norm = emb.iter().map(|e| e * e).sum::<f32>().sqrt().max(0.001);
        emb.iter_mut().for_each(|e| *e /= norm);
        emb
    }

    fn load_entries(&mut self) -> std::io::Result<()> {
        for item in self.sled_db.iter() {
            let (k, v) = item.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            if let Ok(entry) = serde_json::from_slice::<CacheEntry>(v.as_ref()) {
                if let Some(ref emb) = entry.embedding {
                    if emb.len() == 16 {
                        self.turbo.add(&entry.key_hash, emb);
                    }
                }
                let key = String::from_utf8_lossy(k.as_ref()).to_string();
                self.entries.insert(key, entry);
            }
        }
        Ok(())
    }
}

impl Drop for SemanticCache {
    fn drop(&mut self) {
        let _ = self.sled_db.flush();
    }
}
