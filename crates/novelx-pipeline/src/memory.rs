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
/// Cap open foreshadow threads in the hot list (overflow → archived_threads).
pub const OPEN_THREAD_LIMIT: usize = 24;
/// Cap archived (still-open) foreshadow threads kept for recall.
pub const ARCHIVED_THREAD_LIMIT: usize = 120;
/// Cap volume rollups retained in memory.
pub const VOLUME_ROLLUP_LIMIT: usize = 40;

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
    /// Open threads pruned from the hot list — still dangling, recallable by keyword.
    #[serde(default)]
    pub archived_threads: Vec<OpenThread>,
    /// Per-volume continuity rollups (written on human-gated volume sync).
    #[serde(default)]
    pub volume_rollups: Vec<VolumeRollup>,
    #[serde(default)]
    pub asserted_facts: Vec<AssertedFact>,
    /// Highest chapter covered by `recent_digests` / last apply_summary_json.
    #[serde(default)]
    pub last_chapter: u32,
    /// Preserve unknown fields from older Python dumps.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VolumeRollup {
    pub volume_index: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub open_thread_ids: Vec<String>,
    #[serde(default)]
    pub chapter_end: u32,
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
    let mut key_facts: Vec<String> = v
        .get("new_facts")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    // Prefer structured body_state so next chapters get a locked injury/ability board.
    if let Some(bs) = v.get("body_state") {
        if let Some(arr) = bs.get("injuries").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    let line = if s.starts_with("伤势：") {
                        s.to_string()
                    } else {
                        format!("伤势：{s}")
                    };
                    if !key_facts.iter().any(|f| f.contains(s)) {
                        key_facts.push(line);
                    }
                }
            }
        }
        if let Some(arr) = bs.get("ability_loci").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    let line = if s.contains("能力位置") {
                        s.to_string()
                    } else {
                        format!("能力位置：{s}")
                    };
                    if !key_facts.iter().any(|f| f.contains(s)) {
                        key_facts.push(line);
                    }
                }
            }
        }
    }
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

    prune_open_threads_into_archive(&mut mem);
    // Resolve matches in archive too.
    if let Some(arr) = v.get("foreshadow_updates").and_then(|x| x.as_array()) {
        for item in arr {
            let text = item.as_str().unwrap_or("").to_string();
            if text.is_empty() || !(text.contains("回收") || text.contains("已收")) {
                continue;
            }
            for t in mem.archived_threads.iter_mut() {
                if t.status == "open"
                    && text.contains(&t.text.chars().take(12).collect::<String>())
                {
                    t.status = "resolved".into();
                    t.resolved_chapter = chapter;
                }
            }
        }
    }

    let fact_count = key_facts.len();
    save_memory(project_dir, &mem)?;
    let _ = crate::foreshadow::rebuild_foreshadow_index(project_dir);
    Ok(fact_count)
}

/// Move oldest open threads into `archived_threads` instead of dropping them.
pub fn prune_open_threads_into_archive(mem: &mut ProjectMemory) {
    let mut open: Vec<_> = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .cloned()
        .collect();
    open.sort_by_key(|t| t.planted_chapter);
    let mut closed: Vec<_> = mem
        .open_threads
        .iter()
        .filter(|t| t.status != "open" && !t.status.is_empty())
        .cloned()
        .collect();
    if open.len() > OPEN_THREAD_LIMIT {
        let overflow = open.len() - OPEN_THREAD_LIMIT;
        let oldest: Vec<_> = open.drain(..overflow).collect();
        for mut t in oldest {
            t.status = "open".into();
            if !mem
                .archived_threads
                .iter()
                .any(|a| a.id == t.id || a.text == t.text)
            {
                mem.archived_threads.push(t);
            }
        }
    }
    closed.extend(open);
    mem.open_threads = closed;

    // Cap archive: keep newest open entries.
    mem.archived_threads
        .retain(|t| t.status == "open" || t.status.is_empty());
    if mem.archived_threads.len() > ARCHIVED_THREAD_LIMIT {
        mem.archived_threads.sort_by_key(|t| t.planted_chapter);
        let skip = mem.archived_threads.len() - ARCHIVED_THREAD_LIMIT;
        mem.archived_threads = mem.archived_threads.split_off(skip);
    }
}

