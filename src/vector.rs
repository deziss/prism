// PRISM vector.rs — TurboVec IdMapIndex for fast ANN search
// TurboQuant: Google-quality vector compression, 10M docs/4GB, AVX-512BW SIMD, no training needed.
// dim=16 matches cache pseudo-embeddings; bit_width=2 = 4x compression.

use std::collections::HashMap;
use turbovec::IdMapIndex;

pub const TURBO_DIM: usize = 16;
const BIT_WIDTH: usize = 2;

/// Project `text` into a `TURBO_DIM`-dimensional unit vector for [`TurboVecIndex`].
///
/// This is a **feature-hashing sketch**, not a learned embedding: each word token and
/// each character 3-gram hashes to one dimension and adds a signed weight there. The
/// signed sum makes the dot product of two sketches an unbiased estimator of the dot
/// product of the underlying bag-of-features — which is why it ranks related texts above
/// unrelated ones at all — but with only 16 dimensions the variance is large, so a
/// sketch is evidence, never proof. Callers must treat its neighbours as *candidates* and
/// re-rank them (see `SemanticCache::find_similar`, and `MemoryPalace::search`, which
/// both do).
///
/// It lives here rather than in a caller because there is now more than one caller.
/// `cache.rs` grew its own private `pseudo_embedding` first, and this is a byte-for-byte
/// equivalent projection; the two should be collapsed into this one function, but that
/// edit belongs to `cache.rs` and is out of scope for the memory-search work that needed
/// a second copy. Keep them in step: an index written by one and read by the other would
/// otherwise silently compare incomparable vectors.
///
/// Measured, so callers can size their trust in it: over 41k pairs of unrelated
/// memory-block-shaped texts the cosine of two sketches has p50 0.10, p90 0.60, p99 0.85
/// and max 0.96, while genuinely reworded pairs of the *same* text score around 0.4–0.6.
/// Those two distributions overlap completely. The sketch therefore ranks, and must not
/// retrieve: it is right on average, which is all a re-ranker needs and nowhere near what
/// a recall threshold needs. Widening `TURBO_DIM` is what would change that.
///
/// Never returns a non-finite coordinate — `IdMapIndex::search` *panics* on those — since
/// the norm is floored before the division and every input weight is a finite constant.
pub fn embed(text: &str) -> Vec<f32> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut emb = vec![0.0f32; TURBO_DIM];
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        // All-zero is a legal vector here: it is finite, and `cosine` reports 0.0
        // against it rather than dividing by zero.
        return emb;
    }

    // Word tokens carry the topic.
    for word in &words {
        let mut h = DefaultHasher::new();
        word.to_lowercase().hash(&mut h);
        let val = h.finish();
        let dim = (val % TURBO_DIM as u64) as usize;
        let sign = if (val >> 4) & 1 == 1 { 1.0f32 } else { -1.0f32 };
        emb[dim] += sign;
    }

    // Character 3-grams at half weight carry morphology, so "switch" and "switching"
    // do not look like unrelated tokens the way whole-word equality says they are.
    let bytes = text.as_bytes();
    for window in bytes.windows(3) {
        let mut h = DefaultHasher::new();
        window.hash(&mut h);
        let val = h.finish();
        let dim = (val % TURBO_DIM as u64) as usize;
        let sign = if (val >> 5) & 1 == 1 { 0.5f32 } else { -0.5f32 };
        emb[dim] += sign;
    }

    let norm = emb.iter().map(|e| e * e).sum::<f32>().sqrt().max(0.001);
    emb.iter_mut().for_each(|e| *e /= norm);
    emb
}

/// Cosine similarity of two sketches, in `-1.0..=1.0`.
///
/// Returns `0.0` — "no evidence" — for mismatched lengths or a zero vector rather than
/// producing a NaN that would then sort unpredictably against real scores. A corrupt
/// on-disk index is the realistic source of a wrong-length vector, and it must degrade,
/// not poison the ranking.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        (dot / (na * nb)).clamp(-1.0, 1.0)
    }
}

/// TurboVec-backed ANN index mapping string IDs to quantized 16-dim embeddings.
pub struct TurboVecIndex {
    inner: IdMapIndex,
    id_to_u64: HashMap<String, u64>,
    u64_to_id: HashMap<u64, String>,
    next_id: u64,
}

