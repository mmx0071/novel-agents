//! Chapter summary index + BM25-lite recall (`lore/chapter_index.json`).

use crate::memory::{recall_tokens, ChapterDigest};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChapterIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub chapters: Vec<ChapterIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChapterIndexEntry {
    pub chapter: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tokens: Vec<String>,
    #[serde(default)]
    pub event_summary: String,
    #[serde(default)]
    pub hook: String,
    #[serde(default)]
    pub fingerprint: String,
}

fn default_version() -> u32 {
    1
}

pub fn index_path(project_dir: &Path) -> PathBuf {
    project_dir.join("lore/chapter_index.json")
}

pub fn load_index(project_dir: &Path) -> ChapterIndex {
    let path = index_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ChapterIndex {
            version: 1,
            ..Default::default()
        };
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_index(project_dir: &Path, index: &ChapterIndex) -> Result<()> {
    let path = index_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(index).context("serialize chapter_index")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Chapter numbers present in the index (sorted). Empty if index missing/empty.
pub fn list_indexed_chapters(project_dir: &Path) -> Vec<u32> {
    let mut out: Vec<u32> = load_index(project_dir)
        .chapters
        .iter()
        .map(|e| e.chapter)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn fingerprint_blob(event: &str, hook: &str, facts: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    event.hash(&mut h);
    hook.hash(&mut h);
    for f in facts {
        f.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn tokens_from_summary(event: &str, hook: &str, facts: &[String], plot: &str) -> Vec<String> {
    let blob = format!("{event} {hook} {plot} {}", facts.join(" "));
    recall_tokens(&blob)
}

/// Upsert one chapter from summarizer JSON (or summary.json value).
pub fn upsert_chapter_summary(
    project_dir: &Path,
    chapter: u32,
    summary_raw: &str,
) -> Result<()> {
    let v = extract_summary_value(summary_raw);
    let event = v
        .get("event_summary")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let hook = v
        .get("ending_hook")
        .or_else(|| v.get("hook"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let plot = v
        .get("plot_progress")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let facts: Vec<String> = v
        .get("new_facts")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let title = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let fp = fingerprint_blob(&event, &hook, &facts);
    let tokens = tokens_from_summary(&event, &hook, &facts, &plot);

    let mut index = load_index(project_dir);
    if let Some(slot) = index.chapters.iter_mut().find(|e| e.chapter == chapter) {
        if slot.fingerprint == fp && !slot.tokens.is_empty() {
            return Ok(());
        }
        *slot = ChapterIndexEntry {
            chapter,
            title,
            tokens,
            event_summary: event,
            hook,
            fingerprint: fp,
        };
    } else {
        index.chapters.push(ChapterIndexEntry {
            chapter,
            title,
            tokens,
            event_summary: event,
            hook,
            fingerprint: fp,
        });
    }
    index.chapters.sort_by_key(|e| e.chapter);
    save_index(project_dir, &index)
}

/// Drop one chapter from the BM25 index (e.g. after delete_chapter).
pub fn remove_chapter(project_dir: &Path, chapter: u32) -> Result<()> {
    let mut index = load_index(project_dir);
    let before = index.chapters.len();
    index.chapters.retain(|e| e.chapter != chapter);
    if index.chapters.len() != before {
        save_index(project_dir, &index)?;
    }
    Ok(())
}

/// Rebuild index by scanning `chapters/*/summary.json`.
pub fn rebuild_index(project_dir: &Path) -> Result<usize> {
    let chapters_dir = project_dir.join("chapters");
    let Ok(rd) = std::fs::read_dir(&chapters_dir) else {
        save_index(
            project_dir,
            &ChapterIndex {
                version: 1,
                chapters: Vec::new(),
            },
        )?;
        return Ok(0);
    };
    let mut index = ChapterIndex {
        version: 1,
        chapters: Vec::new(),
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(ch) = name.parse::<u32>() else {
            continue;
        };
        let summary_path = path.join("summary.json");
        let Ok(text) = std::fs::read_to_string(&summary_path) else {
            continue;
        };
        let v = extract_summary_value(&text);
        let event = v
            .get("event_summary")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let hook = v
            .get("ending_hook")
            .or_else(|| v.get("hook"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let plot = v
            .get("plot_progress")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let facts: Vec<String> = v
            .get("new_facts")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if event.trim().is_empty() && hook.trim().is_empty() && facts.is_empty() {
            continue;
        }
        let fp = fingerprint_blob(&event, &hook, &facts);
        let tokens = tokens_from_summary(&event, &hook, &facts, &plot);
        index.chapters.push(ChapterIndexEntry {
            chapter: ch,
            title: String::new(),
            tokens,
            event_summary: event,
            hook,
            fingerprint: fp,
        });
    }
    index.chapters.sort_by_key(|e| e.chapter);
    let n = index.chapters.len();
    save_index(project_dir, &index)?;
    Ok(n)
}

/// BM25-lite recall over the chapter index. Falls back to rebuild-once if empty.
pub fn bm25_recall(
    project_dir: &Path,
    haystack: &str,
    entity_names: &[String],
    exclude_chapters: &[u32],
    limit: usize,
) -> Vec<ChapterDigest> {
    if limit == 0 {
        return Vec::new();
    }
    let mut index = load_index(project_dir);
    if index.chapters.is_empty() {
        let _ = rebuild_index(project_dir);
        index = load_index(project_dir);
    }
    if index.chapters.is_empty() {
        return Vec::new();
    }

    let mut query_tokens = recall_tokens(haystack);
    for n in entity_names {
        let t = n.trim();
        if t.chars().count() >= 2 && !query_tokens.iter().any(|x| x == t) {
            query_tokens.push(t.to_string());
        }
    }
    if query_tokens.is_empty() {
        return Vec::new();
    }

    let docs: Vec<&ChapterIndexEntry> = index
        .chapters
        .iter()
        .filter(|e| !exclude_chapters.contains(&e.chapter))
        .collect();
    if docs.is_empty() {
        return Vec::new();
    }

    let n_docs = docs.len() as f64;
    let avg_dl = docs
        .iter()
        .map(|d| d.tokens.len() as f64)
        .sum::<f64>()
        / n_docs.max(1.0);

    let mut df: HashMap<&str, usize> = HashMap::new();
    for d in &docs {
        let mut seen = std::collections::HashSet::new();
        for t in &d.tokens {
            if seen.insert(t.as_str()) {
                *df.entry(t.as_str()).or_default() += 1;
            }
        }
    }

    let mut scored: Vec<(f64, &ChapterIndexEntry)> = Vec::new();
    for d in &docs {
        let mut score = 0.0;
        let dl = d.tokens.len() as f64;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in &d.tokens {
            *tf.entry(t.as_str()).or_default() += 1;
        }
        // Entity-name boost (substring on summary text).
        let blob = format!("{} {}", d.event_summary, d.hook);
        for n in entity_names {
            let name = n.trim();
            if name.chars().count() >= 2 && blob.contains(name) {
                score += 5.0;
            }
        }
        for qt in &query_tokens {
            let tf_q = *tf.get(qt.as_str()).unwrap_or(&0) as f64;
            if tf_q <= 0.0 {
                continue;
            }
            let df_q = *df.get(qt.as_str()).unwrap_or(&0) as f64;
            let idf = ((n_docs - df_q + 0.5) / (df_q + 0.5) + 1.0).ln().max(0.0);
            let denom = tf_q + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / avg_dl.max(1.0)));
            score += idf * (tf_q * (BM25_K1 + 1.0) / denom);
        }
        if score > 0.0 {
            scored.push((score, d));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.chapter.cmp(&a.1.chapter))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, e)| ChapterDigest {
            chapter: e.chapter,
            event_summary: e.event_summary.clone(),
            hook: e.hook.clone(),
            key_facts: Vec::new(),
            relationship_deltas: Vec::new(),
            plot_progress: String::new(),
        })
        .collect()
}

fn extract_summary_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }
    // Best-effort: find first `{…}` block.
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                if let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..=end]) {
                    return v;
                }
            }
        }
    }
    Value::Object(serde_json::Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn upsert_and_bm25_recall() {
        let dir = std::env::temp_dir().join("novelx-chapter-index-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        fs::create_dir_all(dir.join("chapters/001")).unwrap();
        let summary = r#"{"event_summary":"主角抵达落点目击异象。","ending_hook":"深处有光。","new_facts":["主角左臂受伤"]}"#;
        fs::write(dir.join("chapters/001/summary.json"), summary).unwrap();
        upsert_chapter_summary(&dir, 1, summary).unwrap();
        let listed = list_indexed_chapters(&dir);
        assert_eq!(listed, vec![1]);
        let hit = bm25_recall(&dir, "主角 异象 落点", &["主角".into()], &[], 3);
        assert!(!hit.is_empty());
        assert_eq!(hit[0].chapter, 1);
        let miss = bm25_recall(&dir, "无关话题xyz", &[], &[1], 3);
        assert!(miss.is_empty());
    }
}