/// Human-gated: build + persist volume rollup and refresh foreshadow index.
pub fn confirm_volume_memory(
    project_dir: &Path,
    volume_index: u32,
    name: &str,
    start_chapter: u32,
    end_chapter: u32,
    progress_note: &str,
) -> Result<VolumeRollup> {
    let mut rollup = build_volume_rollup_from_summaries(
        project_dir,
        volume_index,
        name,
        start_chapter,
        end_chapter,
    );
    if !progress_note.trim().is_empty() {
        rollup.summary = format!(
            "{}\n进度：{}",
            truncate_chars(&rollup.summary, 700),
            truncate_chars(progress_note.trim(), 200)
        );
    }
    upsert_volume_rollup(project_dir, rollup.clone())?;
    let _ = crate::foreshadow::rebuild_foreshadow_index(project_dir);
    Ok(rollup)
}

/// Format a short preview for volume-end memory gate prompts.
pub fn format_volume_memory_preview(
    project_dir: &Path,
    volume_index: u32,
    name: &str,
    start_chapter: u32,
    end_chapter: u32,
) -> String {
    let rollup = build_volume_rollup_from_summaries(
        project_dir,
        volume_index,
        name,
        start_chapter,
        end_chapter,
    );
    let dangling = crate::foreshadow::format_dangling_for_context(project_dir, 8);
    let title = if name.is_empty() {
        format!("第{volume_index}卷")
    } else {
        format!("第{volume_index}卷「{name}」")
    };
    let mut out = format!(
        "{title}记忆草案（第{}–{}章）：\n{}",
        start_chapter.max(1),
        end_chapter.max(1),
        if rollup.summary.is_empty() {
            "（尚无章摘要可汇总）"
        } else {
            rollup.summary.as_str()
        }
    );
    if !dangling.is_empty() {
        out.push_str("\n\n仍开放伏笔（Top）：\n");
        out.push_str(&dangling);
    }
    out
}

/// Upsert a volume rollup (human-gated volume sync).
pub fn upsert_volume_rollup(project_dir: &Path, rollup: VolumeRollup) -> Result<()> {
    let mut mem = load_memory(project_dir);
    mem.volume_rollups
        .retain(|r| r.volume_index != rollup.volume_index);
    mem.volume_rollups.push(rollup);
    mem.volume_rollups.sort_by_key(|r| r.volume_index);
    if mem.volume_rollups.len() > VOLUME_ROLLUP_LIMIT {
        let skip = mem.volume_rollups.len() - VOLUME_ROLLUP_LIMIT;
        mem.volume_rollups = mem.volume_rollups.split_off(skip);
    }
    save_memory(project_dir, &mem)?;
    Ok(())
}

