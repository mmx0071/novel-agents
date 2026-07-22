//! Rolling chapter memory (`lore/memory.json`).

use crate::cards::{truncate_chars, truncate_chars_tail};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

/// Keep this many chapter digests in the hot window.
pub const RECENT_DIGEST_LIMIT: usize = 12;
/// Rolling summary char budget (prefer recent chapters).
pub const ROLLING_SUMMARY_LIMIT: usize = 2500;
/// Cap open foreshadow threads.
pub const OPEN_THREAD_LIMIT: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectMemory {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub rolling_summary: String,
    #[serde(default)]
    pub recent_digests: Vec<ChapterDigest>,
    #[serde(default)]
    pub open_threads: Vec<OpenThread>,
    #[serde(default)]
    pub asserted_facts: Vec<AssertedFact>,
    /// Highest chapter covered by `recent_digests` / last apply_summary_json.
    #[serde(default)]
    pub last_chapter: u32,
    /// Preserve unknown fields from older Python dumps.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChapterDigest {
    pub chapter: u32,
    #[serde(default)]
    pub event_summary: String,
    #[serde(default)]
    pub hook: String,
    #[serde(default)]
    pub key_facts: Vec<String>,
    #[serde(default)]
    pub relationship_deltas: Vec<Value>,
    #[serde(default)]
    pub plot_progress: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenThread {
    pub id: String,
    pub text: String,
    #[serde(default = "default_open")]
    pub status: String,
    #[serde(default)]
    pub planted_chapter: u32,
    #[serde(default)]
    pub resolved_chapter: u32,
}

fn default_open() -> String {
    "open".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssertedFact {
    pub id: String,
    pub text: String,
    pub chapter: u32,
    #[serde(default)]
    pub source: String,
}

pub fn memory_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/memory.json")
}

pub fn load_memory(project_dir: &Path) -> ProjectMemory {
    let path = memory_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ProjectMemory {
            version: 1,
            ..Default::default()
        };
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_memory(project_dir: &Path, mem: &ProjectMemory) -> Result<()> {
    let path = memory_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(mem).context("serialize memory")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Format one digest line for the rolling summary.
pub fn digest_line(d: &ChapterDigest) -> String {
    format!(
        "第{}章：{}{}",
        d.chapter,
        truncate_chars(&d.event_summary, 180),
        if d.hook.is_empty() {
            String::new()
        } else {
            format!("（钩子：{}）", truncate_chars(&d.hook, 60))
        }
    )
}

/// Rebuild rolling summary from digests, **keeping recent chapters** when over budget.
pub fn rebuild_rolling_summary(digests: &[ChapterDigest], max_chars: usize) -> String {
    if digests.is_empty() {
        return String::new();
    }
    let mut sorted = digests.to_vec();
    sorted.sort_by_key(|d| d.chapter);
    let lines: Vec<String> = sorted.iter().map(digest_line).collect();
    let mut joined = lines.join("\n");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    // Drop oldest lines until within budget (always keep at least the newest line).
    let mut start = 0usize;
    while start + 1 < lines.len() {
        let candidate = lines[start + 1..].join("\n");
        if candidate.chars().count() <= max_chars {
            return candidate;
        }
        start += 1;
    }
    joined = lines.last().cloned().unwrap_or_default();
    truncate_chars_tail(&joined, max_chars)
}

/// Merge summarizer JSON into rolling memory.
pub fn apply_summary_json(project_dir: &Path, chapter: u32, summary_raw: &str) -> Result<usize> {
    let v = extract_summary_value(summary_raw);
    let mut mem = load_memory(project_dir);

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
    let key_facts: Vec<String> = v
        .get("new_facts")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let plot_progress = v
        .get("plot_progress")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let rel = v
        .get("relationship_deltas")
        .or_else(|| v.get("relationship_changes"))
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    let rel_arr = match rel {
        Value::Array(a) => a,
        Value::String(s) if !s.is_empty() => {
            vec![serde_json::json!({"note": s})]
        }
        _ => vec![],
    };

    let digest = ChapterDigest {
        chapter,
        event_summary: event.clone(),
        hook: hook.clone(),
        key_facts: key_facts.clone(),
        relationship_deltas: rel_arr,
        plot_progress,
    };

    // Replace same-chapter digest if re-run.
    mem.recent_digests.retain(|d| d.chapter != chapter);
    mem.recent_digests.push(digest);
    mem.recent_digests.sort_by_key(|d| d.chapter);
    if mem.recent_digests.len() > RECENT_DIGEST_LIMIT {
        let skip = mem.recent_digests.len() - RECENT_DIGEST_LIMIT;
        mem.recent_digests = mem.recent_digests.split_off(skip);
    }

    // Always rebuild from digests so new chapters are never truncated away.
    mem.rolling_summary = rebuild_rolling_summary(&mem.recent_digests, ROLLING_SUMMARY_LIMIT);
    mem.last_chapter = mem
        .recent_digests
        .iter()
        .map(|d| d.chapter)
        .max()
        .unwrap_or(chapter)
        .max(chapter);
    // Drop stale flatten copy if older dumps stored last_chapter only in extra.
    mem.extra.remove("last_chapter");

    // Foreshadow / threads from foreshadow_updates.
    if let Some(arr) = v.get("foreshadow_updates").and_then(|x| x.as_array()) {
        for item in arr {
            let text = if let Some(s) = item.as_str() {
                s.to_string()
            } else {
                item.get("text")
                    .or_else(|| item.get("update"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            if text.is_empty() {
                continue;
            }
            let resolved = text.contains("回收") || text.contains("已收");
            if resolved {
                if let Some(t) = mem.open_threads.iter_mut().find(|t| {
                    t.status == "open"
                        && text.contains(&t.text.chars().take(12).collect::<String>())
                }) {
                    t.status = "resolved".into();
                    t.resolved_chapter = chapter;
                }
            } else if !mem.open_threads.iter().any(|t| t.text == text) {
                mem.open_threads.push(OpenThread {
                    id: format!("thr_{}", &Uuid::new_v4().simple().to_string()[..10]),
                    text,
                    status: "open".into(),
                    planted_chapter: chapter,
                    resolved_chapter: 0,
                });
            }
        }
    }

    // Cap open threads — keep most recently planted.
    let mut open: Vec<_> = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open")
        .cloned()
        .collect();
    open.sort_by_key(|t| t.planted_chapter);
    let mut closed: Vec<_> = mem
        .open_threads
        .iter()
        .filter(|t| t.status != "open")
        .cloned()
        .collect();
    if open.len() > OPEN_THREAD_LIMIT {
        open = open.split_off(open.len() - OPEN_THREAD_LIMIT);
    }
    closed.extend(open);
    mem.open_threads = closed;

    let fact_count = key_facts.len();
    save_memory(project_dir, &mem)?;
    Ok(fact_count)
}

/// Load archived `chapters/NNN/summary.json` digests not in the hot window,
/// ranked by keyword overlap with `haystack` (outline/draft).
pub fn recall_archived_summaries(
    project_dir: &Path,
    haystack: &str,
    exclude_chapters: &[u32],
    limit: usize,
) -> Vec<ChapterDigest> {
    if limit == 0 || haystack.trim().is_empty() {
        return Vec::new();
    }
    let chapters_dir = project_dir.join("chapters");
    let Ok(rd) = std::fs::read_dir(&chapters_dir) else {
        return Vec::new();
    };
    let tokens = recall_tokens(haystack);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, ChapterDigest)> = Vec::new();
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
        if exclude_chapters.contains(&ch) {
            continue;
        }
        let summary_path = path.join("summary.json");
        let Ok(text) = std::fs::read_to_string(&summary_path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
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
        let plot_progress = v
            .get("plot_progress")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let blob = format!("{event} {hook} {plot_progress}");
        if blob.trim().is_empty() {
            continue;
        }
        let score = tokens.iter().filter(|t| blob.contains(t.as_str())).count();
        if score == 0 {
            continue;
        }
        scored.push((
            score,
            ChapterDigest {
                chapter: ch,
                event_summary: event,
                hook,
                key_facts: v
                    .get("new_facts")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                relationship_deltas: Vec::new(),
                plot_progress,
            },
        ));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.chapter.cmp(&a.1.chapter)));
    scored.into_iter().take(limit).map(|(_, d)| d).collect()
}

fn recall_tokens(haystack: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let push = |t: String, tokens: &mut Vec<String>| {
        let n = t.chars().count();
        if n < 2 {
            return;
        }
        if matches!(
            t.as_str(),
            "一个" | "没有" | "已经" | "因为" | "所以" | "然后" | "他们" | "我们" | "自己"
                | "继续" | "开始" | "出现" | "什么" | "这个" | "那个"
        ) {
            return;
        }
        if !tokens.iter().any(|x| x == &t) {
            tokens.push(t);
        }
    };

    let mut cur = String::new();
    let flush = |cur: &mut String, tokens: &mut Vec<String>| {
        if cur.is_empty() {
            return;
        }
        let s = std::mem::take(cur);
        let chars: Vec<char> = s.chars().collect();
        if chars.iter().all(|c| c.is_ascii_alphanumeric()) {
            if chars.len() >= 3 {
                push(s, tokens);
            }
            return;
        }
        // Whole CJK/name run.
        if chars.len() >= 2 && chars.len() <= 12 {
            push(s.clone(), tokens);
        }
        // 2–3 char windows for longer runs (names / place phrases).
        if chars.len() >= 4 {
            for w in 2..=3 {
                for win in chars.windows(w) {
                    push(win.iter().collect(), tokens);
                }
            }
        }
    };

    for ch in haystack.chars() {
        if ch.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            let ascii = ch.is_ascii_alphanumeric();
            let cur_ascii = !cur.is_empty() && cur.chars().all(|c| c.is_ascii_alphanumeric());
            if !cur.is_empty() && ascii != cur_ascii {
                flush(&mut cur, &mut tokens);
            }
            cur.push(ch);
        } else {
            flush(&mut cur, &mut tokens);
        }
    }
    flush(&mut cur, &mut tokens);
    tokens.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
    tokens.truncate(48);
    tokens
}

fn extract_summary_value(raw: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return v;
    }
    // Try fenced / embedded JSON.
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                if let Ok(v) = serde_json::from_str::<Value>(&raw[start..=end]) {
                    return v;
                }
            }
        }
    }
    serde_json::json!({
        "event_summary": truncate_chars(raw, 400),
        "ending_hook": "",
        "new_facts": [],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn apply_summary_updates_digests() {
        let dir = std::env::temp_dir().join("novelx-memory-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let summary = r#"{
            "event_summary": "主角发现异变信号。",
            "ending_hook": "门外有脚步声。",
            "new_facts": ["信号源在旧矿井"],
            "foreshadow_updates": ["新埋：矿井下有人"]
        }"#;
        let n = apply_summary_json(&dir, 2, summary).unwrap();
        assert_eq!(n, 1);
        let mem = load_memory(&dir);
        assert_eq!(mem.recent_digests.len(), 1);
        assert_eq!(mem.last_chapter, 2);
        assert!(mem.rolling_summary.contains("第2章"));
        assert!(mem.open_threads.iter().any(|t| t.text.contains("矿井")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rolling_summary_keeps_recent_chapters() {
        let mut digests = Vec::new();
        for i in 1..=20 {
            digests.push(ChapterDigest {
                chapter: i,
                event_summary: format!("这是第{i}章很长的事件摘要用于撑满滚动窗口ABCDEFGHIJ"),
                hook: format!("钩子{i}"),
                ..Default::default()
            });
        }
        let rolling = rebuild_rolling_summary(&digests, 800);
        assert!(
            rolling.contains("第20章"),
            "must keep newest: {rolling}"
        );
        assert!(
            !rolling.contains("第1章"),
            "must drop oldest when over budget: {rolling}"
        );
    }

    #[test]
    fn recall_archived_by_keyword() {
        let dir = std::env::temp_dir().join("novelx-memory-recall");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("chapters/003")).unwrap();
        fs::write(
            dir.join("chapters/003/summary.json"),
            r#"{"event_summary":"主角在旧址见到故人残影。","ending_hook":"地下机关苏醒。","new_facts":[]}"#,
        )
        .unwrap();
        let hit = recall_archived_summaries(&dir, "主角 旧址 继续追查", &[], 3);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].chapter, 3);
        let miss = recall_archived_summaries(&dir, "无关话题", &[3], 3);
        assert!(miss.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
