// PRISM vector.rs — TurboVec IdMapIndex for fast ANN search
// TurboQuant: Google-quality vector compression, 10M docs/4GB, AVX-512BW SIMD, no training needed.
// dim=16 matches cache pseudo-embeddings; bit_width=2 = 4x compression.

use std::collections::HashMap;
use turbovec::IdMapIndex;

pub const TURBO_DIM: usize = 256;
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
/// Where an installed static-embedding model lives.
///
/// A directory, not a download: prism does not fetch models at runtime, which is why
/// `model2vec-rs` is built with `local-only` and its `hf-hub`/`ureq` features off. The
/// operator puts a model here (`model.safetensors` + `tokenizer.json`) and prism uses
/// it; with nothing there, the feature-hash sketch runs exactly as before.
pub fn model_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("PRISM_EMBEDDING_MODEL") {
        return std::path::PathBuf::from(p);
    }
    crate::prism_data_dir().join("models").join("static")
}

/// The loaded model, or `None` when none is installed.
///
/// Loaded once. A failure to load is cached as `None` rather than retried: this sits on
/// the memory-search path, and re-attempting a broken model on every query would turn a
/// misconfiguration into a performance problem on top of a functional one. The reason is
/// logged once so it is diagnosable.
fn model() -> Option<&'static model2vec_rs::model::StaticModel> {
    static MODEL: std::sync::OnceLock<Option<model2vec_rs::model::StaticModel>> =
        std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| {
            let dir = model_dir();
            if !dir.join("model.safetensors").exists() {
                return None;
            }
            match model2vec_rs::model::StaticModel::from_pretrained(&dir, None, None, None) {
                Ok(m) => {
                    tracing::info!("loaded static embedding model from {}", dir.display());
                    Some(m)
                }
                Err(e) => {
                    tracing::warn!(
                        "static embedding model at {} failed to load: {e}",
                        dir.display()
                    );
                    None
                }
            }
        })
        .as_ref()
}

/// Whether a real embedding model is in use, for `prism status` and the docs to report.
pub fn model_active() -> bool {
    model().is_some()
}

/// Which projection [`embed`] is currently using.
///
/// Derived indexes must key off this. Both backends emit `TURBO_DIM`-wide vectors, so a
/// width check — which is what guarded the 16→256 widening — cannot detect a backend
/// switch, and a sketch vector compared against a model vector is not wrong-looking, it
/// is just quietly meaningless. Installing or removing a model has to invalidate every
/// stored embedding, and this is the value that makes that happen.
pub fn backend_id() -> &'static str {
    if model_active() {
        "static-model"
    } else {
        "sketch"
    }
}

/// Project `text` into a unit vector, using the installed model when there is one.
///
/// Two backends behind one function, because every caller — memory search, the semantic
/// cache, the ANN index — has to agree on the projection or an index written by one is
/// read as nonsense by another. Switching backends therefore invalidates persisted
/// vectors, which is safe here only because both the cache and the memory palace check
/// the stored width and re-embed on a mismatch.
///
/// The model's own dimensionality is resized to [`TURBO_DIM`] so the index width is a
/// compile-time constant: truncation keeps the leading components, which for a static
/// embedding carry the most variance, and short vectors are zero-padded.
pub fn embed(text: &str) -> Vec<f32> {
    if let Some(m) = model() {
        // `encode` returns one vector per input.
        if let Some(v) = m.encode(&[text.to_string()]).into_iter().next() {
            return fit_to_dim(v);
        }
    }
    embed_sketch(text)
}

/// Resize a model vector to `TURBO_DIM` and normalise it.
fn fit_to_dim(mut v: Vec<f32>) -> Vec<f32> {
    v.resize(TURBO_DIM, 0.0);
    let norm = v.iter().map(|e| e * e).sum::<f32>().sqrt().max(0.001);
    v.iter_mut().for_each(|e| *e /= norm);
    v
}