/// Build a deterministic volume rollup from on-disk chapter summaries.
pub fn build_volume_rollup_from_summaries(
    project_dir: &Path,
    volume_index: u32,
    name: &str,
    start_chapter: u32,
    end_chapter: u32,
) -> VolumeRollup {
    let mut lines = Vec::new();
    let start = start_chapter.max(1);
    let end = end_chapter.max(start);
    for ch in start..=end {
        let path = project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("summary.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let v = extract_summary_value(&text);
        let event = v
            .get("event_summary")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if event.is_empty() {
            continue;
        }
        lines.push(format!("第{ch}章：{}", truncate_chars(event, 120)));
    }
    let mem = load_memory(project_dir);
    let open_ids: Vec<String> = mem
        .open_threads
        .iter()
        .chain(mem.archived_threads.iter())
        .filter(|t| t.status == "open" || t.status.is_empty())
        .filter(|t| t.planted_chapter >= start && t.planted_chapter <= end)
        .map(|t| t.id.clone())
        .take(24)
        .collect();
    let joined = lines.join("\n");
    VolumeRollup {
        volume_index,
        name: name.to_string(),
        summary: truncate_chars(&joined, 900),
        open_thread_ids: open_ids,
        chapter_end: end,
    }
}

/// Keyword-recall archived dangling threads (not in the hot open list).
pub fn recall_archived_threads(mem: &ProjectMemory, haystack: &str, limit: usize) -> Vec<OpenThread> {
    if limit == 0 || haystack.trim().is_empty() {
        return Vec::new();
    }
    let tokens = recall_tokens(haystack);
    if tokens.is_empty() {
        return Vec::new();
    }
    let hot_ids: std::collections::HashSet<&str> = mem
        .open_threads
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    let mut scored: Vec<(usize, &OpenThread)> = Vec::new();
    for t in &mem.archived_threads {
        if !(t.status == "open" || t.status.is_empty()) {
            continue;
        }
        if hot_ids.contains(t.id.as_str()) {
            continue;
        }
        let blob = t.text.as_str();
        let score = tokens.iter().filter(|tok| blob.contains(tok.as_str())).count();
        if score > 0 {
            scored.push((score, t));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.planted_chapter.cmp(&a.1.planted_chapter)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, t)| t.clone())
        .collect()
}

/// Asserted facts whose text overlaps haystack / roster names.
pub fn select_asserted_facts_for_context(
    mem: &ProjectMemory,
    haystack: &str,
    roster_names: &[String],
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &AssertedFact)> = Vec::new();
    for f in &mem.asserted_facts {
        let mut score = 0usize;
        for name in roster_names {
            if !name.is_empty() && f.text.contains(name) {
                score += 3;
            }
        }
        for tok in recall_tokens(haystack).into_iter().take(16) {
            if f.text.contains(&tok) {
                score += 1;
            }
        }
        if score > 0 {
            scored.push((score, f));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.chapter.cmp(&a.1.chapter)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, f)| format!("[第{}章] {}", f.chapter, truncate_chars(&f.text, 100)))
        .collect()
}

/// Load archived `chapters/NNN/summary.json` digests not in the hot window,
/// ranked by keyword overlap with `haystack` (outline/draft).
pub fn recall_archived_summaries(
    project_dir: &Path,
    haystack: &str,
    exclude_chapters: &[u32],
    limit: usize,
) -> Vec<ChapterDigest> {
    recall_archived_summaries_with(project_dir, haystack, &[], exclude_chapters, limit)
}

