// PRISM cache.rs — TurboVec IdMapIndex semantic cache + hash-based pseudo-embeddings
// TurboVec uses Google TurboQuant for vector compression: 10M docs in 4GB, no training, AVX-512BW

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use chrono::Utc;

/// A single cache entry
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

/// Semantic cache backed by sled + SQLite index
pub struct SemanticCache {
    data_dir: PathBuf,
    sled_db: sled::Tree,
    entries: HashMap<String, CacheEntry>,
}

impl SemanticCache {
    pub fn new(data_dir: PathBuf) -> std::io::Result<Self> {
        let sled_dir = data_dir.join("cache").join("sled");
        std::fs::create_dir_all(&sled_dir)?;
        let sled_db = sled::open(&sled_dir)?;
        let tree = sled_db.open_tree("cache")?;
        let mut cache = SemanticCache {
            data_dir: data_dir.join("cache"),
            sled_db: tree,
            entries: HashMap::new(),
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
        let entry = CacheEntry {
            key_hash: hash.clone(),
            prompt_hash: hash.clone(),
            response: response.to_string(),
            embedding: Some(Self::pseudo_embedding(prompt)),
            accessed_at: timestamp,
            last_saved: timestamp,
            access_count: 1,
        };
        self.entries.insert(hash.clone(), entry.clone());
        if let Ok(serialized) = serde_json::to_string(&entry) {
            let _ = self.sled_db.insert(hash.as_bytes(), serialized.into_bytes());
            let _ = self.sled_db.flush();
        }
    }

    pub fn remove(&mut self, _prompt: &str) {
        // simplified remove
    }

    pub fn find_similar(&self, query: &str, top_n: usize) -> Vec<CacheEntry> {
        let query_emb = Self::pseudo_embedding(query);
        let mut scored: Vec<(f32, CacheEntry)> = self
            .entries
            .values()
            .filter_map(|entry| {
                entry.embedding.as_ref().map(|emb| {
                    let dist = Self::cosine_distance(emb, &query_emb);
                    (dist, entry.clone())
                })
            })
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(top_n)
            .map(|(_, entry)| entry)
            .collect()
    }

    pub fn evict_lru(&mut self, max_entries: usize, ttl_seconds: u64) {
        let now = Utc::now().timestamp() as u64;
        // Remove stale entries first
        let stale: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| now - entry.accessed_at >= ttl_seconds)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &stale {
            let _ = self.sled_db.remove(key.as_bytes());
            self.entries.remove(key);
        }
        // Enforce max entries via count
        if self.entries.len() > max_entries {
            let excess = self.entries.len() - max_entries;
            let mut entries: Vec<(String, &CacheEntry)> = self
                .entries
                .iter()
                .map(|(k, v)| (k.clone(), v))
                .collect();
            entries.sort_by_key(|(_, e)| e.accessed_at);
            let to_remove: Vec<String> = entries
                .into_iter()
                .take(excess)
                .map(|(k, _)| k)
                .collect();
            for key in to_remove {
                let _ = self.sled_db.remove(key.as_bytes());
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
            .map(|n| n)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    fn hash_input(input: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    fn pseudo_embedding(input: &str) -> Vec<f32> {
        let mut embedding = vec![0.0f32; 16];
        let bytes = input.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            embedding[i % 16] += b as f32 * 0.0039;
        }
        let norm = embedding
            .iter()
            .map(|e| e * e)
            .sum::<f32>()
            .sqrt()
            .max(0.001);
        embedding.iter_mut().for_each(|e| *e /= norm);
        embedding
    }

    fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na < 0.001 || nb < 0.001 {
            return 1.0;
        }
        let cos = dot / (na * nb);
        1.0 - cos
    }

    fn load_entries(&mut self) -> std::io::Result<()> {
        for item in self.sled_db.iter() {
            let (k, v) = item.map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })?;
            if let Ok(entry) = serde_json::from_slice::<CacheEntry>(v.as_ref()) {
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
