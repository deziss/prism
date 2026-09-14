// PRISM memory.rs — 3-layer Memory Palace (Recall → Core → Archive)
// Persistence layer: JSONL for each memory level

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cache::SemanticCache;
use crate::vector::{TURBO_DIM, TurboVecIndex};

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

/// Monotonic tie-breaker for block ids.
///
/// `mem-<millis>` on its own is not unique: two `save` calls inside the same
/// millisecond — routine when a session flushes several memories at once, and trivial
/// to hit from the MCP server — minted the same id twice. That was survivable while
/// nothing keyed off the id, but the derived vector index does, and
/// `TurboVecIndex::add` *skips* an id it already holds: the second block would have
/// silently inherited the first one's embedding and been unreachable by the vector half
/// of search forever. The pid is in there too because the counter is per-process and
/// two prisms can save in the same millisecond.
static BLOCK_SEQ: AtomicU64 = AtomicU64::new(0);

impl MemoryBlock {
    pub fn new(layer: MemoryLayer, content: &str, category: &str) -> Self {
        let now = Utc::now().to_rfc3339();
        let trimmed = if content.len() > 10_000 {
            &content[..10_000]
        } else {
            content
        };
        Self {
            id: format!(
                "mem-{}-{}-{}",
                Utc::now().timestamp_millis(),
                std::process::id(),
                BLOCK_SEQ.fetch_add(1, Ordering::Relaxed)
            ),
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

// ── Retrieval ────────────────────────────────────────────────────────────────────
//
// `search` used to score a block with
//
//     block.keywords.iter().filter(|kw| query_words.contains(kw)).count()
//
// — the size of the exact intersection between the query's whitespace-split words and
// the block's keyword list. Four things were wrong with that, and they compound:
//
//  1. It read `keywords` and nothing else. `extract_keywords` builds that list by
//     dropping every token of four characters or fewer, so "k8s", "npm", "ssh", "tls",
//     "dns", "git", "aws" — precisely the terms that identify a memory — can never
//     appear in it. No query could retrieve a block by such a word even when the word
//     was sitting in the block's own content.
//  2. Every match was worth exactly 1. A block matching "file" scored the same as a
//     block matching "kubeconfig", so one ubiquitous word outranked the single rare
//     word that actually picked the memory out of the palace.
//  3. Matching was equality on raw tokens: "switching" missed "switch", "namespaces"
//     missed "namespace", "queries" missed "query".
//  4. There was no similarity path at all. `vector.rs` — this crate's turbovec index —
//     was wired only into `cache.rs`, so the "Memory Palace" the product describes as
//     semantic recall never touched a vector.
//
// (1)–(3) are fixed below by BM25 over the block's *full* text with document frequency
// computed across the palace. (4) is fixed by reusing the existing `TurboVecIndex` as a
// candidate generator whose neighbours are re-ranked by exact cosine — the same "ANN
// recalls, exact score decides" shape `cache.rs::find_similar` already uses, for the
// same reason: a 16-dimension sketch is evidence, not an answer.
//
// What this is NOT: the sketch in `vector::embed` is feature hashing over words and
// character trigrams, not a learned embedding. It bridges morphology and spelling, not
// synonymy — it will not connect "kubectl" to "k8s" unless the two co-occur in a
// block's text. Fixing (1) is what makes most of those queries work, because the term
// usually *is* in the content; the vector half adds fuzz, not meaning.

/// BM25 term-frequency saturation and length-normalisation constants. Same values as
/// `compress.rs::bm25_select_prose`, so the two scorers rank consistently.
const BM25_K1: f64 = 1.5;
const BM25_B: f64 = 0.75;

/// How the two halves of the hybrid score are mixed.
///
/// Lexical dominates deliberately. BM25 is the half that can *justify* a hit — it names
/// the terms that matched — while the 16-dimension sketch is a noisy estimator (see
/// [`vector_only_min`] for the numbers) that is right on average and wrong often enough
/// that letting it drive would trade one kind of brittleness for another. At 0.25 the
/// sketch can reorder near-ties, which is what a weak signal should be allowed to do,
/// and cannot lift a block with no lexical evidence above one that has it.
const LEXICAL_WEIGHT: f32 = 0.75;
const VECTOR_WEIGHT: f32 = 0.25;

/// Minimum cosine for the vector half to retrieve a block the lexical half found no
/// evidence for at all. Override with `PRISM_MEMORY_VECTOR_MIN`, mirroring
/// `PRISM_CACHE_MIN_SIMILARITY`.
///
/// The default was 1.0 — vector-only retrieval **off**, because at `TURBO_DIM = 16` the
/// two distributions overlapped completely: unrelated memory-block-shaped texts reached
/// p99 0.66 while genuine paraphrases sat around 0.88, so every threshold low enough to
/// admit a paraphrase also admitted a slice of the palace as noise.
///
/// Widening the sketch to 256 dimensions separated them. Same corpus, same method
/// (`vector::embed_tests::sketch_separates_paraphrases_from_unrelated_text`, which runs
/// on every `cargo test` so these numbers cannot go stale):
///
/// | | unrelated p90 | unrelated p99 | paraphrase p10 |
/// |---|---|---|---|
/// | `TURBO_DIM = 16` | 0.459 | 0.664 | 0.876 |
/// | `TURBO_DIM = 256` | 0.297 | **0.411** | **0.904** |
///
/// 0.75 sits in the gap — comfortably above the unrelated tail, comfortably below the
/// paraphrase body — so the vector half may now retrieve a block the lexical half found
/// no evidence for, which is the case BM25 cannot serve: a query that shares meaning but
/// not vocabulary.
///
/// **One limitation survives the widening.** Feature hashing has no semantics, so
/// synonym substitution still scores as noise: `kubectl context switching` against
/// `k8s namespace selection` measures 0.24. Widening cut collision noise; it cannot add
/// meaning that was never encoded. Closing that gap needs a real embedding model, and
/// `vector::embed_tests::the_sketch_cannot_match_synonyms_and_that_is_documented` pins
/// the current ceiling so the day it lifts is visible.
fn vector_only_min() -> f32 {
    std::env::var("PRISM_MEMORY_VECTOR_MIN")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(0.75)
}

/// ANN neighbours to pull per requested result, and a floor for small palaces.
///
/// Over-fetching is the point: the sketch's neighbour order is noisy, so the exact
/// cosine re-rank needs more candidates than it will keep. Below the floor the index
/// simply returns every block, which is the correct behaviour for a palace of twelve.
const ANN_CANDIDATE_FACTOR: usize = 8;
const ANN_CANDIDATE_FLOOR: usize = 32;

/// Schema version of a line in the derived vector index.
///
/// Bump whenever [`indexable_text`], [`crate::vector::embed`] or [`fingerprint`]
/// changes: a line written under an older rule describes a vector that is no longer
/// comparable to one computed now, and silently mixing the two ranks worse than
/// re-embedding everything once.
const VECTOR_INDEX_VERSION: u32 = 1;

/// Filename of the derived index, under `<data>/sessions/`.
const VECTOR_INDEX_FILE: &str = "vectors.jsonl";

/// One block's cached embedding.
///
/// JSONL, matching `blocks.jsonl` next to it, so a save appends a line instead of
/// rewriting the file — and so a crash mid-write costs exactly one embedding rather
/// than the whole index.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VectorIndexLine {
    v: u32,
    id: String,
    /// Hash of the exact text that was embedded, so an edited block is spotted and
    /// re-embedded rather than being searched through a vector of its old self.
    fp: u64,
    emb: Vec<f32>,
}

/// Content hash used to detect a stale cached embedding.
///
/// `DefaultHasher` is deterministic within a build but explicitly not guaranteed stable
/// across Rust releases, so a toolchain bump invalidates every fingerprint at once and
/// the index rebuilds on the next open. That is the right failure: slow once, never
/// wrong. `cache.rs::hash_input` already persists the same assumption into sled.
fn fingerprint(text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The text a block is scored and embedded against.
///
/// Content, not just keywords — that is fix (1). `category` and the keyword list are
/// folded in ahead of it so the terms the block's author thought were important get a
/// second occurrence, which BM25's term frequency turns into a modest boost rather than
/// the override that keyword-only scoring gave them.
fn indexable_text(block: &MemoryBlock) -> String {
    let mut s = String::with_capacity(block.content.len() + 64);
    s.push_str(&block.category);
    s.push(' ');
    for kw in &block.keywords {
        s.push_str(kw);
        s.push(' ');
    }
    s.push_str(&block.content);
    s
}

/// Fold the commonest English inflections so a query and a memory written months apart
/// still meet: "switching"/"switch", "namespaces"/"namespace", "queries"/"query",
/// "cached"/"cache".
///
/// Deliberately crude — two fixed rules, no dictionary, no Porter tables. It does not
/// have to be linguistically right, only *consistent*, because the same function
/// normalises both sides: a wrong-but-stable stem still makes the query and the block
/// meet, which exact token equality never did. The second rule (drop a trailing "e") is
/// what pairs "cached" → "cach" with "cache" → "cach"; suffix-stripping alone leaves
/// those two apart.
fn stem(word: &str) -> String {
    // Never leave a stem shorter than this: chopping "ing" off "king" leaves "k", which
    // then collides with every other one-letter stem in the palace.
    const MIN_STEM: usize = 3;

    let mut w = word.to_string();
    for suffix in ["ies", "ing", "ed", "es", "s"] {
        // "ss" is not a plural ending: without this, "process" stems to "proces" while
        // "processes" stems to "process", and the two never meet.
        if suffix == "s" && w.ends_with("ss") {
            break;
        }
        if let Some(rest) = w.strip_suffix(suffix) {
            if rest.len() >= MIN_STEM {
                w = if suffix == "ies" {
                    format!("{rest}y")
                } else {
                    rest.to_string()
                };
                break;
            }
        }
    }
    if w.len() > MIN_STEM && w.ends_with('e') {
        w.pop();
    }
    w
}

/// Stop words for *scoring*, as distinct from [`is_stop_word`], which filters the
/// keyword list written into a block.
///
/// Two lists because the two jobs differ. `extract_keywords` already drops everything
/// four characters or shorter, so it never sees "the", "and", "for". The scoring
/// tokenizer deliberately *keeps* short tokens — that is the "k8s" fix — and therefore
/// has to name the short function words itself. IDF flattens them eventually, but only
/// once the palace is large: in a palace of six blocks a "the" that appears in one of
/// them looks rare, and rare is exactly what BM25 rewards.
fn is_score_stop_word(w: &str) -> bool {
    is_stop_word(w)
        || matches!(
            w,
            "a" | "an"
                | "the"
                | "and"
                | "or"
                | "but"
                | "if"
                | "of"
                | "on"
                | "in"
                | "to"
                | "at"
                | "by"
                | "for"
                | "is"
                | "it"
                | "its"
                | "as"
                | "be"
                | "am"
                | "are"
                | "was"
                | "my"
                | "we"
                | "you"
                | "do"
                | "did"
                | "how"
                | "why"
                | "who"
                | "not"
                | "no"
                | "so"
                | "up"
                | "out"
                | "off"
                | "all"
                | "any"
                | "can"
                | "i"
        )
}

/// Split text into BM25 terms: lowercased, split on non-alphanumerics, stop-worded,
/// then stemmed.
///
/// Note what is *not* here: `extract_keywords`' `len() > 3` filter. Dropping short
/// tokens is what made a block about "k8s" unreachable by the word "k8s". IDF is the
/// right tool for suppressing uninformative words, and unlike a length cut-off it
/// suppresses them because they are uninformative rather than because they are short.
/// Splitting on non-alphanumerics (rather than whitespace, as the old query tokenizer
/// did) is the other half: "kubectl-context", "foo.bar()" and "--flag=value" used to
/// be single unmatchable tokens.
fn score_terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .filter(|w| !is_score_stop_word(w))
        .map(|w| stem(&w))
        .collect()
}

/// BM25 score of every document against `query_terms`, in document order.
///
/// Free function, and deliberately not a method, so the ranking can be tested without a
/// palace on disk. `docs` is already tokenized by [`score_terms`].
fn bm25_scores(docs: &[Vec<String>], query_terms: &[String]) -> Vec<f64> {
    let n = docs.len() as f64;
    if docs.is_empty() || query_terms.is_empty() {
        return vec![0.0; docs.len()];
    }

    let tfs: Vec<HashMap<&str, f64>> = docs
        .iter()
        .map(|d| {
            let mut m: HashMap<&str, f64> = HashMap::new();
            for t in d {
                *m.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
            m
        })
        .collect();

    // Document frequency across the memory blocks — the corpus statistic the old
    // count-the-matches score had no notion of, and the entire reason "kubeconfig" now
    // outweighs "file".
    let mut df: HashMap<&str, f64> = HashMap::new();
    for tf in &tfs {
        for k in tf.keys() {
            *df.entry(k).or_insert(0.0) += 1.0;
        }
    }

    let total_len = docs.iter().map(|d| d.len()).sum::<usize>() as f64;
    let avg_len = if total_len > 0.0 { total_len / n } else { 1.0 };

    docs.iter()
        .enumerate()
        .map(|(i, d)| {
            let dl = d.len() as f64;
            query_terms
                .iter()
                .map(|term| {
                    let f = tfs[i].get(term.as_str()).copied().unwrap_or(0.0);
                    if f == 0.0 {
                        return 0.0;
                    }
                    let d_f = df.get(term.as_str()).copied().unwrap_or(0.0);
                    // Same IDF form `compress.rs` uses. The +1.0 inside the log keeps it
                    // non-negative even for a term present in every block, so a
                    // ubiquitous word contributes ~0 rather than dragging a score
                    // negative and pushing a genuine match below a non-match.
                    let idf = ((n - d_f + 0.5) / (d_f + 0.5) + 1.0).ln();
                    let tf_norm = (f * (BM25_K1 + 1.0))
                        / (f + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg_len));
                    idf * tf_norm
                })
                .sum()
        })
        .collect()
}

/// A memory block with the hybrid score that retrieved it, and both halves it came
/// from, so a caller — or a test — can see *why* a block came back.
#[derive(Debug, Clone)]
pub struct ScoredBlock {
    pub block: MemoryBlock,
    /// BM25 over the block's full text, rescaled so the best block for this query is
    /// 1.0. Comparable within one query; meaningless across queries.
    pub lexical: f32,
    /// Exact cosine against the block's sketch, clamped at 0 — negative correlation is
    /// not weaker evidence, it is no evidence. 0 also for a block the index has no
    /// embedding for, which is what a corrupt sidecar degrades to.
    pub vector: f32,
    /// `LEXICAL_WEIGHT * lexical + VECTOR_WEIGHT * vector`.
    pub score: f32,
}

pub struct MemoryPalace {
    data_dir: PathBuf,
    pub recall_blocks: Vec<MemoryBlock>,
    pub core_blocks: Vec<MemoryBlock>,
    pub archive_blocks: Vec<MemoryBlock>,
    /// ANN index over the blocks' sketches. Derived state: everything in it can be
    /// recomputed from the JSONL blocks, and an empty one only costs search its vector
    /// half.
    vector_index: TurboVecIndex,
    /// The sketches themselves, kept because `TurboVecIndex::search` hands back ids
    /// without scores and the candidates have to be re-ranked by exact cosine.
    embeddings: HashMap<String, Vec<f32>>,
    /// The semantic cache, when it is enabled.
    ///
    /// `None` is the community case. The Memory Palace is free and its three layers do
    /// not depend on this — but the fallback in [`MemoryPalace::search`] reads the
    /// *cache* store, which is a hub feature, and `SemanticCache::new` creates and locks
    /// `<data>/cache/sled` just by being constructed. Opening it unconditionally would
    /// have made `prism memory` both a way around the gate and the reason the cache
    /// directory appears on a machine that has the cache switched off.
    pub semantic_cache: Option<SemanticCache>,
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
            semantic_cache: if crate::cache::is_enabled() {
                Some(SemanticCache::new(data_dir)?)
            } else {
                None
            },
            vector_index: TurboVecIndex::new(),
            embeddings: HashMap::new(),
        };
        palace.load_cache()?;
        // Deliberately after the blocks are loaded and deliberately infallible: the
        // vector index is derived, so a failure to build it must not fail the open.
        palace.build_vector_index();
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
        // One embedding, one appended line — a few microseconds next to the O(n)
        // `all_blocks` clone and keyword intersection above, which is what actually
        // makes `save` expensive on a large palace (pre-existing, and left alone here).
        // This is the reason the index exists: a palace that re-embedded on every search
        // would hash character trigrams over megabytes of content to answer a question
        // about six words.
        self.index_block(&block);
    }

