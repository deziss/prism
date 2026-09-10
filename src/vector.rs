// PRISM vector.rs — TurboVec IdMapIndex for fast ANN search
// TurboQuant: Google-quality vector compression, 10M docs/4GB, AVX-512BW SIMD, no training needed.
// dim=16 matches cache pseudo-embeddings; bit_width=2 = 4x compression.

use std::collections::HashMap;
use turbovec::IdMapIndex;

pub const TURBO_DIM: usize = 16;
const BIT_WIDTH: usize = 2;

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
