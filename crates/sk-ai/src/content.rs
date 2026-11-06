//! Content search: SigLIP 2 (Google, Apache-2.0) maps images and texts into one space of 768
//! numbers, in which an image and a text that describes it lie close together (and so do two
//! images of the same kind of scene). Each image is analysed once (`ImageEncoder`, about 0.3 s
//! on the processor); a search then only encodes its text (`TextEncoder`) and compares.
//!
//! The model learned from texts in many languages, lowercase, padded to 64 tokens with its
//! tokenizer (Gemma's BPE vocabulary); [`Tokenizer`] does the same.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};

use crate::models::{self, CONTENT_IMAGE, CONTENT_TEXT, CONTENT_TOKENIZER};
use crate::runtime::{Data, Half, Input, Net};

/// Input edge of the image encoder (the whole image, squeezed to a square, as trained).
const SIZE: usize = 224;
/// Length of an embedding.
pub const DIM: usize = 768;
/// Tokens of a text, padded (as trained).
const TOKENS: usize = 64;

/// Calibration of the model: a pair matches with probability
/// `sigmoid(LOGIT_SCALE * similarity + LOGIT_BIAS)` (read from the full model).
const LOGIT_SCALE: f32 = 112.6689;
const LOGIT_BIAS: f32 = -16.7717;

pub const NEEDED: [&models::ModelInfo; 3] = [&CONTENT_IMAGE, &CONTENT_TEXT, &CONTENT_TOKENIZER];

/// Identifies the embeddings of this model: stored ones of another are computed again.
pub fn tag() -> String {
    format!("siglip2b16-{}", &CONTENT_IMAGE.sha256[..12])
}

/// Similarity of two embeddings (both of length 1): 1 the same, around 0 unrelated.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// How likely the text of embedding `text` describes the image of `image` (0..1).
pub fn match_probability(image: &[f32], text: &[f32]) -> f32 {
    1.0 / (1.0 + (-(LOGIT_SCALE * similarity(image, text) + LOGIT_BIAS)).exp())
}

/// What is searched for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    /// A text: images it describes.
    Text,
    /// An image: images like it.
    Image,
}

/// A text describes an image from this similarity on (match probability about 0.07 %:
/// SigLIP's probabilities are small, but unrelated images lie far below)...
const TEXT_MATCH: f32 = 0.085;
/// ...unless the best matches lie much higher: then only those within this distance of the best
/// count (a query that fits a few images well shouldn't list all that fit it a little).
const TEXT_SPREAD: f32 = 0.05;
/// An image is like another from this similarity on (half of all pairs lie below 0.5, the
/// closest other image of a collection typically above 0.9).
const IMAGE_MATCH: f32 = 0.8;

/// The items that match a search, best first, from their similarities to it.
pub fn ranked<T>(mut scored: Vec<(T, f32)>, search: Search) -> Vec<T> {
    let best = scored.iter().map(|s| s.1).fold(f32::MIN, f32::max);
    let min = match search {
        Search::Text => TEXT_MATCH.max(best - TEXT_SPREAD),
        Search::Image => IMAGE_MATCH,
    };
    scored.retain(|s| s.1 >= min);
    // Stable: equal ones keep their order (the browser's sorting).
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().map(|s| s.0).collect()
}

/// Keywords that can be suggested for an image, one per line (German, as the user's).
const KEYWORD_LIST: &str = include_str!("../models/keywords.txt");
/// Their text embeddings: computed with the text encoder by the example `keyword_vectors`
/// (`KEYWORD_MAGIC`, count, length, hash of the list, then the values as f16).
const KEYWORD_VECTORS: &[u8] = include_bytes!("../models/keywords.bin");
const KEYWORD_MAGIC: &[u8; 4] = b"SKKW";