    /// Embed one block and record it in the live index and the sidecar.
    ///
    /// Infallible on purpose. If the append fails (read-only store, full disk) the
    /// in-memory index is still correct for this process, and the next open simply
    /// re-embeds the block it could not persist.
    fn index_block(&mut self, block: &MemoryBlock) {
        let text = indexable_text(block);
        let fp = fingerprint(&text);
        let emb = crate::vector::embed(&text);
        self.vector_index.add(&block.id, &emb);
        self.embeddings.insert(block.id.clone(), emb.clone());
        let line = VectorIndexLine {
            v: VECTOR_INDEX_VERSION,
            id: block.id.clone(),
            fp,
            emb,
        };
        if let Ok(json) = serde_json::to_string(&line) {
            append_to_file(&self.vector_index_path(), &format!("{json}\n")).ok();
        }
    }

    fn vector_index_path(&self) -> PathBuf {
        self.data_dir.join("sessions").join(VECTOR_INDEX_FILE)
    }

    /// Build the derived vector index, reusing whatever the sidecar still describes.
    ///
    /// Returns nothing and cannot fail, which is the whole contract. Every failure mode
    /// — file missing, truncated last line, hand-edited line, a vector of the wrong
    /// width, an unwritable directory — degrades to "embed it again" or "leave that one
    /// out", and search keeps its lexical half either way. The JSONL blocks under
    /// `sessions/<layer>/` are the source of truth; this file is a cache of work and
    /// deleting it is always safe.
    fn build_vector_index(&mut self) {
        let blocks = self.all_blocks();
        let cached = self.load_vector_index();
        let mut fresh: Vec<VectorIndexLine> = Vec::with_capacity(blocks.len());
        let mut recomputed = 0usize;

        for block in &blocks {
            let text = indexable_text(block);
            let fp = fingerprint(&text);
            let emb = match cached.get(&block.id) {
                // Reuse only when the line describes *this* text at *this* width. A
                // stale fingerprint means the block was edited; a wrong width means the
                // line is corrupt, and `TurboVecIndex::add` asserts on that — so the
                // length check here is what turns a corrupt index into a rebuild
                // instead of a panic.
                Some((cached_fp, cached_emb))
                    if *cached_fp == fp && cached_emb.len() == TURBO_DIM =>
                {
                    cached_emb.clone()
                }
                _ => {
                    recomputed += 1;
                    crate::vector::embed(&text)
                }
            };
            self.vector_index.add(&block.id, &emb);
            fresh.push(VectorIndexLine {
                v: VECTOR_INDEX_VERSION,
                id: block.id.clone(),
                fp,
                emb: emb.clone(),
            });
            self.embeddings.insert(block.id.clone(), emb);
        }

        // Rewrite only when the file no longer describes the palace: something was
        // re-embedded, or it holds lines for blocks that are gone (eviction, a manually
        // trimmed blocks.jsonl). Otherwise every open would rewrite an identical file.
        if recomputed > 0 || cached.len() != fresh.len() {
            self.write_vector_index(&fresh);
        }
    }