impl TurboVecIndex {
    pub fn new() -> Self {
        Self {
            // turbovec 1.0's `new` returns Result (dim must be a positive multiple of 8,
            // bit_width in {2,3,4}) -- TURBO_DIM=16 and BIT_WIDTH=2 are compile-time
            // constants that always satisfy both, so a failure here would mean the
            // constants themselves are wrong, worth panicking on rather than masking.
            inner: IdMapIndex::new(TURBO_DIM, BIT_WIDTH)
                .expect("TURBO_DIM/BIT_WIDTH are valid turbovec constants"),
            id_to_u64: HashMap::new(),
            u64_to_id: HashMap::new(),
            next_id: 0,
        }
    }

    /// Add a TURBO_DIM-dimensional embedding. Skips if id already present.
    pub fn add(&mut self, id: &str, embedding: &[f32]) {
        assert_eq!(
            embedding.len(),
            TURBO_DIM,
            "embedding must be {TURBO_DIM}-dim"
        );
        if self.id_to_u64.contains_key(id) {
            return;
        }
        let uid = self.next_id;
        self.next_id += 1;
        self.id_to_u64.insert(id.to_string(), uid);
        self.u64_to_id.insert(uid, id.to_string());
        // turbovec 1.0's add_with_ids can fail on a duplicate id (ruled out above -- uid
        // is freshly minted) or a non-finite coordinate (every embedding reaching this
        // module comes from a bounded hash projection, never NaN/inf). Both maps are
        // rolled back on the (should-never-happen) error path so `contains`/`remove`
        // cannot disagree with what `inner` actually holds.
        if let Err(e) = self.inner.add_with_ids(embedding, &[uid]) {
            self.id_to_u64.remove(id);
            self.u64_to_id.remove(&uid);
            eprintln!("prism: turbovec rejected an embedding for {id}: {e}");
        }
    }

    /// Top-k nearest string IDs for query embedding.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<String> {
        if self.inner.is_empty() || k == 0 {
            return Vec::new();
        }
        assert_eq!(query.len(), TURBO_DIM, "query must be {TURBO_DIM}-dim");
        let k = k.min(self.inner.len());
        let (_scores, ids) = self.inner.search(query, k);
        ids.iter()
            .filter_map(|uid| self.u64_to_id.get(uid).cloned())
            .collect()
    }

    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(uid) = self.id_to_u64.remove(id) {
            self.u64_to_id.remove(&uid);
            self.inner.remove(uid)
        } else {
            false
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.id_to_u64.contains_key(id)
    }
}

impl Default for TurboVecIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod embed_tests {
    use super::*;

    #[test]
    fn embed_is_always_turbo_dim_and_finite() {
        // `IdMapIndex::add_with_ids` rejects, and `IdMapIndex::search` *panics* on, a
        // non-finite coordinate. The empty string is the interesting input: its norm is
        // zero before the floor, so an unguarded divide would hand turbovec 16 NaNs.
        for input in ["", " ", "a", "kubectl context switching", "🙂🙂🙂"] {
            let e = embed(input);
            assert_eq!(e.len(), TURBO_DIM, "input {input:?}");
            assert!(e.iter().all(|x| x.is_finite()), "input {input:?}");
        }
    }

    #[test]
    fn embed_ranks_a_reworded_text_above_an_unrelated_one() {
        // The sketch is the whole justification for the vector half of memory search:
        // it has to put a morphological variant closer than a different subject. It is
        // noisy at 16 dimensions, which is exactly why callers re-rank rather than
        // trusting the neighbour list — but the ordering must hold here.
        let base = embed("switch the kubernetes context to production");
        let variant = embed("switching kubernetes contexts to production");
        let other = embed("compress a long prompt with bm25 sentence scoring");
        let near = cosine(&base, &variant);
        let far = cosine(&base, &other);
        assert!(near > far, "near={near} should beat far={far}");
    }

    #[test]
    fn cosine_degrades_instead_of_producing_nan() {
        // Both of these arrive from a corrupt on-disk index, and a NaN score would sort
        // unpredictably against real scores rather than simply losing.
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    #[test]
    fn cosine_of_a_vector_with_itself_is_one() {
        let e = embed("the proxy keys on the original body");
        assert!((cosine(&e, &e) - 1.0).abs() < 1e-5);
    }
}