/// Hash of the keyword list (FNV-1a): the stored embeddings must belong to it.
fn list_hash(list: &str) -> u64 {
    list.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn keyword_words() -> Vec<&'static str> {
    KEYWORD_LIST.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// The keyword embeddings file for the list, made with `encoder`.
pub fn keyword_vectors_file(encoder: &TextEncoder) -> Result<Vec<u8>> {
    let words = keyword_words();
    let mut out = Vec::with_capacity(20 + words.len() * DIM * 2);
    out.extend_from_slice(KEYWORD_MAGIC);
    out.extend((words.len() as u32).to_le_bytes());
    out.extend((DIM as u32).to_le_bytes());
    out.extend(list_hash(KEYWORD_LIST).to_le_bytes());
    for w in words {
        for v in encoder.embed(w)? {
            out.extend(half::f16::from_f32(v).to_le_bytes());
        }
    }
    Ok(out)
}

/// The keywords with their embeddings.
pub struct Keywords {
    words: Vec<&'static str>,
    vectors: Vec<f32>,
}

impl Keywords {
    /// Those shipped with Silberkorn; `None` if their embeddings don't belong to the list.
    pub fn shipped() -> Option<&'static Keywords> {
        static KEYWORDS: std::sync::LazyLock<Option<Keywords>> =
            std::sync::LazyLock::new(|| Keywords::parse(KEYWORD_VECTORS, keyword_words(), list_hash(KEYWORD_LIST)));
        KEYWORDS.as_ref()
    }

    fn parse(file: &[u8], words: Vec<&'static str>, hash: u64) -> Option<Keywords> {
        let header = file.get(..20)?;
        let u32_at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().expect("4 bytes")) as usize;
        let fits = &header[..4] == KEYWORD_MAGIC
            && u32_at(4) == words.len()
            && u32_at(8) == DIM
            && u64::from_le_bytes(header[12..20].try_into().expect("8 bytes")) == hash
            && file.len() == 20 + words.len() * DIM * 2;
        if !fits {
            log::warn!("Keyword-Vektoren passen nicht zur Liste (example keyword_vectors ausführen)");
            return None;
        }
        let vectors = file[20..].as_chunks::<2>().0.iter().map(|b| half::f16::from_le_bytes(*b).to_f32()).collect();
        Some(Keywords { words, vectors })
    }

    /// Up to `n` keywords that describe the image of embedding `image`, best first, without
    /// those it has (case doesn't matter).
    pub fn suggest(&self, image: &[f32], has: &[String], n: usize) -> Vec<&'static str> {
        let scored = self
            .words
            .iter()
            .zip(self.vectors.as_chunks::<DIM>().0)
            .filter(|(w, _)| !has.iter().any(|h| h.to_lowercase() == w.to_lowercase()))
            .map(|(w, v)| (*w, similarity(image, v)))
            .collect();
        let mut found = ranked(scored, Search::Text);
        found.truncate(n);
        found
    }
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

/// The embedding among the network's outputs (the other one is per patch or token).
fn embedding(out: Vec<crate::runtime::Output>) -> Result<Vec<f32>> {
    let o = out.into_iter().find(|o| o.data.len() == DIM).context("Ausgabe des Netzes fehlt")?;
    Ok(normalized(o.data))
}

/// The image half of the model.
pub struct ImageEncoder {
    net: Net,
}

impl ImageEncoder {
    pub fn available() -> bool {
        models::available(&CONTENT_IMAGE)
    }

    pub fn load() -> Result<Self> {
        let net = Net::load_half(&models::path(&CONTENT_IMAGE), &[Input::F32([1, 3, SIZE, SIZE])], Half::ToF32).context("Inhaltssuche (Bild)")?;
        Ok(Self { net })
    }

    pub fn prefers_serial(&self) -> bool {
        self.net.prefers_serial()
    }

    /// The embedding of an sRGB image of any size.
    pub fn embed(&self, img: &image::RgbImage) -> Result<Vec<f32>> {
        let small = image::imageops::resize(img, SIZE as u32, SIZE as u32, image::imageops::FilterType::Triangle);
        // Planar, -1..1.
        let mut x = vec![0.0f32; 3 * SIZE * SIZE];
        for (i, p) in small.pixels().enumerate() {
            for c in 0..3 {
                x[c * SIZE * SIZE + i] = p[c] as f32 / 127.5 - 1.0;
            }
        }
        embedding(self.net.run(&[Data::F32(&x)])?)
    }
}

/// The text half of the model.
pub struct TextEncoder {
    net: Net,
    tokenizer: Tokenizer,
}

impl TextEncoder {
    pub fn available() -> bool {
        models::available(&CONTENT_TEXT) && models::available(&CONTENT_TOKENIZER)
    }