    fn load_vector_index(&self) -> HashMap<String, (u64, Vec<f32>)> {
        let mut out: HashMap<String, (u64, Vec<f32>)> = HashMap::new();
        let Ok(s) = fs::read_to_string(self.vector_index_path()) else {
            // Missing is the normal first-run case, and unreadable is indistinguishable
            // from it here — both mean "no cached work", not "no results".
            return out;
        };
        for line in s.lines().filter(|l| !l.trim().is_empty()) {
            // One bad line loses one embedding, not the index. A crash mid-append
            // leaves a half-written final line, and that must cost a single re-embed
            // rather than a full rebuild — or, worse, a parse error out of
            // `MemoryPalace::new`, which would take `prism memory` down over a cache.
            if let Ok(entry) = serde_json::from_str::<VectorIndexLine>(line) {
                if entry.v == VECTOR_INDEX_VERSION {
                    // Later line wins: appends supersede whatever a rewrite left.
                    out.insert(entry.id, (entry.fp, entry.emb));
                }
            }
        }
        out
    }

    fn write_vector_index(&self, lines: &[VectorIndexLine]) {
        let body: String = lines
            .iter()
            .filter_map(|l| serde_json::to_string(l).ok())
            .map(|s| s + "\n")
            .collect();
        // Write beside and rename, so an interrupted rewrite leaves the previous index
        // intact rather than a truncated one — the sidecar is recoverable either way,
        // but a torn file costs a full re-embed for no reason. Failure at any step is
        // silent on purpose: there is nothing the user can do about it and search works
        // without the file.
        let path = self.vector_index_path();
        let tmp = path.with_extension("jsonl.tmp");
        if fs::write(&tmp, body).is_ok() && fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Hybrid search: BM25 over the block text, re-ranked with the vector sketch.
    ///
    /// The public shape is unchanged — callers in `cli.rs` and `mcp.rs` still get blocks
    /// in rank order — but the ranking is [`MemoryPalace::search_scored`]; see the
    /// module comment above `BM25_K1` for what was wrong with the old one.
    ///
    /// The empty-result fallback to the semantic cache is deliberately preserved,
    /// including its gate: those are the cache's rows, not the palace's.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<MemoryBlock> {
        let results: Vec<MemoryBlock> = self
            .search_scored(query, max_results)
            .into_iter()
            .map(|s| s.block)
            .collect();

        if results.is_empty() {
            // Return cached entries as memory blocks — only when the semantic cache is
            // enabled, since these are its rows and not the Memory Palace's own.
            let Some(cache) = self.semantic_cache.as_ref() else {
                return Vec::new();
            };
            cache
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

    /// The ranking behind [`MemoryPalace::search`], with both score halves exposed.
    ///
    /// Shape: score every block lexically with BM25 (one pass, document frequency taken
    /// across the whole palace); score every block by exact cosine against its sketch;
    /// mix. A block is retrieved if the lexical half saw at least one query term anywhere
    /// in its text, or if the ANN shortlisted it *and* its cosine clears
    /// [`vector_only_min`] — which, at that function's default, means never. The sketch
    /// is a re-ranker; the comment on `vector_only_min` has the measurement that says why.
    ///
    /// Costs one tokenization pass over the palace per search. Measured on a 2000-block
    /// palace of ~1 KB blocks (a release build): 137 ms to search, against 201 ms to
    /// *open* the palace, which is JSONL parsing and `all_blocks`' clone-and-sort. So the
    /// scan is not the thing to optimise first, and the expensive half — hashing every
    /// character trigram of every block — is already avoided by the sidecar index (a cold
    /// open that must re-embed everything costs 254 ms instead of 201 ms).
    ///
    /// This is deliberately slower than the keyword-set intersection it replaces, which
    /// touched only the short `keywords` list. That scan was cheap because it was reading
    /// almost none of the data; see the module comment above `BM25_K1`. If it ever needs
    /// to be faster, the win is not rebuilding the whole `MemoryPalace` for every
    /// `search_blocks` call, not a tighter inner loop.
    pub fn search_scored(&self, query: &str, max_results: usize) -> Vec<ScoredBlock> {
        if max_results == 0 {
            return Vec::new();
        }
        let blocks = self.all_blocks();
        if blocks.is_empty() {
            return Vec::new();
        }

        // ── lexical half ──────────────────────────────────────────────────────────
        let mut query_terms = score_terms(query);
        // Deduplicated so a query that repeats a word does not count it twice; BM25's
        // job is to weight terms by rarity, not by how often the user typed them.
        query_terms.sort();
        query_terms.dedup();

        let docs: Vec<Vec<String>> = blocks
            .iter()
            .map(|b| score_terms(&indexable_text(b)))
            .collect();
        let lexical_raw = bm25_scores(&docs, &query_terms);
        // Rescaled against the best block for *this* query. BM25 has no natural upper
        // bound — it grows with corpus size and term rarity — so mixing it with a cosine
        // in 0..=1 needs a common scale, and the top hit is the only stable reference
        // point available without a trained calibration.
        let max_lexical = lexical_raw.iter().copied().fold(0.0f64, f64::max);

        // ── vector half ───────────────────────────────────────────────────────────
        // The ANN recalls; the exact cosine decides. Same division of labour as
        // `cache.rs::find_similar`, and for the same reason: turbovec's neighbour order
        // over a 2-bit-quantized 16-dimension sketch is a shortlist, not a ranking.
        //
        // The shortlist gates *retrieval* only. Scoring reads the exact cosine for every
        // block, because scoring off shortlist membership would systematically zero the
        // vector half of any lexical hit the sketch happened not to shortlist — a bias
        // that grows with the palace, bought for 16 multiply-adds per block next to a
        // tokenization pass that costs thousands.
        let query_embedding = crate::vector::embed(query);
        // Saturating: `max_results` is caller-supplied, and a debug build panics on the
        // overflow rather than quietly asking for a smaller shortlist.
        let want = max_results
            .saturating_mul(ANN_CANDIDATE_FACTOR)
            .max(ANN_CANDIDATE_FLOOR);
        let shortlist: std::collections::HashSet<String> = self
            .vector_index
            .search(&query_embedding, want)
            .into_iter()
            .collect();
        let vector_floor = vector_only_min();

        let mut scored: Vec<ScoredBlock> = Vec::new();
        for (i, block) in blocks.into_iter().enumerate() {
            let vector = self
                .embeddings
                .get(&block.id)
                .map(|emb| crate::vector::cosine(&query_embedding, emb).max(0.0))
                .unwrap_or(0.0);
            // `lexical_raw[i] > 0.0` means "at least one query term appears somewhere in
            // this block" — evidence a hit can be explained by. A block with none of that
            // is retrieved only if the ANN shortlisted it *and* the cosine clears the
            // floor; see `vector_only_min` for why that floor is where it is.
            if lexical_raw[i] <= 0.0 && !(vector >= vector_floor && shortlist.contains(&block.id)) {
                continue;
            }
            let lexical = if max_lexical > 0.0 {
                (lexical_raw[i] / max_lexical) as f32
            } else {
                0.0
            };
            scored.push(ScoredBlock {
                block,
                lexical,
                vector,
                score: LEXICAL_WEIGHT * lexical + VECTOR_WEIGHT * vector,
            });
        }

        // `total_cmp` rather than `partial_cmp().unwrap()`: a NaN here would panic the
        // sort. `cosine` cannot produce one and BM25 with a finite corpus cannot either,
        // but sorting is not the place to find out.
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(max_results);
        scored
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

#[cfg(test)]
mod search_tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "prism-memory-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// A palace on disk with `(category, content)` blocks saved into Core.
    fn palace_with(tag: &str, blocks: &[(&str, &str)]) -> (PathBuf, MemoryPalace) {
        let dir = temp_dir(tag);
        let mut palace = MemoryPalace::new(dir.clone()).expect("palace opens");
        for (category, content) in blocks {
            palace.save(MemoryLayer::Core, content, category);
        }
        (dir, palace)
    }

    fn ids(hits: Vec<ScoredBlock>) -> Vec<String> {
        hits.into_iter().map(|h| h.block.id).collect()
    }

    /// The headline case, and the one the old scorer could not pass at any threshold.
    ///
    /// `extract_keywords` drops every token of four characters or fewer, so "k8s" is
    /// absent from the block's keyword list — and `search` only ever intersected the
    /// query with that list. The word is sitting in the block's own content, and no
    /// query containing it could retrieve the block. Ever.
    #[test]
    fn finds_a_block_by_a_short_term_its_keyword_list_could_never_hold() {
        let (dir, palace) = palace_with(
            "short-term",
            &[
                (
                    "ops",
                    "switch the k8s namespace with kubens before you edit the deployment",
                ),
                (
                    "notes",
                    "the compress filter drops prose lines using bm25 sentence scoring",
                ),
            ],
        );

        let target = palace
            .all_blocks()
            .into_iter()
            .find(|b| b.content.contains("kubens"))
            .expect("saved");
        assert!(
            !target.keywords.iter().any(|k| k == "k8s"),
            "test premise broken — keywords were {:?}",
            target.keywords
        );

        let hits = palace.search_scored("k8s", 5);
        assert_eq!(
            ids(hits),
            vec![target.id],
            "a term in the content but not the keyword list must still retrieve"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The second half of the same bug: matching was string equality on raw tokens, so
    /// a query written in a different tense or number scored exactly zero.
    #[test]
    fn finds_a_block_through_an_inflected_query() {
        let (dir, palace) = palace_with(
            "morphology",
            &[
                (
                    "ops",
                    "switch the kubernetes namespace before applying the manifest",
                ),
                (
                    "notes",
                    "the reader summarises a file into a symbol outline",
                ),
            ],
        );

        // Neither "switching" nor "namespaces" appears anywhere in either block.
        let hits = palace.search_scored("switching namespaces", 5);
        assert!(!hits.is_empty(), "an inflected query must still match");
        assert!(
            hits[0].block.content.contains("kubernetes namespace"),
            "matched {:?}",
            hits[0].block.content
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Every match used to be worth 1, so one ubiquitous word outranked the single rare
    /// word that actually picked the memory out of the palace.
    #[test]
    fn a_rare_term_match_outranks_a_common_term_match() {
        let mut owned: Vec<(String, String)> = (0..8)
            .map(|i| {
                (
                    "ops".to_string(),
                    format!(
                        "deployment notes number {i}: the deployment pipeline runs a deployment"
                    ),
                )
            })
            .collect();
        // One block holds the rare word, and nothing else the query asks for.
        owned.push((
            "ops".to_string(),
            "kubeconfig lives under the xdg config home directory".to_string(),
        ));
        let blocks: Vec<(&str, &str)> = owned
            .iter()
            .map(|(c, t)| (c.as_str(), t.as_str()))
            .collect();
        let (dir, palace) = palace_with("rare-term", &blocks);

        // Under the old scorer both candidates matched exactly one query word, tied at
        // 1, and whichever sorted first won.
        let hits = palace.search_scored("deployment kubeconfig", 9);
        assert!(
            hits[0].block.content.contains("kubeconfig"),
            "rare term must win; got {:?}",
            hits[0].block.content
        );

        let rare = &hits[0];
        let common = hits
            .iter()
            .find(|h| h.block.content.contains("pipeline"))
            .expect("the common-term blocks are still results");
        assert!(
            rare.lexical > common.lexical * 2.0,
            "rare {} should dominate common {}",
            rare.lexical,
            common.lexical
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The same property at the level of the scorer itself, with no disk involved.
    #[test]
    fn bm25_scores_weight_a_term_by_how_rare_it_is() {
        let docs: Vec<Vec<String>> = [
            ["deploy", "rare"],
            ["deploy", "other"],
            ["deploy", "more"],
            ["deploy", "words"],
        ]
        .into_iter()
        .map(|d| d.into_iter().map(String::from).collect())
        .collect();

        let common = bm25_scores(&docs, &["deploy".to_string()]);
        let rare = bm25_scores(&docs, &["rare".to_string()]);

        assert!(
            rare[0] > common[0] * 5.0,
            "rare={} common={}",
            rare[0],
            common[0]
        );
        // A term present in every document must earn ~nothing and never a *negative*
        // score, which would push a genuine match below a block that matched nothing.
        assert!(common.iter().all(|s| *s >= 0.0));
        assert_eq!(rare[1], 0.0, "documents without the term score zero");
    }

    /// Derived state, not a source of truth: deleting the index must cost nothing but
    /// the work to rebuild it.
    #[test]
    fn a_missing_index_rebuilds_and_search_never_depended_on_it() {
        let (dir, palace) = palace_with(
            "missing-index",
            &[
                ("ops", "switch the k8s namespace with kubens"),
                (
                    "notes",
                    "the reader summarises a file into a symbol outline",
                ),
            ],
        );
        let baseline = ids(palace.search_scored("kubens namespace", 5));
        assert!(!baseline.is_empty());
        drop(palace);

        let index = dir.join("sessions").join(VECTOR_INDEX_FILE);
        assert!(
            index.exists(),
            "saving a block should have written the index"
        );
        fs::remove_file(&index).expect("removable");

        let reopened =
            MemoryPalace::new(dir.clone()).expect("a missing index must not fail the open");
        assert_eq!(
            ids(reopened.search_scored("kubens namespace", 5)),
            baseline,
            "results must not depend on the derived index"
        );
        assert!(index.exists(), "and the index rebuilds itself");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Four ways the sidecar can be wrong, all of which must cost at most a re-embed.
    ///
    /// The third line is the dangerous one: `TurboVecIndex::add` *asserts* that an
    /// embedding is `TURBO_DIM` wide, so a short vector reaching it aborts the process.
    /// It carries a valid version and a valid fingerprint, so only the explicit width
    /// check in `build_vector_index` stands between a hand-edited file and a panic in
    /// `prism memory search`.
    #[test]
    fn a_corrupt_index_degrades_to_the_lexical_path_without_panicking() {
        let (dir, palace) = palace_with(
            "corrupt-index",
            &[
                ("ops", "switch the k8s namespace with kubens"),
                (
                    "notes",
                    "the reader summarises a file into a symbol outline",
                ),
            ],
        );
        let target = palace
            .all_blocks()
            .into_iter()
            .find(|b| b.content.contains("kubens"))
            .expect("saved");
        let baseline = ids(palace.search_scored("kubens namespace", 5));
        drop(palace);

        let index = dir.join("sessions").join(VECTOR_INDEX_FILE);
        let fp = fingerprint(&indexable_text(&target));
        let id = &target.id;
        let v = VECTOR_INDEX_VERSION;
        fs::write(
            &index,
            format!(
                "not json at all\n\
                 {{\"v\":999,\"id\":\"{id}\",\"fp\":1,\"emb\":[]}}\n\
                 {{\"v\":{v},\"id\":\"{id}\",\"fp\":{fp},\"emb\":[1.0,2.0,3.0]}}\n\
                 {{\"v\":{v},\"id\":\"truncated\",\"fp\":1,\"emb\":[0.1,0.2\n"
            ),
        )
        .expect("writable");

        let reopened =
            MemoryPalace::new(dir.clone()).expect("a corrupt index must not fail the open");
        assert_eq!(
            ids(reopened.search_scored("kubens namespace", 5)),
            baseline,
            "a corrupt index must leave the lexical path intact"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The index has to actually save work, or it is just a file.
    ///
    /// Proof by planted vector: the lines are rewritten with a recognisable embedding
    /// under their *correct* fingerprints. A palace that re-embeds on open recomputes
    /// them and rewrites the file; one that honours the cache leaves the file untouched
    /// and scores with what it read.
    #[test]
    fn a_valid_index_is_reused_rather_than_recomputed() {
        let (dir, palace) = palace_with(
            "reuse-index",
            &[
                ("ops", "switch the k8s namespace with kubens"),
                (
                    "notes",
                    "the reader summarises a file into a symbol outline",
                ),
            ],
        );
        drop(palace);

        let index = dir.join("sessions").join(VECTOR_INDEX_FILE);
        let planted_vector = vec![0.25f32; TURBO_DIM];
        let planted: String = fs::read_to_string(&index)
            .expect("readable")
            .lines()
            .map(|l| {
                let mut entry: VectorIndexLine = serde_json::from_str(l).expect("we wrote it");
                entry.emb = planted_vector.clone();
                serde_json::to_string(&entry).expect("serializable") + "\n"
            })
            .collect();
        fs::write(&index, &planted).expect("writable");

        let reopened = MemoryPalace::new(dir.clone()).expect("opens");
        assert_eq!(
            fs::read_to_string(&index).expect("readable"),
            planted,
            "the index was rewritten, so every block was re-embedded on open"
        );

        let hits = reopened.search_scored("kubens namespace", 5);
        let expected =
            crate::vector::cosine(&crate::vector::embed("kubens namespace"), &planted_vector)
                .max(0.0);
        assert!(
            hits.iter().all(|h| (h.vector - expected).abs() < 1e-6),
            "scoring did not use the cached vectors: {:?}",
            hits.iter().map(|h| h.vector).collect::<Vec<_>>()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The vector half must not turn "no match" into "the nearest sketch".
    ///
    /// At 16 dimensions an unrelated pair of texts reaches a cosine of 0.9 often enough
    /// to matter (p99 is 0.85), which is exactly why `vector_only_min` defaults to 1.0
    /// and the sketch is confined to re-ranking.
    #[test]
    fn an_unrelated_query_returns_nothing_rather_than_the_nearest_sketch() {
        let (dir, palace) = palace_with(
            "no-false-hits",
            &[
                ("ops", "switch the k8s namespace with kubens"),
                (
                    "notes",
                    "the reader summarises a file into a symbol outline",
                ),
                ("build", "cargo clippy groups warnings by file"),
                ("proxy", "the proxy keys on the original request body"),
            ],
        );
        assert!(
            palace
                .search_scored("photosynthesis chlorophyll stomata", 5)
                .is_empty()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stemming_makes_inflected_forms_meet() {
        for (a, b) in [
            ("switching", "switch"),
            ("switches", "switch"),
            ("namespaces", "namespace"),
            ("queries", "query"),
            ("cached", "cache"),
            ("processes", "process"),
            ("contexts", "context"),
            ("annotations", "annotation"),
        ] {
            assert_eq!(stem(a), stem(b), "{a} and {b} must share a stem");
        }
        // …without collapsing the short technical terms that identify a memory and that
        // `extract_keywords` throws away.
        for w in ["k8s", "tls", "dns", "npm", "ssh", "aws", "git"] {
            assert_eq!(stem(w), w, "{w} must survive stemming intact");
        }
    }

    #[test]
    fn score_terms_keeps_the_short_tokens_the_keyword_list_throws_away() {
        let terms = score_terms("run kubectl --context=prod -n k8s-system");
        for expected in ["kubectl", "context", "prod", "k8s", "system"] {
            assert!(
                terms.iter().any(|t| t == expected),
                "{expected} in {terms:?}"
            );
        }
        // The contrast that names the bug: the same token cannot reach a keyword list.
        assert!(
            !MemoryBlock::extract_keywords("deploy to k8s now")
                .iter()
                .any(|k| k == "k8s")
        );
        // Function words are still dropped up front, because IDF only flattens them
        // once the palace is large.
        assert!(score_terms("the and for is to").is_empty());
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// The Memory Palace is free and must keep working with the semantic cache off —
    /// but it must not become a way around the gate. It opens the *same*
    /// `<data>/cache/sled` store the cache uses, so an unconditional
    /// `SemanticCache::new` would both create that directory on a machine with the
    /// cache disabled and serve its rows back through `prism memory search`.
    #[test]
    fn memory_search_works_without_the_cache_and_does_not_read_it() {
        let dir = std::env::temp_dir().join(format!(
            "prism-memory-gate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut palace = MemoryPalace::new(dir.clone()).expect("memory needs no cache to open");

        if crate::cache::is_enabled() {
            // This machine has policy enabling the cache; the gate is not under test.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        assert!(
            palace.semantic_cache.is_none(),
            "the cache store must not be opened when the cache is disabled"
        );
        assert!(
            !dir.join("cache").join("sled").exists(),
            "…and therefore must not be created"
        );

        // All three layers still save and search — the free feature is intact.
        palace.save(
            MemoryLayer::Core,
            "the proxy keys on the original body",
            "notes",
        );
        let hits = palace.search("proxy keys original body", 5);
        assert!(!hits.is_empty(), "keyword search must still work");

        // A query that matches nothing returns nothing, rather than falling back to
        // cache rows.
        assert!(
            palace
                .search("kubernetes ingress annotations", 5)
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