pub fn embed_sketch(text: &str) -> Vec<f32> {
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
    // ─── separation calibration ──────────────────────────────────────────────

    /// Deterministic corpus of memory-block-shaped sentences.
    ///
    /// Generated rather than hand-written so the sample is large enough for tail
    /// percentiles to mean something — p99 over 40 pairs is noise.
    fn corpus() -> Vec<String> {
        const SUBJECTS: &[&str] = &[
            "kubectl",
            "docker",
            "postgres",
            "redis",
            "nginx",
            "systemd",
            "cargo",
            "webpack",
            "terraform",
            "ansible",
            "prometheus",
            "grafana",
            "kafka",
            "rabbitmq",
            "elasticsearch",
            "vault",
            "consul",
            "etcd",
            "helm",
            "argocd",
        ];
        const VERBS: &[&str] = &[
            "restarts",
            "fails to start",
            "times out",
            "leaks memory",
            "drops connections",
            "rejects the token",
            "corrupts the index",
            "blocks on startup",
            "loses the lease",
            "rotates the certificate",
        ];
        const OBJECTS: &[&str] = &[
            "after a node reboot",
            "under load",
            "when the disk fills",
            "on a cold cache",
            "behind the proxy",
            "during a rolling update",
            "with a stale config",
            "on the staging cluster",
            "after the upgrade",
            "when DNS is slow",
        ];
        let mut out = Vec::new();
        for s in SUBJECTS {
            for v in VERBS {
                for o in OBJECTS {
                    out.push(format!("{s} {v} {o}"));
                }
            }
        }
        out
    }

    fn percentile(sorted: &[f32], p: f64) -> f32 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[idx]
    }

    /// A reworded version of the same text: same subject and object, different verb
    /// phrasing and word order. This is what a user typing a half-remembered query
    /// looks like.
    fn reword(s: &str) -> String {
        let parts: Vec<&str> = s.splitn(2, ' ').collect();
        format!(
            "{} issue: {}",
            parts[0],
            parts.get(1).copied().unwrap_or("")
        )
    }

    /// The measurement the module doc quotes, run as a test so the numbers cannot go
    /// stale and the sketch cannot silently regress.
    ///
    /// Calls `embed_sketch` rather than `embed`: this measures the *sketch*, and must
    /// keep doing so on a machine where a real model happens to be installed.
    ///
    /// At `TURBO_DIM = 16` the two distributions overlapped completely — unrelated p99
    /// 0.85 against paraphrase scores of 0.38–0.59 — which is why the sketch was
    /// documented as able to rank but not retrieve. The assertions below encode the
    /// separation the current width actually achieves.
    #[test]
    fn sketch_separates_paraphrases_from_unrelated_text() {
        let texts = corpus();

        let mut unrelated: Vec<f32> = Vec::new();
        // Deterministic stride sampling: every pair is far apart in the corpus, so no
        // two share a subject, verb or object.
        for i in (0..texts.len()).step_by(7) {
            for j in (i + 313..texts.len()).step_by(311) {
                unrelated.push(cosine(&embed_sketch(&texts[i]), &embed_sketch(&texts[j])));
            }
        }
        let mut paraphrase: Vec<f32> = texts
            .iter()
            .step_by(11)
            .map(|t| cosine(&embed_sketch(t), &embed_sketch(&reword(t))))
            .collect();

        unrelated.sort_by(f32::total_cmp);
        paraphrase.sort_by(f32::total_cmp);

        let u_p50 = percentile(&unrelated, 0.50);
        let u_p90 = percentile(&unrelated, 0.90);
        let u_p99 = percentile(&unrelated, 0.99);
        let p_p10 = percentile(&paraphrase, 0.10);
        let p_p50 = percentile(&paraphrase, 0.50);

        println!(
            "TURBO_DIM={TURBO_DIM}  unrelated n={} p50={u_p50:.3} p90={u_p90:.3} p99={u_p99:.3}  \
             paraphrase n={} p10={p_p10:.3} p50={p_p50:.3}",
            unrelated.len(),
            paraphrase.len()
        );

        // The property that matters for retrieval: the unrelated tail must sit below
        // the paraphrase body, or no threshold can separate them.
        assert!(
            u_p99 < p_p10,
            "unrelated p99 ({u_p99:.3}) must fall below paraphrase p10 ({p_p10:.3}) — \
             overlapping tails mean the sketch can rank but not retrieve"
        );
        // The tail is what a retrieval threshold has to clear. At TURBO_DIM=16 this
        // measured 0.664 on the same corpus; at 256 it is ~0.41. Narrowing the sketch
        // again should fail here rather than quietly degrade recall.
        assert!(
            u_p99 < 0.50,
            "unrelated p99 ({u_p99:.3}) is too high for any usable threshold"
        );
        // Not asserted tightly: these sentences share vocabulary by construction
        // ("fails to start", "under load"), so the median is real signal, not noise.
        assert!(u_p50 < 0.30, "median unrelated similarity: {u_p50:.3}");
    }

    /// The sketch's ceiling, pinned so it is a known limitation rather than a surprise.
    ///
    /// Feature hashing has **no semantics**: `k8s` and `kubectl` hash to unrelated
    /// dimensions, so a paraphrase that substitutes synonyms scores like unrelated
    /// text no matter how wide the sketch gets. Widening cuts collision noise, which is
    /// why the tail dropped from 0.66 to 0.41 — it cannot add meaning that was never
    /// encoded.
    ///
    /// This is why `MemoryPalace::search` leads with BM25 and uses the sketch only to
    /// re-rank. Installing a static embedding model in `vector::model_dir()` is what
    /// closes it — `embed` then routes through the model and these numbers no longer
    /// describe what retrieval uses. The test stays pinned to `embed_sketch` so it
    /// keeps describing the fallback honestly.
    #[test]
    fn the_sketch_cannot_match_synonyms_and_that_is_documented() {
        let pairs = [
            ("kubectl context switching", "k8s namespace selection"),
            (
                "postgres connection pool exhausted",
                "pg client limit reached",
            ),
            (
                "container image pull failure",
                "docker registry fetch error",
            ),
        ];

        let scores: Vec<f32> = pairs
            .iter()
            .map(|(a, b)| cosine(&embed_sketch(a), &embed_sketch(b)))
            .collect();
        println!("synonym-pair cosines at TURBO_DIM={TURBO_DIM}: {scores:?}");

        // Every one of them sits in the noise band, not the paraphrase band (>0.9).
        for (score, (a, b)) in scores.iter().zip(pairs.iter()) {
            assert!(
                *score < 0.60,
                "{a:?} vs {b:?} scored {score:.3} — if this now succeeds, a real \
                 embedding model has landed and this test should be replaced"
            );
        }
    }
    // ─── model backend ───────────────────────────────────────────────────────

    #[test]
    fn embed_falls_back_to_the_sketch_when_no_model_is_installed() {
        // SAFETY: single-threaded test body, restored immediately.
        unsafe { std::env::set_var("PRISM_EMBEDDING_MODEL", "/nonexistent/prism-model") };
        let out = embed("kubectl context switching");
        unsafe { std::env::remove_var("PRISM_EMBEDDING_MODEL") };

        assert_eq!(out.len(), TURBO_DIM);
        assert!(out.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn every_backend_produces_a_turbo_dim_unit_vector() {
        for text in [
            "",
            "one",
            "a much longer sentence about kubernetes and postgres",
        ] {
            let v = embed(text);
            assert_eq!(v.len(), TURBO_DIM, "input {text:?}");
            assert!(v.iter().all(|x| x.is_finite()), "input {text:?}");
        }
    }

    /// A model of any width has to land on `TURBO_DIM`, because the index width is a
    /// compile-time constant and a mismatched vector aborts `TurboVecIndex::add`.
    #[test]
    fn a_model_vector_is_resized_and_normalised() {
        let wide = fit_to_dim(vec![1.0; TURBO_DIM * 2]);
        assert_eq!(wide.len(), TURBO_DIM);

        let narrow = fit_to_dim(vec![1.0, 2.0, 3.0]);
        assert_eq!(narrow.len(), TURBO_DIM);

        let norm: f32 = narrow.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "expected a unit vector, got {norm}"
        );
    }

    #[test]
    fn resizing_an_all_zero_vector_does_not_divide_by_zero() {
        let v = fit_to_dim(vec![0.0; 8]);

        assert_eq!(v.len(), TURBO_DIM);
        assert!(v.iter().all(|x| x.is_finite()));
    }

    /// Compare both backends on the same corpus, so the choice is evidence-based.
    ///
    /// Skipped when no model is installed: a model is an operator-installed asset and a
    /// clean checkout has none, so failing here would be noise rather than signal.
    #[test]
    fn measure_model_against_sketch() {
        if !model_active() {
            eprintln!("skipped: no model at {}", model_dir().display());
            return;
        }
        let texts = corpus();

        let stats = |f: &dyn Fn(&str) -> Vec<f32>, name: &str| {
            let mut unrelated: Vec<f32> = Vec::new();
            for i in (0..texts.len()).step_by(7) {
                for j in (i + 313..texts.len()).step_by(311) {
                    unrelated.push(cosine(&f(&texts[i]), &f(&texts[j])));
                }
            }
            let mut para: Vec<f32> = texts
                .iter()
                .step_by(11)
                .map(|t| cosine(&f(t), &f(&reword(t))))
                .collect();
            unrelated.sort_by(f32::total_cmp);
            para.sort_by(f32::total_cmp);
            let (u99, p10) = (percentile(&unrelated, 0.99), percentile(&para, 0.10));
            println!(
                "{name:8} unrelated p50={:.3} p90={:.3} p99={u99:.3} | paraphrase p10={p10:.3} | margin={:.3}",
                percentile(&unrelated, 0.50),
                percentile(&unrelated, 0.90),
                p10 - u99
            );
            p10 - u99
        };

        let sketch_margin = stats(&|t| embed_sketch(t), "sketch");
        let model_margin = stats(&|t| embed(t), "model");

        let syn = [
            ("kubectl context switching", "k8s namespace selection"),
            (
                "postgres connection pool exhausted",
                "pg client limit reached",
            ),
            (
                "container image pull failure",
                "docker registry fetch error",
            ),
        ];
        for (a, b) in syn {
            println!(
                "synonym {a:?} vs {b:?}: sketch {:.3} model {:.3}",
                cosine(&embed_sketch(a), &embed_sketch(b)),
                cosine(&embed(a), &embed(b))
            );
        }
        println!("margin: sketch {sketch_margin:.3} model {model_margin:.3}");

        // Asserted on the separation margin, not on any one synonym pair. The model
        // beats the sketch on `postgres`/`pg` (0.068 -> 0.352) and
        // `container image`/`docker registry` (0.066 -> 0.293) but loses on
        // `kubectl`/`k8s` (0.244 -> 0.210), where `k8s` is a token the distillation
        // barely saw. A per-pair assertion would encode that quirk as a requirement.
        assert!(
            model_margin >= sketch_margin,
            "installing a model must not narrow the separation: {model_margin:.3} vs {sketch_margin:.3}"
        );
    }

    #[test]
    fn the_backend_id_tracks_whether_a_model_is_loaded() {
        let id = backend_id();
        assert!(
            id == "sketch" || id == "static-model",
            "unexpected backend id {id:?}"
        );
        assert_eq!(id == "static-model", model_active());
    }
}
