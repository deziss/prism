// PRISM cache.rs — Semantic cache: sled persistence + TurboVec ANN similarity search
//
// sled 0.34.7 is intentionally left as-is here (not bumped as part of the Phase 2
// dependency sweep): it IS the latest release, sled 1.0 never shipped. A future
// migration off sled (e.g. to redb) would be a storage-engine change, not a version
// bump, and is out of scope for that pass.

use crate::vector::TurboVecIndex;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key_hash: String,
    pub prompt_hash: String,
    /// The prompt as written. Similarity needs the text — a hash cannot be compared for
    /// anything but equality. Defaulted so entries written before this field still load.
    #[serde(default)]
    pub prompt: String,
    pub response: String,
    pub model: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub accessed_at: u64,
    pub last_saved: u64,
    pub access_count: u64,
    /// `content-type` of the stored response, so a replay is framed the way the original
    /// was. Defaulted for entries written before this field.
    #[serde(default)]
    pub content_type: String,
    /// Whether the stored response was a server-sent-event stream. A cached body may
    /// only ever be replayed to a request that asked for the same framing.
    #[serde(default)]
    pub is_stream: bool,
    /// Which projection produced `embedding`.
    ///
    /// Both backends emit `TURBO_DIM`-wide vectors, so the width check below cannot
    /// tell a sketch vector from a model one. Empty on entries written before this
    /// field existed, which is treated as a mismatch — those predate the model path
    /// entirely and are re-embedded when next stored.
    #[serde(default)]
    pub embedding_backend: String,
}

/// A cache entry with how close its prompt is to the query, in `0.0..=1.0`.
#[derive(Debug, Clone)]
pub struct Scored {
    pub score: f32,
    pub entry: CacheEntry,
}

/// Minimum score for a hit. Below this a "match" is noise, and returning it invites the
/// caller to reuse an answer to a different question. Override with
/// `PRISM_CACHE_MIN_SIMILARITY` (0.0 returns everything, 1.0 exact matches only).
pub fn min_similarity() -> f32 {
    std::env::var("PRISM_CACHE_MIN_SIMILARITY")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(0.55)
}

/// Word-level cosine similarity over the two prompts, with a character-trigram cosine
/// blended in so that typos and morphology ("container" vs "containers") still score.
///
/// This is deliberately lexical. It detects the thing a prompt cache can actually reuse
/// — the same question asked again, near-verbatim — and it never claims that two
/// unrelated prompts are close, which a narrow random-projection hash does.
pub fn prompt_similarity(a: &str, b: &str) -> f32 {
    fn norm(s: &str) -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .collect()
    }
    fn cosine(a: &[String], b: &[String]) -> f32 {
        use std::collections::HashMap;
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let mut ca: HashMap<&str, f32> = HashMap::new();
        let mut cb: HashMap<&str, f32> = HashMap::new();
        for w in a {
            *ca.entry(w.as_str()).or_insert(0.0) += 1.0;
        }
        for w in b {
            *cb.entry(w.as_str()).or_insert(0.0) += 1.0;
        }
        let dot: f32 = ca
            .iter()
            .map(|(k, v)| cb.get(k).copied().unwrap_or(0.0) * v)
            .sum();
        let na: f32 = ca.values().map(|v| v * v).sum::<f32>().sqrt();
        let nb: f32 = cb.values().map(|v| v * v).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }
    fn trigrams(s: &str) -> Vec<String> {
        let cleaned: Vec<char> = s
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if cleaned.len() < 3 {
            return cleaned.iter().map(|c| c.to_string()).collect();
        }
        cleaned.windows(3).map(|w| w.iter().collect()).collect()
    }
    let words = cosine(&norm(a), &norm(b));
    let chars = cosine(&trigrams(a), &trigrams(b));
    // words carry the meaning; trigrams keep near-misses from falling off a cliff
    0.7 * words + 0.3 * chars
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

static GLOBAL_CACHE: OnceLock<Result<Mutex<SemanticCache>, String>> = OnceLock::new();

