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
/// Cap archived (still-open) foreshadow threads kept in `memory.json`
/// (overflow → `lore/foreshadow_archive.jsonl`, never hard-dropped).
pub const ARCHIVED_THREAD_LIMIT: usize = 120;
/// Cap volume rollups retained in hot memory (overflow → archive jsonl).
pub const VOLUME_ROLLUP_LIMIT: usize = 40;
/// Cap entity timeline facts in hot memory (overflow → archive jsonl).
pub const HOT_ENTITY_TIMELINE_LIMIT: usize = 200;

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
    /// Per-entity timeline facts (status / holdings / injury / relation).
    #[serde(default)]
    pub entity_timeline: Vec<EntityTimelineFact>,
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
    /// Expected payoff distance: `near` | `mid` | `far` (empty = auto by age).
    #[serde(default)]
    pub horizon: String,
    /// Tracker urgency: `low` | `mid` | `high` (empty = unknown).
    #[serde(default)]
    pub urgency: String,
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
    /// Optional entity this fact is about (longform timeline).
    #[serde(default)]
    pub entity: String,
    /// 0.0–1.0; higher preferred when recall ties. Default 1.0 for back-compat.
    #[serde(default = "default_fact_confidence")]
    pub confidence: f32,
}

fn default_fact_confidence() -> f32 {
    1.0
}

/// Structured per-entity continuity fact for long-horizon recall.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntityTimelineFact {
    pub entity: String,
    /// character | item | location | unknown
    #[serde(default)]
    pub kind: String,
    pub text: String,
    pub chapter: u32,
    /// status | holding | injury | ability | relation | fact
    #[serde(default)]
    pub predicate: String,
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

    // Single-pass foreshadow_updates: resolve (hot/archived/cold) or bury with full dedup.
    if let Some(arr) = v.get("foreshadow_updates").and_then(|x| x.as_array()) {
        let mut resolve_needles = Vec::new();
        let mut bury_texts = Vec::new();
        for item in arr {
            let text = foreshadow_update_text(item);
            if text.is_empty() {
                continue;
            }
            if text.contains("回收") || text.contains("已收") {
                resolve_needles.push(text);
            } else {
                bury_texts.push(text);
            }
        }
        if !resolve_needles.is_empty() {
            resolve_open_threads_in_memory(
                project_dir,
                &mut mem,
                chapter,
                &resolve_needles,
            );
            let _ = resolve_foreshadow_archive(project_dir, chapter, |t| {
                resolve_needles
                    .iter()
                    .any(|n| foreshadow_text_matches(n, &t.text))
            });
        }
        for text in bury_texts {
            if foreshadow_already_known(project_dir, &mem, None, &text) {
                continue;
            }
            mem.open_threads.push(OpenThread {
                id: format!("thr_{}", &Uuid::new_v4().simple().to_string()[..10]),
                text,
                status: "open".into(),
                planted_chapter: chapter,
                ..Default::default()
            });
        }
    }

    let _ = prune_open_threads_into_archive(Some(project_dir), &mut mem);

    // Entity timeline from body_state + new_facts (best-effort name attribution).
    ingest_entity_timeline_from_summary(project_dir, &mut mem, chapter, &v, &key_facts);

    let fact_count = key_facts.len();
    save_memory(project_dir, &mem)?;
    let _ = crate::foreshadow::rebuild_foreshadow_index(project_dir);
    // Keep chapter index in sync for BM25 recall (best-effort).
    let _ = crate::chapter_index::upsert_chapter_summary(project_dir, chapter, summary_raw);
    // SQLite acceleration layer (best-effort).
    let digest: String = event.chars().take(120).collect();
    let title = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let _ = crate::lore_index::upsert_chapter_index_row(
        project_dir,
        chapter,
        &title,
        &digest,
        true,
    );
    for t in mem.open_threads.iter().chain(mem.archived_threads.iter()) {
        let kw: String = t.text.chars().take(40).collect();
        let status = if t.status.is_empty() {
            "open"
        } else {
            t.status.as_str()
        };
        let _ = crate::lore_index::upsert_foreshadow_index_row(
            project_dir,
            &t.id,
            t.planted_chapter,
            status,
            &kw,
        );
    }
    Ok(fact_count)
}