    pub fn load() -> Result<Self> {
        let tokenizer = Tokenizer::load(&models::path(&CONTENT_TOKENIZER))?;
        // Kept in f16 by tract: its 256 000 tokens would take 800 MB as f32.
        let net = Net::load_half(&models::path(&CONTENT_TEXT), &[Input::I64([1, TOKENS])], Half::Keep).context("Inhaltssuche (Text)")?;
        Ok(Self { net, tokenizer })
    }

    /// The embedding of a text (any language; lowercased as the model learned).
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let ids = self.tokenizer.encode(text);
        embedding(self.net.run(&[Data::I64(&ids)])?)
    }
}

/// Gemma's tokenizer as SigLIP 2 uses it: lowercase, spaces as "▁", BPE (bytes for characters
/// outside the vocabulary), `<eos>` at the end, padded with `<pad>` to [`TOKENS`].
pub struct Tokenizer {
    vocab: HashMap<String, i64>,
    /// Rank and result of merging two adjacent tokens.
    merges: HashMap<(i64, i64), (u32, i64)>,
    eos: i64,
    pad: i64,
    /// Characters of the longest token.
    longest: usize,
}

#[derive(serde::Deserialize)]
struct TokenizerFile {
    model: BpeModel,
}

#[derive(serde::Deserialize)]
struct BpeModel {
    vocab: HashMap<String, i64>,
    merges: Vec<(String, String)>,
}