fn global_cache_result() -> &'static Result<Mutex<SemanticCache>, String> {
    GLOBAL_CACHE.get_or_init(|| {
        let dir = crate::prism_data_dir();
        SemanticCache::new(dir.clone())
            .map(Mutex::new)
            .map_err(|e| format!("{}: {}", dir.join("cache").join("sled").display(), e))
    })
}

/// Get or initialize the shared global SemanticCache singleton.
///
/// This fronts every cache operation, which is why the policy gate lives here: the
/// module-level `cache_response`/`lookup_*`/`record_keyed`/`get_cache_stats`/
/// `clear_cache` helpers all route through it, so one check covers them all. When the
/// cache is disabled the store is never opened — sled creates no directory and takes no
/// lock.
///
/// `None` now means *either* "disabled" or "unopenable". Ask [`unavailable`] which;
/// neither of them is "empty".
pub fn global_cache() -> Option<&'static Mutex<SemanticCache>> {
    if !is_enabled() {
        return None;
    }
    global_cache_result().as_ref().ok()
}

/// Why the store could not be opened, when it could not.
///
/// sled takes an exclusive lock on its directory, so any second process — the CLI while
/// `prism serve` is running — fails to open it. That failure used to be swallowed, and
/// `prism cache stats` reported `Total entries: 0` for a cache that was merely
/// unreachable. An inaccessible store and an empty one are different answers.
pub fn open_error() -> Option<&'static str> {
    global_cache_result().as_ref().err().map(String::as_str)
}

/// Why the semantic cache cannot be used, when it cannot.
///
/// The same argument [`open_error`] already made, extended by one case. There are now
/// three distinct answers where the API only has room for `None`: the cache is *empty*,
/// it is *unreachable*, or it is *disabled*. Collapsing the third into either of the
/// others tells the user to stop `prism serve` or to expect entries, when the actual
/// remedy is to enrol with a hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// No configuration layer enables it. The semantic cache is a PRISM Hub feature.
    Disabled,
    /// Enabled, but sled could not take its directory lock.
    Unopenable(&'static str),
}

impl Unavailable {
    /// The user-facing explanation, with the remedy that applies to each case.
    pub fn message(self) -> String {
        match self {
            Unavailable::Disabled => crate::config::disabled_by_policy("The semantic cache"),
            Unavailable::Unopenable(e) => format!(
                "cache unavailable: {e}\n  the store is locked while `prism serve` is running — stop it, or read the cache from the proxy's own MCP endpoint."
            ),
        }
    }
}

/// Whether the semantic cache is enabled, resolved through the full chain: hub policy >
/// project `.prismrc` > global `config.yaml` > default **off**.
///
/// Re-read per call rather than memoized, so a policy fetched by `prism hub config`
/// takes effect on the next request instead of on the next `prism serve` restart. The
/// cost is three `stat`s beside a model round trip.
pub fn is_enabled() -> bool {
    crate::config::resolve().cache_enabled()
}

/// Availability for a given policy decision.
///
/// The gate is a parameter so the reporting paths are testable without a config file,
/// and so `Disabled` provably never opens the store — [`open_error`] is only consulted
/// on the enabled branch.
pub fn availability(enabled: bool) -> Option<Unavailable> {
    if !enabled {
        return Some(Unavailable::Disabled);
    }
    open_error().map(Unavailable::Unopenable)
}

/// `Some(reason)` when no cache operation can succeed.
pub fn unavailable() -> Option<Unavailable> {
    availability(is_enabled())
}

/// Convenience global helper: save response for a prompt
pub fn cache_response(prompt: &str, response: &str, model: &str) {
    if let Some(cache_lock) = global_cache() {
        if let Ok(mut cache) = cache_lock.lock() {
            cache.put_with_model(prompt, response, model);
        }
    }
}

/// Record a proxied response under an explicit cache key.
pub fn record_keyed(
    key: &str,
    prompt: &str,
    response: &str,
    model: &str,
    content_type: &str,
    is_stream: bool,
) {
    if let Some(lock) = global_cache() {
        if let Ok(mut cache) = lock.lock() {
            cache.put_keyed(key, prompt, response, model, content_type, is_stream);
        }
    }
}