/// Entity-name–driven recall: roster / outline names score higher than token overlap.
pub fn recall_archived_summaries_with(
    project_dir: &Path,
    haystack: &str,
    entity_names: &[String],
    exclude_chapters: &[u32],
    limit: usize,
) -> Vec<ChapterDigest> {
    if limit == 0 {
        return Vec::new();
    }
    let chapters_dir = project_dir.join("chapters");
    let Ok(rd) = std::fs::read_dir(&chapters_dir) else {
        return Vec::new();
    };
    let tokens = recall_tokens(haystack);
    let names: Vec<&str> = entity_names
        .iter()
        .map(|s| s.trim())
        .filter(|s| s.chars().count() >= 2)
        .collect();
    if tokens.is_empty() && names.is_empty() {
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
        let facts: Vec<String> = v
            .get("new_facts")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let blob = format!("{event} {hook} {plot_progress} {}", facts.join(" "));
        if blob.trim().is_empty() {
            continue;
        }
        let mut score = 0usize;
        for n in &names {
            if blob.contains(n) {
                score += 5;
            }
        }
        score += tokens.iter().filter(|t| blob.contains(t.as_str())).count();
        if score == 0 {
            continue;
        }
        scored.push((
            score,
            ChapterDigest {
                chapter: ch,
                event_summary: event,
                hook,
                key_facts: facts,
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

    #[test]
    fn recall_archived_prefers_entity_names() {
        let dir = std::env::temp_dir().join("novelx-memory-entity-recall");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("chapters/004")).unwrap();
        fs::create_dir_all(dir.join("chapters/005")).unwrap();
        fs::write(
            dir.join("chapters/004/summary.json"),
            r#"{"event_summary":"主角路过集市。","ending_hook":"天色将晚。","new_facts":[]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("chapters/005/summary.json"),
            r#"{"event_summary":"阿洛在矿脉留下记号。","ending_hook":"回声未绝。","new_facts":[]}"#,
        )
        .unwrap();
        // Haystack alone may miss; entity name boosts chapter 5.
        let hit = recall_archived_summaries_with(
            &dir,
            "继续追查记号",
            &["阿洛".into()],
            &[],
            2,
        );
        assert!(!hit.is_empty());
        assert_eq!(hit[0].chapter, 5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_volume_memory_writes_rollup() {
        let dir = std::env::temp_dir().join("novelx-memory-confirm-vol");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("chapters/001")).unwrap();
        fs::write(
            dir.join("chapters/001/summary.json"),
            r#"{"event_summary":"主角进入试炼场。","ending_hook":"门缝透光。","new_facts":[]}"#,
        )
        .unwrap();
        let rollup = confirm_volume_memory(&dir, 1, "试炼卷", 1, 1, "卷末确认").unwrap();
        assert_eq!(rollup.volume_index, 1);
        assert!(rollup.summary.contains("试炼") || rollup.summary.contains("进度"));
        let mem = load_memory(&dir);
        assert_eq!(mem.volume_rollups.len(), 1);
        let preview = format_volume_memory_preview(&dir, 1, "试炼卷", 1, 1);
        assert!(preview.contains("记忆草案"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_thread_overflow_archives_instead_of_drop() {
        let mut mem = ProjectMemory::default();
        for i in 1..=30 {
            mem.open_threads.push(OpenThread {
                id: format!("t{i}"),
                text: format!("伏笔线索{i}矿井"),
                status: "open".into(),
                planted_chapter: i,
                resolved_chapter: 0,
            });
        }
        prune_open_threads_into_archive(&mut mem);
        let open_n = mem
            .open_threads
            .iter()
            .filter(|t| t.status == "open")
            .count();
        assert_eq!(open_n, OPEN_THREAD_LIMIT);
        assert!(
            !mem.archived_threads.is_empty(),
            "oldest open threads must be archived"
        );
        assert!(
            mem.archived_threads.iter().any(|t| t.text.contains("线索1")),
            "oldest planted must survive in archive"
        );
        let recalled = recall_archived_threads(&mem, "矿井 线索", 5);
        assert!(!recalled.is_empty());
    }

    #[test]
    fn volume_rollup_from_summaries() {
        let dir = std::env::temp_dir().join("novelx-memory-rollup");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("chapters/001")).unwrap();
        fs::create_dir_all(dir.join("chapters/002")).unwrap();
        fs::write(
            dir.join("chapters/001/summary.json"),
            r#"{"event_summary":"主角抵达边境。","ending_hook":"门外有声"}"#,
        )
        .unwrap();
        fs::write(
            dir.join("chapters/002/summary.json"),
            r#"{"event_summary":"边境冲突升级。","ending_hook":"援军将至"}"#,
        )
        .unwrap();
        let rollup = build_volume_rollup_from_summaries(&dir, 1, "试炼卷", 1, 2);
        assert_eq!(rollup.volume_index, 1);
        assert!(rollup.summary.contains("边境"));
        upsert_volume_rollup(&dir, rollup).unwrap();
        let mem = load_memory(&dir);
        assert_eq!(mem.volume_rollups.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