impl Tokenizer {
    /// From a `tokenizer.json` of Hugging Face's tokenizers.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).with_context(|| format!("{} nicht lesbar", path.display()))?;
        let f: TokenizerFile = serde_json::from_reader(std::io::BufReader::new(file)).with_context(|| format!("{} unlesbar", path.display()))?;
        let vocab = f.model.vocab;
        let id = |t: &str| vocab.get(t).copied().ok_or_else(|| anyhow!("Token {t} fehlt"));
        let (eos, pad) = (id("<eos>")?, id("<pad>")?);
        let mut merges = HashMap::with_capacity(f.model.merges.len());
        for (rank, (a, b)) in f.model.merges.iter().enumerate() {
            if let (Some(&x), Some(&y), Some(&m)) = (vocab.get(a), vocab.get(b), vocab.get(&format!("{a}{b}"))) {
                merges.entry((x, y)).or_insert((rank as u32, m));
            }
        }
        ensure!(!merges.is_empty(), "{}: keine Wortteile", path.display());
        let longest = vocab.keys().map(|t| t.chars().count()).max().unwrap_or(1);
        Ok(Self { vocab, merges, eos, pad, longest })
    }

    /// The token ids of `text`, always [`TOKENS`] of them.
    pub fn encode(&self, text: &str) -> Vec<i64> {
        // Only the first tokens count, and merging takes quadratic time: a long text is cut
        // well behind what they can cover (a token changes only through its neighbours).
        let text: String = text.to_lowercase().chars().take(2 * TOKENS * self.longest).map(|c| if c == ' ' { '▁' } else { c }).collect();
        let mut ids: Vec<i64> = Vec::with_capacity(text.len());
        let mut buf = [0u8; 4];
        for c in text.chars() {
            match self.vocab.get(c.encode_utf8(&mut buf) as &str) {
                Some(&id) => ids.push(id),
                None => ids.extend(c.encode_utf8(&mut buf).bytes().filter_map(|b| self.vocab.get(&format!("<0x{b:02X}>")).copied())),
            }
        }
        // Merge the adjacent pair learned first, until none is left.
        while let Some((i, merged)) =
            ids.windows(2).enumerate().filter_map(|(i, w)| self.merges.get(&(w[0], w[1])).map(|&(rank, m)| (rank, i, m))).min().map(|(_, i, m)| (i, m))
        {
            ids[i] = merged;
            ids.remove(i + 1);
        }
        ids.truncate(TOKENS - 1);
        ids.push(self.eos);
        ids.resize(TOKENS, self.pad);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_matches_best_first() {
        // A text that fits nothing well: everything above the floor, best first.
        assert_eq!(ranked(vec![("a", 0.09), ("b", 0.05), ("c", 0.12)], Search::Text), ["c", "a"]);
        // One image fits much better: the weak matches go.
        assert_eq!(ranked(vec![("a", 0.09), ("b", 0.16), ("c", 0.12)], Search::Text), ["b", "c"]);
        assert!(ranked(vec![("a", 0.03)], Search::Text).is_empty());
        // Images: a fixed bound, equal ones in their order.
        assert_eq!(ranked(vec![("a", 0.85), ("b", 0.7), ("c", 1.0), ("d", 0.85)], Search::Image), ["c", "a", "d"]);
    }

    /// The shipped embeddings belong to the list (else `keyword_vectors` wasn't run after a change).
    #[test]
    fn shipped_keyword_vectors_match_the_list() {
        assert!(Keywords::shipped().is_some());
        assert!(Keywords::parse(KEYWORD_VECTORS, keyword_words(), list_hash("Hund\nKatze\n")).is_none());
    }

    /// Keywords the image already has (in any case) are not suggested again.
    #[test]
    fn suggests_keywords_the_image_lacks() {
        let axis = |i: usize| (0..DIM).map(|j| if j == i { 1.0 } else { 0.0 }).collect::<Vec<f32>>();
        let k = Keywords { words: vec!["Hund", "Strand", "Katze"], vectors: [axis(0), axis(1), axis(2)].concat() };
        let image = normalized((0..DIM).map(|j| [0.12, 0.115].get(j).copied().unwrap_or(0.0)).collect());
        assert_eq!(k.suggest(&image, &[], 5), ["Hund", "Strand"]);
        assert_eq!(k.suggest(&image, &["hund".to_string()], 5), ["Strand"]);
        assert_eq!(k.suggest(&image, &[], 1), ["Hund"]);
    }

    /// A vocabulary of a few tokens: the merges apply lowest rank first, unknown characters
    /// become bytes, and the ids end in `<eos>` and padding.
    #[test]
    fn tokenizes_like_bpe() {
        let dir = std::env::temp_dir().join(format!("sk-tok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tokenizer.json");
        let json = r#"{"model":{"vocab":{"<pad>":0,"<eos>":1,"a":2,"b":3,"▁":4,"ab":5,"ab▁":6,"b▁":7,"<0xC3>":8,"<0xA4>":9},
            "merges":[["a","b"],["ab","▁"],["b","▁"]]}}"#;
        std::fs::write(&path, json).unwrap();
        let t = Tokenizer::load(&path).unwrap();
        // "AB b" → "ab▁b": a+b first (rank 0), then ab+▁ (rank 1) before b+▁ could apply.
        assert_eq!(&t.encode("AB b")[..4], &[6, 3, 1, 0]);
        // "ä" is not in the vocabulary: its two UTF-8 bytes.
        assert_eq!(&t.encode("ä")[..3], &[8, 9, 1]);
        assert_eq!(t.encode("ab").len(), TOKENS);
        // Too long: cut so that `<eos>` still ends it.
        let long = t.encode(&"a ".repeat(100));
        assert_eq!((long.len(), long[TOKENS - 1]), (TOKENS, 1));
        // A pasted text of any length: as fast as a short one (merging is quadratic), the same tokens.
        let start = std::time::Instant::now();
        assert_eq!(t.encode(&"b ".repeat(200_000)), t.encode(&"b ".repeat(TOKENS)));
        assert!(start.elapsed() < std::time::Duration::from_secs(1), "{:?}", start.elapsed());
        std::fs::remove_dir_all(dir).ok();
    }

    /// The real vocabulary gives the ids of Hugging Face's tokenizers (when downloaded).
    #[test]
    fn real_tokenizer_matches_reference() {
        if !models::available(&CONTENT_TOKENIZER) {
            return;
        }
        let t = Tokenizer::load(&models::path(&CONTENT_TOKENIZER)).unwrap();
        let cases: [(&str, &[i64]); 6] = [
            ("Hund am Strand", &[91738, 1144, 40672, 1, 0]),
            ("Grüße aus München 🙂", &[2434, 17998, 2918, 144658, 33359, 1, 0]),
            ("  zwei  Leerzeichen", &[139, 131201, 139, 96541, 54554, 1, 0]),
            ("ÄÖÜ ß straße", &[235383, 235397, 235387, 110870, 4091, 7156, 1, 0]),
            ("café crème", &[81453, 60750, 1, 0]),
            ("123 456,78 €", &[235274, 235284, 235304, 235248, 235310, 235308, 235318, 235269, 235324, 235321, 8296, 1, 0]),
        ];
        for (text, ids) in cases {
            assert_eq!(&t.encode(text)[..ids.len()], ids, "{text}");
        }
    }
}