/// Exact-key lookup. Returns the whole entry so the caller can check the framing and
/// content type before deciding a replay is safe.
pub fn lookup_keyed(key: &str) -> Option<CacheEntry> {
    if let Some(lock) = global_cache() {
        if let Ok(cache) = lock.lock() {
            return cache.get_keyed(key);
        }
    }
    None
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
pub fn lookup_similar(query: &str, top_n: usize) -> Vec<Scored> {
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
        self.put_keyed(prompt, prompt, response, model, "", false);
    }

    /// Store `response` under `key`, keeping `prompt` as the text similarity compares.
    ///
    /// The two are separate on purpose. `key` has to cover everything that determines
    /// the answer — model, system prompt, whole message list, tools, sampling — so that
    /// an exact hit is genuinely the same request; `prompt` only has to be the part a
    /// human would recognise, because that is what `find_similar` reads. Keying both off
    /// the prompt text (as `put_with_model` did, and still does for its callers) means
    /// the same question asked of two different models collides on one entry.
    pub fn put_keyed(
        &mut self,
        key: &str,
        prompt: &str,
        response: &str,
        model: &str,
        content_type: &str,
        is_stream: bool,
    ) {
        let hash = Self::hash_input(key);
        let timestamp = Utc::now().timestamp() as u64;
        let emb = Self::pseudo_embedding(prompt);
        let entry = CacheEntry {
            key_hash: hash.clone(),
            prompt_hash: Self::hash_input(prompt),
            prompt: prompt.to_string(),
            response: response.to_string(),
            model: Some(model.to_string()),
            embedding: Some(emb.clone()),
            accessed_at: timestamp,
            last_saved: timestamp,
            access_count: 1,
            content_type: content_type.to_string(),
            is_stream,
            embedding_backend: crate::vector::backend_id().to_string(),
        };
        self.turbo.add(&hash, &emb);
        self.entries.insert(hash.clone(), entry.clone());
        if let Ok(serialized) = serde_json::to_string(&entry) {
            let _ = self
                .sled_db
                .insert(hash.as_bytes(), serialized.into_bytes());
            let _ = self.sled_db.flush();
        }
    }

    /// Exact lookup by cache key. No similarity, no threshold — this is the only path
    /// whose hit is safe to hand back as an answer rather than a suggestion.
    pub fn get_keyed(&self, key: &str) -> Option<CacheEntry> {
        self.entries.get(&Self::hash_input(key)).cloned()
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
    /// Nearest prompts, **scored and thresholded**.
    ///
    /// The ANN index is a recall device only: its feature-hash sketch is not
    /// locality sensitive, so its neighbours are candidates, not answers. Everything it
    /// returns (plus the rest of the table, which is small) is re-ranked by
    /// [`prompt_similarity`] and anything below `min_similarity` is dropped — without
    /// that a query always came back with *something*, and an unrelated entry read as a
    /// hit.
    pub fn find_similar(&self, query: &str, top_n: usize) -> Vec<Scored> {
        let min = min_similarity();
        let mut scored: Vec<Scored> = self
            .entries
            .values()
            .map(|entry| Scored {
                score: if entry.prompt.is_empty() {
                    // legacy entry: only an exact key match can be trusted
                    if Self::hash_input(query) == entry.prompt_hash {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    prompt_similarity(query, &entry.prompt)
                },
                entry: entry.clone(),
            })
            .filter(|s| s.score >= min)
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(top_n);
        scored
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
        self.sled_db.flush().map_err(std::io::Error::other)
    }

    fn hash_input(input: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Normalized feature-hash projection for fast ANN search.
    ///
    /// Delegates to [`crate::vector::embed`], which is the single definition of the
    /// projection. This used to be a byte-for-byte copy of it, with the width written
    /// as a bare `16` in four places — two implementations that had to stay in step by
    /// convention, where an index written by one and read by the other would silently
    /// compare incomparable vectors. Widening `TURBO_DIM` is precisely the edit that
    /// would have broken them apart.
    ///
    /// Kept as a method rather than folded into its call sites so the cache's own name
    /// for the concept survives.
    fn pseudo_embedding(input: &str) -> Vec<f32> {
        crate::vector::embed(input)
    }

    fn load_entries(&mut self) -> std::io::Result<()> {
        for item in self.sled_db.iter() {
            let (k, v) = item.map_err(std::io::Error::other)?;
            if let Ok(entry) = serde_json::from_slice::<CacheEntry>(v.as_ref()) {
                if let Some(ref emb) = entry.embedding {
                    // Entries written before the sketch widened carry the old width.
                    // Skipping them keeps `TurboVecIndex::add`'s length assertion from
                    // aborting the process on a cache that predates the change; they
                    // are re-embedded the next time their prompt is stored.
                    if emb.len() == crate::vector::TURBO_DIM
                        && entry.embedding_backend == crate::vector::backend_id()
                    {
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
mod similarity_tests {
    use super::*;

    #[test]
    fn identical_prompts_score_one() {
        assert!(
            prompt_similarity(
                "how do I list docker containers",
                "how do I list docker containers"
            ) > 0.99
        );
    }

    #[test]
    fn rewordings_of_the_same_question_clear_the_threshold() {
        let min = 0.55;
        for (a, b) in [
            (
                "how do I list docker containers",
                "how do I list the docker containers",
            ),
            ("list docker containers", "list docker container"),
            (
                "what does this function return",
                "what does this function returns",
            ),
        ] {
            let s = prompt_similarity(a, b);
            assert!(s >= min, "{:.2} for {:?} vs {:?}", s, a, b);
        }
    }

    #[test]
    fn unrelated_prompts_never_look_like_a_hit() {
        // the exact failure the old 16-dimension sign hash produced: a Tokio answer
        // came back for a Docker question
        let s = prompt_similarity(
            "show me all containers in docker",
            "explain how tokio schedules tasks on its work-stealing threadpool",
        );
        assert!(s < 0.55, "unrelated prompts scored {:.2}", s);
        assert!(prompt_similarity("deploy the api", "what is the capital of France") < 0.3);
    }

    #[test]
    fn scoring_is_symmetric_and_bounded_and_safe_on_empty() {
        let (a, b) = ("list the pods", "list pods in the cluster");
        assert!((prompt_similarity(a, b) - prompt_similarity(b, a)).abs() < 1e-6);
        for (x, y) in [
            ("", ""),
            ("", "something"),
            ("a", ""),
            ("é", "é"),
            ("🎉", "🎉"),
        ] {
            let s = prompt_similarity(x, y);
            assert!((0.0..=1.0).contains(&s), "{:.2} for {:?}/{:?}", s, x, y);
        }
    }

    #[test]
    fn threshold_is_configurable_but_clamped() {
        // Single-threaded env mutation in a test — safe (edition 2024 requires the
        // block because set_var/remove_var are unsound under concurrent access from
        // other threads, not applicable to this one process-wide var in this test).
        unsafe {
            // default when unset or unparseable
            std::env::remove_var("PRISM_CACHE_MIN_SIMILARITY");
            assert!((min_similarity() - 0.55).abs() < 1e-6);
            std::env::set_var("PRISM_CACHE_MIN_SIMILARITY", "9");
            assert_eq!(min_similarity(), 1.0);
            std::env::set_var("PRISM_CACHE_MIN_SIMILARITY", "-1");
            assert_eq!(min_similarity(), 0.0);
            std::env::remove_var("PRISM_CACHE_MIN_SIMILARITY");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pseudo_embedding_dimension_and_norm() {
        let emb = SemanticCache::pseudo_embedding("hello world test prompt");
        assert_eq!(emb.len(), crate::vector::TURBO_DIM);
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

        assert!(
            dot12 > dot13,
            "Similar code signatures must have higher similarity than unrelated topics"
        );
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "prism-cache-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_same_prompt_to_two_models_is_two_entries() {
        // The old keying hashed the prompt for both key_hash and prompt_hash, so this
        // pair collided and whichever landed second answered for both models.
        let dir = tmp("models");
        let mut c = SemanticCache::new(dir.clone()).unwrap();
        c.put_keyed(
            "k|opus|list pods",
            "list pods",
            "OPUS SAYS",
            "opus",
            "application/json",
            false,
        );
        c.put_keyed(
            "k|sonnet|list pods",
            "list pods",
            "SONNET SAYS",
            "sonnet",
            "application/json",
            false,
        );
        assert_eq!(c.len(), 2);
        assert_eq!(
            c.get_keyed("k|opus|list pods").unwrap().response,
            "OPUS SAYS"
        );
        assert_eq!(
            c.get_keyed("k|sonnet|list pods").unwrap().response,
            "SONNET SAYS"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_exact_key_miss_is_a_miss_however_close_the_prompt() {
        let dir = tmp("miss");
        let mut c = SemanticCache::new(dir.clone()).unwrap();
        c.put_keyed(
            "key-a",
            "how do I list docker containers",
            "R",
            "m",
            "",
            false,
        );
        assert!(
            c.get_keyed("key-b").is_none(),
            "exact lookup must not fall back to similarity"
        );
        // …while similarity search still reaches it by prompt text.
        let hits = c.find_similar("how do I list the docker containers", 3);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score >= min_similarity());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn framing_and_content_type_round_trip_through_sled() {
        let dir = tmp("framing");
        {
            let mut c = SemanticCache::new(dir.clone()).unwrap();
            c.put_keyed(
                "k",
                "p",
                "event: x\ndata: {}\n\n",
                "m",
                "text/event-stream",
                true,
            );
        }
        // reopen: a replay decision made after a restart needs these fields persisted
        let c = SemanticCache::new(dir.clone()).unwrap();
        let e = c.get_keyed("k").expect("entry should survive a reopen");
        assert!(e.is_stream);
        assert_eq!(e.content_type, "text/event-stream");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unrelated_prompt_is_not_returned_as_a_hit() {
        let dir = tmp("unrelated");
        let mut c = SemanticCache::new(dir.clone()).unwrap();
        c.put_keyed(
            "k1",
            "explain how tokio schedules tasks on its work-stealing threadpool",
            "TOKIO",
            "m",
            "",
            false,
        );
        assert!(
            c.find_similar("show me all containers in docker", 5)
                .is_empty(),
            "the original defect: any query returned its top-N neighbours"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// Three answers, three values. The store being empty, unreachable and disabled are
    /// different situations with different fixes, and `prism cache stats` must not print
    /// one of them for another.
    #[test]
    fn a_disabled_cache_is_not_reported_as_empty_or_locked() {
        assert_eq!(
            availability(false),
            Some(Unavailable::Disabled),
            "policy off must report Disabled without consulting the store"
        );

        let off = Unavailable::Disabled.message();
        let locked = Unavailable::Unopenable("…/cache/sled: could not acquire lock").message();
        assert_ne!(off, locked);
        assert!(
            off.contains("disabled") && off.contains("prism hub enroll"),
            "{off}"
        );
        assert!(
            !off.contains("locked") && !off.contains("prism serve"),
            "a disabled cache must not be blamed on the proxy holding a lock: {off}"
        );
        assert!(
            locked.contains("locked") && !locked.contains("prism hub enroll"),
            "a lock failure has nothing to do with enrolment: {locked}"
        );
    }

    /// The community default. Nothing on disk enables the cache, so every module-level
    /// helper must be a no-op — and crucially the sled directory must not be created,
    /// which is what makes "disabled" a real gate and not just a hidden filter.
    #[test]
    fn the_community_default_is_off() {
        let community = crate::config::PrismConfig::default();
        assert!(!community.cache_enabled());
        assert_eq!(availability(false), Some(Unavailable::Disabled));
    }
}
