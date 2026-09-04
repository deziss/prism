// PRISM cache.rs — Semantic cache: sled persistence + TurboVec ANN similarity search

use crate::vector::TurboVecIndex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use chrono::Utc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key_hash: String,
    pub prompt_hash: String,
    pub response: String,
    pub model: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub accessed_at: u64,
    pub last_saved: u64,
    pub access_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: usize,
    pub sled_path: String,
}

pub struct SemanticCache {
    data_dir: PathBuf,
    sled_db: sled::Tree,
    entries: HashMap<String, CacheEntry>,
    turbo: TurboVecIndex,
}

static GLOBAL_CACHE: OnceLock<Option<Mutex<SemanticCache>>> = OnceLock::new();

/// Get or initialize the shared global SemanticCache singleton
pub fn global_cache() -> Option<&'static Mutex<SemanticCache>> {
    GLOBAL_CACHE.get_or_init(|| {
        let dir = crate::prism_data_dir();
        SemanticCache::new(dir).map(Mutex::new).ok()
    }).as_ref()
}

/// Convenience global helper: save response for a prompt
pub fn cache_response(prompt: &str, response: &str, model: &str) {
    if let Some(cache_lock) = global_cache() {
        if let Ok(mut cache) = cache_lock.lock() {
            cache.put_with_model(prompt, response, model);
        }
    }
}

/// Convenience global helper: lookup exact match
pub fn lookup_exact(prompt: &str) -> Option<String> {
    if let Some(cache_lock) = global_cache() {
        if let Ok(cache) = cache_lock.lock() {
            return cache.get(prompt);
        }
    }
    None
}

/// Convenience global helper: lookup similar match
pub fn lookup_similar(query: &str, top_n: usize) -> Vec<CacheEntry> {
    if let Some(cache_lock) = global_cache() {
        if let Ok(cache) = cache_lock.lock() {
            return cache.find_similar(query, top_n);
        }
    }
    Vec::new()
}

/// Convenience global helper: get cache stats
pub fn get_cache_stats() -> CacheStats {
    if let Some(cache_lock) = global_cache() {
        if let Ok(cache) = cache_lock.lock() {
            return CacheStats {
                total_entries: cache.len(),
                sled_path: cache.data_dir.display().to_string(),
            };
        }
    }
    CacheStats {
        total_entries: 0,
        sled_path: crate::prism_data_dir().join("cache").display().to_string(),
    }
}

/// Convenience global helper: clear cache
pub fn clear_cache() -> std::io::Result<usize> {
    if let Some(cache_lock) = global_cache() {
        if let Ok(mut cache) = cache_lock.lock() {
            let count = cache.len();
            cache.clear()?;
            return Ok(count);
        }
    }
    Ok(0)
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

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn get(&self, prompt: &str) -> Option<String> {
        let hash = Self::hash_input(prompt);
        self.entries.get(&hash).map(|e| e.response.clone())
    }

    pub fn put(&mut self, prompt: &str, response: &str) {
        self.put_with_model(prompt, response, "unknown");
    }

    pub fn put_with_model(&mut self, prompt: &str, response: &str, model: &str) {
        let hash = Self::hash_input(prompt);
        let timestamp = Utc::now().timestamp() as u64;
        let emb = Self::pseudo_embedding(prompt);
        let entry = CacheEntry {
            key_hash: hash.clone(),
            prompt_hash: hash.clone(),
            response: response.to_string(),
            model: Some(model.to_string()),
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

    pub fn clear(&mut self) -> std::io::Result<()> {
        let _ = self.sled_db.clear();
        let _ = self.sled_db.flush();
        self.entries.clear();
        self.turbo = TurboVecIndex::new();
        Ok(())
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

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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

    /// 16-dim normalized SimHash/MinHash feature projection for fast ANN search
    fn pseudo_embedding(input: &str) -> Vec<f32> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut emb = vec![0.0f32; 16];
        let words: Vec<&str> = input.split_whitespace().collect();
        if words.is_empty() {
            return emb;
        }

        // Word tokens
        for word in &words {
            let mut h = DefaultHasher::new();
            word.to_lowercase().hash(&mut h);
            let val = h.finish();
            let dim = (val % 16) as usize;
            let sign = if (val >> 4) & 1 == 1 { 1.0f32 } else { -1.0f32 };
            emb[dim] += sign;
        }

        // Character 3-grams
        let bytes = input.as_bytes();
        for window in bytes.windows(3) {
            let mut h = DefaultHasher::new();
            window.hash(&mut h);
            let val = h.finish();
            let dim = (val % 16) as usize;
            let sign = if (val >> 5) & 1 == 1 { 0.5f32 } else { -0.5f32 };
            emb[dim] += sign;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pseudo_embedding_dimension_and_norm() {
        let emb = SemanticCache::pseudo_embedding("hello world test prompt");
        assert_eq!(emb.len(), 16);
        let norm: f32 = emb.iter().map(|e| e * e).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_pseudo_embedding_similarity() {
        let e1 = SemanticCache::pseudo_embedding("fn calculate_tokens(text: &str) -> usize");
        let e2 = SemanticCache::pseudo_embedding("fn calculate_tokens(input: &str) -> usize");
        let e3 = SemanticCache::pseudo_embedding("kubernetes cluster deployment yaml replica");

        let dot12: f32 = e1.iter().zip(&e2).map(|(a, b)| a * b).sum();
        let dot13: f32 = e1.iter().zip(&e3).map(|(a, b)| a * b).sum();

        assert!(dot12 > dot13, "Similar code signatures must have higher similarity than unrelated topics");
    }
}