fn foreshadow_update_text(item: &Value) -> String {
    if let Some(s) = item.as_str() {
        return s.to_string();
    }
    item.get("text")
        .or_else(|| item.get("update"))
        .or_else(|| item.get("description"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

pub fn foreshadow_text_matches(needle: &str, thread_text: &str) -> bool {
    if needle.is_empty() || thread_text.is_empty() {
        return false;
    }
    if needle.contains(thread_text) || thread_text.contains(needle) {
        return true;
    }
    let prefix: String = thread_text.chars().take(12).collect();
    !prefix.is_empty() && needle.contains(&prefix)
}

/// True if text/id already exists in hot, archived, or cold open foreshadows.
pub fn foreshadow_already_known(
    project_dir: &Path,
    mem: &ProjectMemory,
    id: Option<&str>,
    text: &str,
) -> bool {
    let text = text.trim();
    if text.is_empty() && id.unwrap_or("").is_empty() {
        return false;
    }
    let in_mem = mem
        .open_threads
        .iter()
        .chain(mem.archived_threads.iter())
        .any(|t| {
            if !(t.status == "open" || t.status.is_empty()) {
                return false;
            }
            if let Some(id) = id {
                if !id.is_empty() && t.id == id {
                    return true;
                }
            }
            !text.is_empty() && (t.text == text || foreshadow_text_matches(text, &t.text))
        });
    if in_mem {
        return true;
    }
    load_foreshadow_archive(project_dir).iter().any(|t| {
        if let Some(id) = id {
            if !id.is_empty() && t.id == id {
                return true;
            }
        }
        !text.is_empty() && (t.text == text || foreshadow_text_matches(text, &t.text))
    })
}

fn resolve_open_threads_in_memory(
    project_dir: &Path,
    mem: &mut ProjectMemory,
    chapter: u32,
    needles: &[String],
) {
    for t in mem
        .open_threads
        .iter_mut()
        .chain(mem.archived_threads.iter_mut())
    {
        if !(t.status == "open" || t.status.is_empty()) {
            continue;
        }
        if !needles
            .iter()
            .any(|n| foreshadow_text_matches(n, &t.text))
        {
            continue;
        }
        t.status = "resolved".into();
        t.resolved_chapter = chapter;
        let kw: String = t.text.chars().take(40).collect();
        let _ = crate::lore_index::upsert_foreshadow_index_row(
            project_dir,
            &t.id,
            t.planted_chapter,
            "resolved",
            &kw,
        );
    }
}

/// Move oldest open threads into `archived_threads`; spill archive overflow to jsonl
/// (never hard-delete open foreshadows).
pub fn prune_open_threads_into_archive(
    project_dir: Option<&Path>,
    mem: &mut ProjectMemory,
) -> Result<()> {
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

    mem.archived_threads
        .retain(|t| t.status == "open" || t.status.is_empty());
    if mem.archived_threads.len() > ARCHIVED_THREAD_LIMIT {
        mem.archived_threads.sort_by_key(|t| t.planted_chapter);
        let skip = mem.archived_threads.len() - ARCHIVED_THREAD_LIMIT;
        // Without a project_dir we cannot spill — keep overflow in memory (never hard-drop).
        if let Some(dir) = project_dir {
            let spilled: Vec<OpenThread> = mem.archived_threads.drain(..skip).collect();
            append_foreshadow_archive(dir, &spilled)?;
            for t in &spilled {
                let kw: String = t.text.chars().take(40).collect();
                let _ = crate::lore_index::upsert_foreshadow_index_row(
                    dir,
                    &t.id,
                    t.planted_chapter,
                    "open",
                    &kw,
                );
            }
        }
    }
    Ok(())
}

pub fn foreshadow_archive_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/foreshadow_archive.jsonl")
}

pub fn append_foreshadow_archive(project_dir: &Path, threads: &[OpenThread]) -> Result<()> {
    if threads.is_empty() {
        return Ok(());
    }
    let path = foreshadow_archive_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    for t in threads {
        let line = serde_json::to_string(t).context("serialize foreshadow archive row")?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

fn foreshadow_row_key(t: &OpenThread) -> String {
    if t.id.is_empty() {
        t.text.clone()
    } else {
        t.id.clone()
    }
}

/// Load cold foreshadow rows (last write wins per id/text). Includes resolved.
pub fn load_foreshadow_archive_rows(project_dir: &Path) -> Vec<OpenThread> {
    let path = foreshadow_archive_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut by_id: std::collections::HashMap<String, OpenThread> =
        std::collections::HashMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(t) = serde_json::from_str::<OpenThread>(line) else {
            continue;
        };
        by_id.insert(foreshadow_row_key(&t), t);
    }
    by_id.into_values().collect()
}

/// Open (still dangling) cold-archive foreshadows only.
pub fn load_foreshadow_archive(project_dir: &Path) -> Vec<OpenThread> {
    load_foreshadow_archive_rows(project_dir)
        .into_iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .collect()
}

/// Rewrite cold foreshadow archive (deduped rows).
pub fn save_foreshadow_archive(project_dir: &Path, threads: &[OpenThread]) -> Result<()> {
    let path = foreshadow_archive_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::File::create(&path)
        .with_context(|| format!("create {}", path.display()))?;
    for t in threads {
        let line = serde_json::to_string(t).context("serialize foreshadow archive row")?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

/// Mark matching open cold-archive threads resolved and sync SQLite.
pub fn resolve_foreshadow_archive(
    project_dir: &Path,
    chapter: u32,
    mut matches: impl FnMut(&OpenThread) -> bool,
) -> Result<usize> {
    let mut rows = load_foreshadow_archive_rows(project_dir);
    if rows.is_empty() {
        return Ok(0);
    }
    let mut n = 0usize;
    for t in &mut rows {
        if (t.status == "open" || t.status.is_empty()) && matches(t) {
            t.status = "resolved".into();
            t.resolved_chapter = chapter;
            n += 1;
            let kw: String = t.text.chars().take(40).collect();
            let _ = crate::lore_index::upsert_foreshadow_index_row(
                project_dir,
                &t.id,
                t.planted_chapter,
                "resolved",
                &kw,
            );
        }
    }
    if n > 0 {
        save_foreshadow_archive(project_dir, &rows)?;
    }
    Ok(n)
}

/// Sort key for longform: older planted threads rank higher (age boost).
pub fn foreshadow_age_priority(a: &OpenThread, b: &OpenThread) -> std::cmp::Ordering {
    a.planted_chapter
        .cmp(&b.planted_chapter)
        .then_with(|| a.id.cmp(&b.id))
}

/// Select dangling threads for prompts: oldest-first age boost, then fill remainder.
pub fn select_dangling_age_boosted(threads: &[OpenThread], limit: usize) -> Vec<OpenThread> {
    if limit == 0 {
        return Vec::new();
    }
    let mut open: Vec<_> = threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .cloned()
        .collect();
    open.sort_by(foreshadow_age_priority);
    open.truncate(limit);
    open
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
    let checklist = crate::volume_checklist::run_volume_memory_checklist(
        project_dir,
        volume_index,
        start_chapter,
        end_chapter,
    );
    if !checklist.items.is_empty() {
        out.push_str("\n\n");
        out.push_str(&checklist.summary_markdown());
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
        let spilled: Vec<VolumeRollup> = mem.volume_rollups.drain(..skip).collect();
        append_volume_rollups_archive(project_dir, &spilled)?;
    }
    save_memory(project_dir, &mem)?;
    Ok(())
}

pub fn volume_rollups_archive_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/volume_rollups_archive.jsonl")
}

pub fn append_volume_rollups_archive(project_dir: &Path, rolls: &[VolumeRollup]) -> Result<()> {
    if rolls.is_empty() {
        return Ok(());
    }
    let path = volume_rollups_archive_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    for r in rolls {
        let line = serde_json::to_string(r).context("serialize volume rollup archive")?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

pub fn load_volume_rollups_archive(project_dir: &Path) -> Vec<VolumeRollup> {
    let path = volume_rollups_archive_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut by_vol: std::collections::BTreeMap<u32, VolumeRollup> =
        std::collections::BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(r) = serde_json::from_str::<VolumeRollup>(line) {
            by_vol.insert(r.volume_index, r);
        }
    }
    by_vol.into_values().collect()
}

/// Hot + archived volume rollups (dedup by volume_index; hot wins).
pub fn load_all_volume_rollups(project_dir: &Path) -> Vec<VolumeRollup> {
    let mem = load_memory(project_dir);
    let mut by_vol: std::collections::BTreeMap<u32, VolumeRollup> =
        std::collections::BTreeMap::new();
    for r in load_volume_rollups_archive(project_dir) {
        by_vol.insert(r.volume_index, r);
    }
    for r in mem.volume_rollups {
        by_vol.insert(r.volume_index, r);
    }
    by_vol.into_values().collect()
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

/// Keyword-recall archived dangling threads (hot archive + cold jsonl).
pub fn recall_archived_threads(mem: &ProjectMemory, haystack: &str, limit: usize) -> Vec<OpenThread> {
    recall_archived_threads_in(None, mem, haystack, limit)
}

/// Like [`recall_archived_threads`], also searching `foreshadow_archive.jsonl`.
pub fn recall_archived_threads_in(
    project_dir: Option<&Path>,
    mem: &ProjectMemory,
    haystack: &str,
    limit: usize,
) -> Vec<OpenThread> {
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
    let mut pool: Vec<OpenThread> = mem.archived_threads.clone();
    if let Some(dir) = project_dir {
        for t in load_foreshadow_archive(dir) {
            if hot_ids.contains(t.id.as_str()) {
                continue;
            }
            if pool.iter().any(|a| a.id == t.id || a.text == t.text) {
                continue;
            }
            pool.push(t);
        }
    }
    let mut scored: Vec<(usize, OpenThread)> = Vec::new();
    for t in pool {
        if !(t.status == "open" || t.status.is_empty()) {
            continue;
        }
        if hot_ids.contains(t.id.as_str()) {
            continue;
        }
        let blob = t.text.as_str();
        let score = tokens.iter().filter(|tok| blob.contains(tok.as_str())).count();
        // Age boost: older planted lines win ties / get +1 after long dormancy.
        let age_bonus = if t.planted_chapter > 0 { 1 } else { 0 };
        if score > 0 {
            scored.push((score.saturating_add(age_bonus), t));
        }
    }
    // Prefer higher keyword score, then older planted (longform debt first).
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.planted_chapter.cmp(&b.1.planted_chapter))
    });
    scored.into_iter().take(limit).map(|(_, t)| t).collect()
}

/// Hot-zone cap for `asserted_facts` in `memory.json` (overflow → facts_archive.jsonl).
pub const HOT_FACTS_LIMIT: usize = 400;

pub fn facts_archive_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/facts_archive.jsonl")
}

/// Append facts to archive jsonl (one JSON object per line).
pub fn append_facts_archive(project_dir: &Path, facts: &[AssertedFact]) -> Result<()> {
    if facts.is_empty() {
        return Ok(());
    }
    let path = facts_archive_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    for fact in facts {
        let line = serde_json::to_string(fact).context("serialize archived fact")?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

/// Read archived facts (best-effort; skips bad lines).
pub fn load_facts_archive(project_dir: &Path) -> Vec<AssertedFact> {
    let path = facts_archive_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<AssertedFact>(l).ok())
        .collect()
}

/// Move oldest hot facts into archive so `asserted_facts.len() <= HOT_FACTS_LIMIT`.
pub fn prune_asserted_facts_to_archive(project_dir: &Path, mem: &mut ProjectMemory) -> Result<()> {
    if mem.asserted_facts.len() <= HOT_FACTS_LIMIT {
        return Ok(());
    }
    let skip = mem.asserted_facts.len() - HOT_FACTS_LIMIT;
    let overflow: Vec<AssertedFact> = mem.asserted_facts.drain(..skip).collect();
    append_facts_archive(project_dir, &overflow)?;
    Ok(())
}

fn score_fact(f: &AssertedFact, haystack: &str, roster_names: &[String]) -> usize {
    let mut score = 0usize;
    for name in roster_names {
        if name.is_empty() {
            continue;
        }
        if !f.entity.is_empty() && (f.entity == *name || f.entity.contains(name)) {
            score += 5;
        } else if f.text.contains(name) {
            score += 3;
        }
    }
    for tok in recall_tokens(haystack).into_iter().take(16) {
        if f.text.contains(&tok) {
            score += 1;
        }
    }
    if score == 0 {
        return 0;
    }
    // Confidence + light recency (only among already-relevant facts).
    let conf = if f.confidence.is_finite() {
        f.confidence.clamp(0.0, 1.0)
    } else {
        1.0
    };
    score = ((score as f32) * (0.5 + 0.5 * conf)).round() as usize;
    score = score.saturating_add((f.chapter / 50) as usize); // +1 per 50 chapters
    score.max(1)
}

/// Asserted facts whose text overlaps haystack / roster names.
pub fn select_asserted_facts_for_context(
    mem: &ProjectMemory,
    haystack: &str,
    roster_names: &[String],
    limit: usize,
) -> Vec<String> {
    select_asserted_facts_for_context_in(None, mem, haystack, roster_names, limit)
}

/// Like [`select_asserted_facts_for_context`], also topping up from `facts_archive.jsonl`.
pub fn select_asserted_facts_for_context_in(
    project_dir: Option<&Path>,
    mem: &ProjectMemory,
    haystack: &str,
    roster_names: &[String],
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(usize, AssertedFact)> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    for f in &mem.asserted_facts {
        let score = score_fact(f, haystack, roster_names);
        if score > 0 {
            seen_ids.insert(f.id.clone());
            scored.push((score, f.clone()));
        }
    }
    if scored.len() < limit {
        if let Some(dir) = project_dir {
            for f in load_facts_archive(dir) {
                if seen_ids.contains(&f.id) {
                    continue;
                }
                let score = score_fact(&f, haystack, roster_names);
                if score > 0 {
                    seen_ids.insert(f.id.clone());
                    scored.push((score, f));
                }
            }
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(
                b.1.confidence
                    .partial_cmp(&a.1.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(b.1.chapter.cmp(&a.1.chapter))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, f)| {
            format!(
                "[第{}章·c{:.1}] {}",
                f.chapter,
                f.confidence.clamp(0.0, 1.0),
                truncate_chars(&f.text, 100)
            )
        })
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

/// Entity-name–driven recall: prefer BM25 chapter index; fall back to directory scan.
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
    let indexed = crate::chapter_index::bm25_recall(
        project_dir,
        haystack,
        entity_names,
        exclude_chapters,
        limit,
    );
    if !indexed.is_empty() {
        return indexed;
    }
    recall_archived_summaries_scan(project_dir, haystack, entity_names, exclude_chapters, limit)
}

fn recall_archived_summaries_scan(
    project_dir: &Path,
    haystack: &str,
    entity_names: &[String],
    exclude_chapters: &[u32],
    limit: usize,
) -> Vec<ChapterDigest> {
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

pub fn recall_tokens(haystack: &str) -> Vec<String> {
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

pub fn entity_timeline_archive_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/entity_timeline.jsonl")
}

pub fn append_entity_timeline_archive(
    project_dir: &Path,
    facts: &[EntityTimelineFact],
) -> Result<()> {
    if facts.is_empty() {
        return Ok(());
    }
    let path = entity_timeline_archive_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    for fact in facts {
        let line = serde_json::to_string(fact).context("serialize entity timeline")?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

pub fn load_entity_timeline_archive(project_dir: &Path) -> Vec<EntityTimelineFact> {
    let path = entity_timeline_archive_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<EntityTimelineFact>(l).ok())
        .collect()
}

pub fn prune_entity_timeline_to_archive(
    project_dir: &Path,
    mem: &mut ProjectMemory,
) -> Result<()> {
    if mem.entity_timeline.len() <= HOT_ENTITY_TIMELINE_LIMIT {
        return Ok(());
    }
    let skip = mem.entity_timeline.len() - HOT_ENTITY_TIMELINE_LIMIT;
    let spilled: Vec<EntityTimelineFact> = mem.entity_timeline.drain(..skip).collect();
    append_entity_timeline_archive(project_dir, &spilled)?;
    Ok(())
}

fn push_entity_timeline(
    mem: &mut ProjectMemory,
    entity: &str,
    kind: &str,
    predicate: &str,
    text: &str,
    chapter: u32,
) {
    let entity = entity.trim();
    let text = text.trim();
    if entity.is_empty() || text.is_empty() {
        return;
    }
    if mem.entity_timeline.iter().any(|f| {
        f.entity == entity && f.text == text && f.chapter == chapter && f.predicate == predicate
    }) {
        return;
    }
    mem.entity_timeline.push(EntityTimelineFact {
        entity: entity.to_string(),
        kind: kind.to_string(),
        text: text.to_string(),
        chapter,
        predicate: predicate.to_string(),
    });
}

fn entity_roster_pairs(project_dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for group in ["characters", "items", "locations"] {
        let folder = project_dir.join("entities").join(group);
        for card in crate::cards::load_markdown_cards(&folder, group) {
            let name = if card.name.trim().is_empty() {
                card.slug.clone()
            } else {
                card.name.clone()
            };
            if !name.trim().is_empty() {
                out.push((group.trim_end_matches('s').to_string(), name.clone()));
            }
            if !card.slug.is_empty() && card.slug != name {
                out.push((group.trim_end_matches('s').to_string(), card.slug));
            }
        }
    }
    // Prefer longer names first for matching.
    out.sort_by(|a, b| b.1.chars().count().cmp(&a.1.chars().count()));
    out
}

fn ingest_entity_timeline_from_summary(
    project_dir: &Path,
    mem: &mut ProjectMemory,
    chapter: u32,
    v: &Value,
    key_facts: &[String],
) {
    let roster = entity_roster_pairs(project_dir);
    let match_entity = |blob: &str| -> Option<(String, String)> {
        for (kind, name) in &roster {
            if !name.is_empty() && blob.contains(name.as_str()) {
                return Some((kind.clone(), name.clone()));
            }
        }
        // Fallback: 「甲：…」 style prefixes (reject meta labels).
        if let Some((head, _)) = blob.split_once('：') {
            let head = head.trim();
            if (2..=12).contains(&head.chars().count())
                && !matches!(
                    head,
                    "伤势" | "能力位置" | "持有" | "状态" | "关系" | "事实" | "备注" | "摘要"
                )
            {
                return Some(("unknown".into(), head.to_string()));
            }
        }
        None
    };

    if let Some(bs) = v.get("body_state") {
        if let Some(arr) = bs.get("injuries").and_then(|x| x.as_array()) {
            for item in arr {
                let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                    continue;
                };
                // Genre-neutral: skip unattributed body_state lines.
                let Some((kind, entity)) = match_entity(s) else {
                    continue;
                };
                push_entity_timeline(mem, &entity, &kind, "injury", s, chapter);
            }
        }
        if let Some(arr) = bs.get("ability_loci").and_then(|x| x.as_array()) {
            for item in arr {
                let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                    continue;
                };
                let Some((kind, entity)) = match_entity(s) else {
                    continue;
                };
                push_entity_timeline(mem, &entity, &kind, "ability", s, chapter);
            }
        }
    }

    for fact in key_facts {
        let Some((kind, entity)) = match_entity(fact) else {
            continue;
        };
        let predicate = if fact.contains("持有") || fact.contains("失去") {
            "holding"
        } else if fact.contains("伤") {
            "injury"
        } else if fact.contains("关系") || fact.contains("敌对") || fact.contains("结盟") {
            "relation"
        } else {
            "fact"
        };
        push_entity_timeline(mem, &entity, &kind, predicate, fact, chapter);
    }

    let _ = prune_entity_timeline_to_archive(project_dir, mem);
}

/// Select entity timeline lines for CanonContext (hot + cold archive).
pub fn select_entity_timeline_for_context(
    project_dir: &Path,
    mem: &ProjectMemory,
    entity_names: &[String],
    limit: usize,
) -> Vec<String> {
    if limit == 0 || entity_names.is_empty() {
        return Vec::new();
    }
    // Allow single CJK name chars (e.g. 「甲」); keep ASCII min length 2.
    let names: Vec<&str> = entity_names
        .iter()
        .map(|s| s.trim())
        .filter(|s| {
            let n = s.chars().count();
            n >= 2 || (n == 1 && s.chars().next().is_some_and(|c| !c.is_ascii()))
        })
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    let mut pool = mem.entity_timeline.clone();
    for f in load_entity_timeline_archive(project_dir) {
        if pool.iter().any(|p| {
            p.entity == f.entity && p.text == f.text && p.chapter == f.chapter
        }) {
            continue;
        }
        pool.push(f);
    }
    let mut scored: Vec<(usize, EntityTimelineFact)> = Vec::new();
    for f in pool {
        let mut score = 0usize;
        for n in &names {
            if f.entity.contains(n) || f.text.contains(n) {
                score += 3;
            }
        }
        if score == 0 {
            continue;
        }
        // Prefer newer facts for the same entity, but keep older ones if room.
        score += f.chapter.min(50) as usize / 10;
        scored.push((score, f));
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.chapter.cmp(&a.1.chapter))
    });
    // Cap per entity so one character doesn't fill the whole budget.
    let mut per_entity: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    for (_, f) in scored {
        let n = per_entity.entry(f.entity.clone()).or_insert(0);
        if *n >= 3 {
            continue;
        }
        *n += 1;
        out.push(format!(
            "[{}/{}·第{}章] {}",
            f.entity,
            if f.predicate.is_empty() {
                "fact"
            } else {
                f.predicate.as_str()
            },
            f.chapter,
            truncate_chars(&f.text, 90)
        ));
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Longform health snapshot: foreshadow debt + volume ops (for Studio / Web).
pub fn longform_health_snapshot(project_dir: &Path) -> Value {
    let mem = load_memory(project_dir);
    let idx = crate::foreshadow::load_foreshadow_index(project_dir);
    let cold = load_foreshadow_archive(project_dir);
    let open_hot = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .count();
    let open_archived = mem
        .archived_threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .count();
    let open_cold = cold.len();
    // Index is authoritative after rebuild (hot + archived + open cold, deduped).
    // Studio / Web debt panel needs the full list (prompt context still uses dangling_show).
    let dangling_total = idx.dangling.len();
    let rolls = load_all_volume_rollups(project_dir);
    let state = crate::project::load_project_state(project_dir).ok();
    let published = state.as_ref().map(|s| s.published_count).unwrap_or(0);
    let current_for_debt = state
        .as_ref()
        .map(|s| s.next_chapter.max(1))
        .unwrap_or(1);
    let vol = crate::volume::active_volume_for_chapter(project_dir, published.max(1));
    let mid_threshold = crate::volume_audit_gate::mid_audit_threshold_resolved(project_dir);
    let thick_threshold = crate::volume_audit_gate::thick_volume_threshold_resolved(project_dir);
    let (vol_idx, vol_chapters) = if let Some(ref v) = vol {
        let (from, to) = crate::volume::volume_chapter_span(v, published.max(1));
        let mut n = 0u32;
        if from <= to {
            for ch in from..=to {
                let dir = project_dir.join("chapters").join(format!("{ch:03}"));
                if dir.join("summary.json").exists()
                    || dir.join("draft.md").exists()
                    || dir.join("draft.md.gz").exists()
                {
                    n += 1;
                }
            }
        }
        (v.volume_index, n)
    } else {
        (0u32, 0u32)
    };
    let has_audit = vol
        .as_ref()
        .map(|v| crate::volume_audit::volume_has_audit_report(project_dir, v.volume_index))
        .unwrap_or(false);
    let thick_warning = vol_chapters > thick_threshold;
    let soft_short_streak = state
        .as_ref()
        .map(|s| novelx_harness::consecutive_soft_short_from_meta(&s.meta))
        .unwrap_or(0);
    let target_chapters = state.as_ref().map(|s| s.target_chapters).unwrap_or(900);
    let word_budget = novelx_harness::ChapterBudget::default();
    // Rough progress: published × midpoint target vs target_chapters × midpoint.
    let target_mid = ((word_budget.word_min + word_budget.word_max) / 2) as u64;
    let est_chars = published as u64 * target_mid;
    let target_chars = target_chapters as u64 * target_mid;
    // Recent SoftShort rate from last 50 chapter dirs (body length vs soft min).
    let mut recent_soft = 0u32;
    let mut recent_ok = 0u32;
    let mut recent_hard = 0u32;
    let sample_from = published.saturating_sub(49).max(1);
    if published >= 1 {
        for ch in sample_from..=published {
            let Some(draft) = crate::project::read_chapter_draft(project_dir, ch)
                .or_else(|| crate::cold_archive::read_chapter_draft_resolved(project_dir, ch))
            else {
                continue;
            };
            let n = crate::schemas::draft_body_chars(&draft);
            match word_budget.assess_body_chars(n) {
                novelx_harness::LengthAssessment::HardShort => recent_hard += 1,
                novelx_harness::LengthAssessment::SoftShort => recent_soft += 1,
                novelx_harness::LengthAssessment::HardLong
                | novelx_harness::LengthAssessment::SoftLong
                | novelx_harness::LengthAssessment::Ok => recent_ok += 1,
            }
        }
    }
    let recent_n = recent_soft + recent_ok + recent_hard;
    let soft_short_rate = if recent_n == 0 {
        0.0
    } else {
        recent_soft as f64 / recent_n as f64
    };
    let drift_samples = count_jsonl_lines(&project_dir.join("lore/drift_samples.jsonl"));
    let lf = crate::volume_audit_gate::find_config_root(project_dir)
        .map(|r| novelx_harness::LongformConfig::load_from_config_root(&r))
        .unwrap_or_default();
    let debt = crate::foreshadow::foreshadow_debt_breakdown(
        &idx.dangling,
        current_for_debt,
        &lf.foreshadow_debt,
    );
    let mut oldest = idx.dangling.clone();
    oldest.sort_by(foreshadow_age_priority);
    let oldest_lines: Vec<Value> = oldest
        .iter()
        .map(|t| {
            let class = crate::foreshadow::classify_foreshadow_debt(
                t.planted_chapter,
                &t.horizon,
                &t.urgency,
                current_for_debt,
                &lf.foreshadow_debt,
            );
            serde_json::json!({
                "id": t.id,
                "text": truncate_chars(&t.text, 120),
                "planted_chapter": t.planted_chapter,
                "status": if t.status.is_empty() { "open" } else { &t.status },
                "horizon": t.horizon,
                "urgency": t.urgency,
                "debt_class": class.as_str(),
            })
        })
        .collect();
    serde_json::json!({
        "foreshadow": {
            "open_hot": open_hot,
            "open_archived": open_archived,
            "open_cold": open_cold,
            "dangling_total": dangling_total,
            "dangling_fresh": debt.fresh,
            "dangling_near": debt.near,
            "dangling_mid": debt.mid,
            "dangling_far": debt.far,
            "dangling_pressure": debt.pressure,
            "debt_current_chapter": current_for_debt,
            "oldest": oldest_lines,
        },
        "volume": {
            "active_index": vol_idx,
            "chapters_in_volume": vol_chapters,
            "mid_audit_threshold": mid_threshold,
            "thick_volume_threshold": thick_threshold,
            "has_audit_report": has_audit,
            "thick_volume_warning": thick_warning,
            "rollup_hot": mem.volume_rollups.len(),
            "rollup_total": rolls.len(),
        },
        "length": {
            "word_min": word_budget.word_min,
            "word_max": word_budget.word_max,
            "word_hard_min": word_budget.word_hard_min,
            "word_hard_max": word_budget.word_hard_max,
            "consecutive_soft_short": soft_short_streak,
            "recent_sampled": recent_n,
            "recent_soft_short": recent_soft,
            "recent_hard_short": recent_hard,
            "recent_ok": recent_ok,
            "soft_short_rate": soft_short_rate,
            "est_chars": est_chars,
            "target_chars": target_chars,
            "target_chapters": target_chapters,
        },
        "longform": {
            "quality_tier": lf.quality_tier.as_str(),
            "audit_tier": lf.audit_tier.as_str(),
            "impact_scan_mode": lf.impact_scan_mode.as_str(),
            "batch_max_chapters": lf.batch_max_chapters,
            "drift_samples": drift_samples,
        },
        "entity_timeline_hot": mem.entity_timeline.len(),
        "published_count": published,
    })
}

fn count_jsonl_lines(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    text.lines().filter(|l| !l.trim().is_empty()).count()
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
                ..Default::default()
            });
        }
        prune_open_threads_into_archive(None, &mut mem).unwrap();
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
    fn foreshadow_cold_archive_can_resolve() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-fs-resolve-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        append_foreshadow_archive(
            &dir,
            &[OpenThread {
                id: "cold_old".into(),
                text: "远古驿站密信尚未揭开".into(),
                status: "open".into(),
                planted_chapter: 2,
                ..Default::default()
            }],
        )
        .unwrap();
        let n = resolve_foreshadow_archive(&dir, 90, |t| t.id == "cold_old").unwrap();
        assert_eq!(n, 1);
        let open = load_foreshadow_archive(&dir);
        assert!(open.is_empty(), "resolved cold thread must leave open list");
        let rows = load_foreshadow_archive_rows(&dir);
        assert!(
            rows.iter()
                .any(|t| t.id == "cold_old" && t.status == "resolved"),
            "resolved row must remain on disk: {rows:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn foreshadow_cold_archive_never_hard_drops() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-fs-cold-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let mut mem = ProjectMemory::default();
        for i in 1..=(ARCHIVED_THREAD_LIMIT as u32 + OPEN_THREAD_LIMIT as u32 + 5) {
            mem.open_threads.push(OpenThread {
                id: format!("t{i}"),
                text: format!("长线伏笔{i}驿站密信"),
                status: "open".into(),
                planted_chapter: i,
                ..Default::default()
            });
        }
        prune_open_threads_into_archive(Some(&dir), &mut mem).unwrap();
        assert!(mem.archived_threads.len() <= ARCHIVED_THREAD_LIMIT);
        let cold = load_foreshadow_archive(&dir);
        assert!(
            !cold.is_empty(),
            "overflow must spill to foreshadow_archive.jsonl"
        );
        assert!(
            cold.iter().any(|t| t.text.contains("伏笔1")),
            "oldest line must survive in cold archive"
        );
        let recalled = recall_archived_threads_in(Some(&dir), &mem, "驿站 密信", 8);
        assert!(
            recalled.iter().any(|t| t.text.contains("伏笔1") || t.text.contains("密信")),
            "cold archive must be recallable: {recalled:?}"
        );
        let boosted = select_dangling_age_boosted(
            &mem
                .open_threads
                .iter()
                .chain(mem.archived_threads.iter())
                .chain(cold.iter())
                .cloned()
                .collect::<Vec<_>>(),
            5,
        );
        assert!(
            boosted.first().map(|t| t.planted_chapter).unwrap_or(999) <= 5,
            "age boost must prefer oldest: {boosted:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn entity_timeline_ingest_and_select() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-entity-tl-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("entities/characters")).unwrap();
        fs::create_dir_all(dir.join("lore")).unwrap();
        fs::write(
            dir.join("entities/characters/甲.md"),
            "---\nname: 甲\ncomplete: true\n---\n简介\n",
        )
        .unwrap();
        let summary = r#"{
            "event_summary": "甲负伤撤退。",
            "ending_hook": "远处有火光。",
            "new_facts": ["甲持有旧钥"],
            "body_state": {"injuries": ["甲左臂骨裂"], "ability_loci": ["无关无主条目"]}
        }"#;
        apply_summary_json(&dir, 3, summary).unwrap();
        let mem = load_memory(&dir);
        assert!(
            mem.entity_timeline.iter().any(|f| f.entity.contains("甲")),
            "timeline should capture entity: {:?}",
            mem.entity_timeline
        );
        assert!(
            !mem.entity_timeline
                .iter()
                .any(|f| f.text.contains("无关无主")),
            "unattributed body_state must be skipped: {:?}",
            mem.entity_timeline
        );
        let lines = select_entity_timeline_for_context(&dir, &mem, &["甲".into()], 6);
        assert!(
            lines.iter().any(|s| s.contains("甲")),
            "context select should surface timeline: {lines:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn longform_health_returns_all_dangling_foreshadows() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-lf-health-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let mut mem = ProjectMemory::default();
        for i in 1..=12 {
            mem.open_threads.push(OpenThread {
                id: format!("t{i}"),
                text: format!("伏笔线索{i}"),
                status: "open".into(),
                planted_chapter: i,
                ..Default::default()
            });
        }
        save_memory(&dir, &mem).unwrap();
        let _ = crate::foreshadow::rebuild_foreshadow_index(&dir);
        // next_chapter defaults to 1 when no state.json → most plants look "future";
        // still expose tier fields for Studio.
        let snap = longform_health_snapshot(&dir);
        let fs = &snap["foreshadow"];
        assert_eq!(fs["dangling_total"], 12);
        assert!(fs.get("dangling_pressure").is_some());
        assert!(fs.get("dangling_fresh").is_some());
        assert!(fs.get("dangling_far").is_some());
        let oldest = fs["oldest"].as_array().expect("oldest array");
        assert_eq!(oldest.len(), 12, "studio debt list must not truncate: {oldest:?}");
        assert_eq!(oldest[0]["planted_chapter"], 1);
        assert!(oldest[0].get("debt_class").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bury_skips_cold_archive_duplicates() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-bury-dedup-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        append_foreshadow_archive(
            &dir,
            &[OpenThread {
                id: "cold_dup".into(),
                text: "矿井下仍有回声".into(),
                status: "open".into(),
                planted_chapter: 4,
                ..Default::default()
            }],
        )
        .unwrap();
        let mem = ProjectMemory::default();
        assert!(foreshadow_already_known(
            &dir,
            &mem,
            None,
            "矿井下仍有回声"
        ));
        let summary = r#"{
            "event_summary": "继续追查。",
            "ending_hook": "风起。",
            "new_facts": [],
            "foreshadow_updates": ["矿井下仍有回声"]
        }"#;
        apply_summary_json(&dir, 10, summary).unwrap();
        let mem = load_memory(&dir);
        assert!(
            !mem.open_threads.iter().any(|t| t.text.contains("矿井")),
            "must not re-bury cold open thread into hot list: {:?}",
            mem.open_threads
        );
        let _ = fs::remove_dir_all(&dir);
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

    #[test]
    fn facts_overflow_archives_and_recalls() {
        let dir = std::env::temp_dir().join("novelx-facts-archive");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let mut mem = ProjectMemory {
            version: 1,
            ..Default::default()
        };
        for i in 0..(HOT_FACTS_LIMIT + 5) {
            mem.asserted_facts.push(AssertedFact {
                id: format!("fact_{i}"),
                text: if i < 5 {
                    format!("旧事实{i}：主角曾路过驿站")
                } else {
                    format!("新事实{i}：无关条目填充")
                },
                chapter: (i as u32) + 1,
                source: "test".into(),
                entity: if i < 5 { "主角".into() } else { String::new() },
                confidence: 1.0,
            });
        }
        prune_asserted_facts_to_archive(&dir, &mut mem).unwrap();
        assert_eq!(mem.asserted_facts.len(), HOT_FACTS_LIMIT);
        let archived = load_facts_archive(&dir);
        assert_eq!(archived.len(), 5);
        save_memory(&dir, &mem).unwrap();
        let picked = select_asserted_facts_for_context_in(
            Some(&dir),
            &mem,
            "主角 驿站",
            &["主角".into()],
            3,
        );
        assert!(
            picked.iter().any(|s| s.contains("驿站")),
            "archive recall should surface old facts: {picked:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
