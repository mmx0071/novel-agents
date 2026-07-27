//! Volume (卷) bounds and termination — chapter count is open-ended;
//! a volume ends when its outline ending conditions are satisfied.

use anyhow::Result;
use novelx_llm::LlmClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Soft default only when acts lack any structure (not used as hard end).
const DEFAULT_CHAPTERS_PER_VOLUME: u32 = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VolumeBound {
    pub volume_index: u32,
    /// First chapter of this volume (0 = unknown / inherit).
    pub start_chapter: u32,
    /// Soft hint or recorded end after completion (0 = still open-ended).
    pub end_chapter: u32,
    #[serde(default)]
    pub name: String,
    /// Explicit termination criteria from 卷纲 / story_outline.ending_condition.
    #[serde(default)]
    pub ending_conditions: Vec<String>,
    #[serde(default)]
    pub goal: String,
    /// Marked true after ending conditions fire and volume sync gate runs.
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone)]
pub struct VolumeEndDecision {
    pub volume: VolumeBound,
    pub matched: Vec<String>,
    pub reason: String,
}

/// Load all known volume bounds for a project.
pub fn load_volume_bounds(project_dir: &Path) -> Vec<VolumeBound> {
    let mut bounds = Vec::new();

    // 1) Structured acts in story_outline.json (primary)
    let outline_path = project_dir.join("artifacts/story_outline.json");
    if let Ok(text) = std::fs::read_to_string(&outline_path) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(acts) = v.get("acts").and_then(|a| a.as_array()) {
                for act in acts {
                    if let Some(b) = bound_from_act(act) {
                        bounds.push(b);
                    }
                }
            }
        }
    }

    // 2) Parse markdown 卷纲（per-volume files under arc_outlines/ + legacy arc_outline.md）
    // Arc file is authoritative for name + ending_conditions (story_outline acts often drift).
    migrate_arc_outlines(project_dir);
    for vi in list_arc_outline_volumes(project_dir) {
        if let Some(text) = read_arc_outline_text(project_dir, vi) {
            for b in parse_volume_meta_from_markdown(&text) {
                force_upsert_arc_bound(&mut bounds, b);
            }
        }
    }

    // 3) Fallback: invent open-ended volumes from volume_index list
    if bounds.is_empty() {
        let max_vi = max_volume_index(project_dir).unwrap_or(1);
        for vi in 1..=max_vi {
            bounds.push(default_bound(vi));
        }
    }

    bounds.sort_by_key(|b| b.volume_index);
    bounds
}

fn bound_from_act(act: &Value) -> Option<VolumeBound> {
    let vi = act
        .get("volume_index")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    if vi == 0 {
        return None;
    }
    let name = act
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let goal = act
        .get("goal")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let start_chapter = act
        .get("start_chapter")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let end_chapter = act
        .get("end_chapter")
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let completed = act
        .get("completed")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
        || act.get("status").and_then(|x| x.as_str()) == Some("completed");
    let mut ending_conditions = parse_ending_condition_field(act.get("ending_condition"));
    if ending_conditions.is_empty() {
        if let Some(arr) = act.get("ending_conditions").and_then(|x| x.as_array()) {
            ending_conditions = arr
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Some(VolumeBound {
        volume_index: vi,
        start_chapter,
        end_chapter,
        name,
        ending_conditions,
        goal,
        completed,
    })
}

fn parse_ending_condition_field(v: Option<&Value>) -> Vec<String> {
    let Some(v) = v else {
        return vec![];
    };
    if let Some(s) = v.as_str() {
        return split_ending_conditions(s);
    }
    if let Some(arr) = v.as_array() {
        return arr
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    vec![]
}

/// Split prose / bullet ending_condition into discrete criteria.
pub fn split_ending_conditions(text: &str) -> Vec<String> {
    let t = text.trim();
    if t.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    for line in t.lines() {
        let line = line
            .trim()
            .trim_start_matches(['-', '*', '•', '·'])
            .trim();
        if line.is_empty() {
            continue;
        }
        // Also split on Chinese semicolons within a line
        for part in line.split(['；', ';']) {
            let p = part.trim();
            if !p.is_empty() {
                out.push(p.to_string());
            }
        }
    }
    if out.is_empty() && !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// Volume currently being written for `chapter` (incomplete act preferred).
pub fn active_volume_for_chapter(project_dir: &Path, chapter: u32) -> Option<VolumeBound> {
    let bounds = load_volume_bounds(project_dir);
    active_volume_from_bounds(&bounds, chapter)
}

pub fn active_volume_from_bounds(bounds: &[VolumeBound], chapter: u32) -> Option<VolumeBound> {
    if bounds.is_empty() {
        return None;
    }
    // Prefer incomplete volume whose soft range contains chapter
    for b in bounds {
        if b.completed {
            continue;
        }
        let start = if b.start_chapter == 0 {
            1
        } else {
            b.start_chapter
        };
        let in_soft_range = chapter >= start
            && (b.end_chapter == 0 || chapter <= b.end_chapter);
        if in_soft_range {
            return Some(b.clone());
        }
    }
    // First incomplete volume
    if let Some(b) = bounds.iter().find(|b| !b.completed) {
        return Some(b.clone());
    }
    // All completed: volume of chapter by recorded ranges
    bounds
        .iter()
        .find(|b| {
            let start = if b.start_chapter == 0 { 1 } else { b.start_chapter };
            let end = if b.end_chapter == 0 {
                u32::MAX
            } else {
                b.end_chapter
            };
            chapter >= start && chapter <= end
        })
        .cloned()
}

/// Legacy: fixed end_chapter equals published_count (only when no ending conditions).
pub fn volume_just_ended(published_count: u32, bounds: &[VolumeBound]) -> Option<VolumeBound> {
    if published_count == 0 {
        return None;
    }
    bounds
        .iter()
        .find(|b| {
            !b.completed
                && b.ending_conditions.is_empty()
                && b.end_chapter > 0
                && b.end_chapter == published_count
        })
        .cloned()
}

pub fn bound_for_volume(bounds: &[VolumeBound], volume_index: u32) -> Option<VolumeBound> {
    bounds
        .iter()
        .find(|b| b.volume_index == volume_index)
        .cloned()
        .or_else(|| {
            if volume_index > 0 {
                Some(default_bound(volume_index))
            } else {
                None
            }
        })
}

pub fn default_bound(volume_index: u32) -> VolumeBound {
    let start = (volume_index.saturating_sub(1)) * DEFAULT_CHAPTERS_PER_VOLUME + 1;
    VolumeBound {
        volume_index,
        start_chapter: start,
        end_chapter: 0, // open-ended
        name: format!("第{volume_index}卷"),
        ending_conditions: vec![],
        goal: String::new(),
        completed: false,
    }
}

/// Inclusive chapter span for gathering summaries (open end → up to `upto_chapter`).
pub fn volume_chapter_span(volume: &VolumeBound, upto_chapter: u32) -> (u32, u32) {
    let start = if volume.start_chapter == 0 {
        1
    } else {
        volume.start_chapter
    };
    let end = if volume.end_chapter > 0 {
        volume.end_chapter.min(upto_chapter.max(start))
    } else {
        upto_chapter.max(start)
    };
    (start, end)
}

/// After a chapter is published: decide whether this volume's ending conditions are met.
pub async fn evaluate_volume_end(
    project_dir: &Path,
    chapter: u32,
    llm: &LlmClient,
) -> Result<Option<VolumeEndDecision>> {
    let Some(volume) = active_volume_for_chapter(project_dir, chapter) else {
        return Ok(None);
    };
    if volume.completed {
        return Ok(None);
    }

    // Hard gate: one local plot card ≠ volume end. Still-open plot work blocks sync gate.
    if crate::plots::volume_has_open_plot_work(project_dir, volume.volume_index) {
        tracing::info!(
            chapter,
            volume = volume.volume_index,
            "volume end skipped: open plot cards / unfinished next_plot"
        );
        return Ok(None);
    }

    // Primary: explicit ending conditions → LLM judge against chapter evidence
    if !volume.ending_conditions.is_empty() {
        let evidence = gather_end_evidence(project_dir, &volume, chapter);
        let decision = llm_judge_ending(llm, &volume, chapter, &evidence).await?;
        if decision.ended {
            // Re-check after judge — plots may have been mis-read; never end with open work.
            if crate::plots::volume_has_open_plot_work(project_dir, volume.volume_index) {
                return Ok(None);
            }
            let mut vol = volume;
            vol.end_chapter = chapter;
            return Ok(Some(VolumeEndDecision {
                volume: vol,
                matched: decision.matched,
                reason: decision.reason,
            }));
        }
        return Ok(None);
    }

    // Legacy fallback: soft fixed end_chapter with no conditions — still respect plot gate.
    if volume.end_chapter > 0 && chapter == volume.end_chapter {
        if crate::plots::volume_has_open_plot_work(project_dir, volume.volume_index) {
            return Ok(None);
        }
        return Ok(Some(VolumeEndDecision {
            volume: volume.clone(),
            matched: vec![format!("软边界：第{}章（无终止条件时的兼容）", chapter)],
            reason: "卷纲未写终止条件，命中旧版 end_chapter".into(),
        }));
    }
    Ok(None)
}

struct JudgeResult {
    ended: bool,
    matched: Vec<String>,
    reason: String,
}

async fn llm_judge_ending(
    llm: &LlmClient,
    volume: &VolumeBound,
    chapter: u32,
    evidence: &str,
) -> Result<JudgeResult> {
    let conditions = volume
        .ending_conditions
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {c}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let system = r#"你是卷末判定员。根据本章及本卷近期摘要，判断卷纲「终止条件」是否已实质达成。
只输出 JSON：{"ended":true|false,"matched":["命中的条件原文或编号"],"reason":"一句话理由"}
规则：
- 必须**多数（过半）**关键终止条件已在正文/摘要中兑现才 ended=true（不必字字相同，语义达成即可）
- 仅开局卡/铺垫、未完成高潮/团队分裂/卷末抉择 → ended=false
- 一张剧情卡收束 ≠ 整卷结束；不要因为「推进很多」或章数少而判定结束
- 证据不足以覆盖终止条件列表时必须 ended=false"#;
    let user = format!(
        "第{}卷「{}」刚发布第{}章。\n卷目标：{}\n\n# 终止条件\n{}\n\n# 证据（摘要）\n{}\n\n只输出 JSON。",
        volume.volume_index,
        volume.name,
        chapter,
        volume.goal,
        conditions,
        evidence
    );
    let raw = match llm
        .complete_for_agent("arc_planner", system, &user)
        .await
    {
        Ok(s) if !s.trim().is_empty() => s,
        _ => {
            llm.complete(
                system,
                &user,
                Some(&llm.model_for_agent("summarizer")),
            )
            .await?
        }
    };
    let v = extract_json_value(&raw).unwrap_or_else(|| json!({"ended": false}));
    let ended = v.get("ended").and_then(|x| x.as_bool()).unwrap_or(false);
    let matched = v
        .get("matched")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let reason = v
        .get("reason")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Ok(JudgeResult {
        ended,
        matched,
        reason,
    })
}

fn gather_end_evidence(project_dir: &Path, volume: &VolumeBound, chapter: u32) -> String {
    let (start, _) = volume_chapter_span(volume, chapter);
    let from = start.max(chapter.saturating_sub(4));
    let mut parts = Vec::new();
    for ch in from..=chapter {
        let path = project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("summary.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let excerpt: String = text.chars().take(1500).collect();
            parts.push(format!("## 第{ch}章\n{excerpt}"));
        }
    }
    if parts.is_empty() {
        let draft = project_dir
            .join("chapters")
            .join(format!("{chapter:03}"))
            .join("draft.md");
        if let Ok(text) = std::fs::read_to_string(draft) {
            let excerpt: String = text.chars().take(2500).collect();
            parts.push(format!("## 第{chapter}章正文节选\n{excerpt}"));
        }
    }
    if parts.is_empty() {
        "（无摘要/正文）".into()
    } else {
        parts.join("\n\n")
    }
}

fn extract_json_value(raw: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return Some(v);
    }
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&raw[start..=end]).ok()
}

/// Parse volume meta from arc markdown: ending conditions + optional soft chapter hints.
pub fn parse_volume_meta_from_markdown(text: &str) -> Vec<VolumeBound> {
    let mut out: Vec<VolumeBound> = Vec::new();
    let mut vol_hint: u32 = 0;
    let mut current_name = String::new();
    let mut in_ending = false;
    let mut ending_buf: Vec<String> = Vec::new();
    let mut soft_start = 0u32;
    let mut soft_end = 0u32;

    let flush = |out: &mut Vec<VolumeBound>,
                 vol_hint: u32,
                 name: &str,
                 ending: &mut Vec<String>,
                 soft_start: u32,
                 soft_end: u32| {
        if vol_hint == 0 && ending.is_empty() && soft_end == 0 {
            return;
        }
        let vi = if vol_hint > 0 {
            vol_hint
        } else {
            (out.len() as u32) + 1
        };
        let mut b = VolumeBound {
            volume_index: vi,
            start_chapter: soft_start,
            end_chapter: soft_end,
            name: if name.is_empty() {
                format!("第{vi}卷")
            } else {
                name.to_string()
            },
            ending_conditions: ending.drain(..).collect(),
            goal: String::new(),
            completed: false,
        };
        // Soft ranges from「第A–B章」are hints only; clear hard end so conditions own termination
        if !b.ending_conditions.is_empty() {
            b.end_chapter = 0;
        }
        upsert_bound(out, b);
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            if let Some(vi) = extract_volume_index(trimmed) {
                flush(
                    &mut out,
                    vol_hint,
                    &current_name,
                    &mut ending_buf,
                    soft_start,
                    soft_end,
                );
                vol_hint = vi;
                current_name = extract_volume_name(trimmed).unwrap_or_default();
                in_ending = false;
                soft_start = 0;
                soft_end = 0;
            }
        }
        let heading = trimmed.trim_start_matches('#').trim();
        if heading.contains("终止条件")
            || heading.contains("卷末交付")
            || heading.contains("卷末收束")
            || heading == "卷末条件"
        {
            in_ending = true;
            continue;
        }
        if trimmed.starts_with('#') && in_ending {
            in_ending = false;
        }
        if in_ending {
            let item = trimmed
                .trim_start_matches(['-', '*', '•', '·'])
                .trim();
            if !item.is_empty() && !item.starts_with('#') {
                ending_buf.push(item.to_string());
            }
            continue;
        }
        if let Some(vi) = extract_volume_index(trimmed) {
            vol_hint = vi;
        }
        if let Some((start, end)) = extract_chapter_range(trimmed) {
            soft_start = start;
            soft_end = end;
            if let Some(n) = extract_volume_name(trimmed) {
                current_name = n;
            }
        }
        // Inline: **终止条件**：…
        if let Some(rest) = trimmed
            .strip_prefix("终止条件：")
            .or_else(|| trimmed.strip_prefix("终止条件:"))
            .or_else(|| {
                trimmed
                    .find("终止条件")
                    .and_then(|i| {
                        let after = &trimmed[i + "终止条件".len()..];
                        after.strip_prefix('：').or_else(|| after.strip_prefix(':'))
                    })
            })
        {
            for c in split_ending_conditions(rest) {
                ending_buf.push(c);
            }
        }
    }
    flush(
        &mut out,
        vol_hint,
        &current_name,
        &mut ending_buf,
        soft_start,
        soft_end,
    );
    out
}

/// Backward-compatible alias used by tools.
pub fn parse_bounds_from_markdown(text: &str) -> Vec<VolumeBound> {
    parse_volume_meta_from_markdown(text)
}

fn extract_volume_index(line: &str) -> Option<u32> {
    if !(line.contains('第') && line.contains('卷')) {
        return None;
    }
    for (i, _) in line.char_indices() {
        if line[i..].starts_with('第') {
            let rest = &line[i + '第'.len_utf8()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() && rest[digits.len()..].starts_with('卷') {
                return digits.parse().ok();
            }
        }
    }
    None
}

fn extract_chapter_range(line: &str) -> Option<(u32, u32)> {
    // 第1–15章 / 第1-15章 / 第1—15章
    for (i, _) in line.char_indices() {
        if !line[i..].starts_with('第') {
            continue;
        }
        let rest = &line[i + '第'.len_utf8()..];
        let a: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if a.is_empty() {
            continue;
        }
        let after_a = &rest[a.len()..];
        let sep_len = if after_a.starts_with('–') {
            '–'.len_utf8()
        } else if after_a.starts_with('—') {
            '—'.len_utf8()
        } else if after_a.starts_with('-') {
            1
        } else if after_a.starts_with('~') || after_a.starts_with('～') {
            after_a.chars().next().map(|c| c.len_utf8()).unwrap_or(1)
        } else {
            continue;
        };
        let after_sep = &after_a[sep_len..];
        let b: String = after_sep.chars().take_while(|c| c.is_ascii_digit()).collect();
        if b.is_empty() {
            continue;
        }
        let after_b = &after_sep[b.len()..];
        if after_b.starts_with('章') {
            let start: u32 = a.parse().ok()?;
            let end: u32 = b.parse().ok()?;
            if end >= start {
                return Some((start, end));
            }
        }
    }
    None
}

fn extract_volume_name(line: &str) -> Option<String> {
    let t = line.trim().trim_start_matches('#').trim();
    // 第1卷 · 裂隙呼吸
    if let Some(idx) = t.find('·').or_else(|| t.find('：')).or_else(|| t.find(':')) {
        let name = t[idx + t[idx..].chars().next()?.len_utf8()..].trim();
        let name = name
            .split(['（', '('])
            .next()
            .unwrap_or(name)
            .trim();
        if name.chars().count() > 1 && name.chars().count() < 40 {
            return Some(name.to_string());
        }
    }
    let cut = t.find('（').or_else(|| t.find('(')).unwrap_or(t.len());
    let name = t[..cut].trim();
    if name.chars().count() > 1 && name.chars().count() < 40 {
        Some(name.to_string())
    } else {
        None
    }
}

fn upsert_bound(bounds: &mut Vec<VolumeBound>, b: VolumeBound) {
    if let Some(existing) = bounds.iter_mut().find(|x| x.volume_index == b.volume_index) {
        if existing.name.is_empty() && !b.name.is_empty() {
            existing.name = b.name.clone();
        }
        if existing.goal.is_empty() && !b.goal.is_empty() {
            existing.goal = b.goal.clone();
        }
        if existing.ending_conditions.is_empty() && !b.ending_conditions.is_empty() {
            existing.ending_conditions = b.ending_conditions.clone();
        } else if !b.ending_conditions.is_empty()
            && b.ending_conditions.len() > existing.ending_conditions.len()
        {
            existing.ending_conditions = b.ending_conditions.clone();
        }
        if existing.start_chapter == 0 && b.start_chapter > 0 {
            existing.start_chapter = b.start_chapter;
        }
        // Prefer open-ended when conditions exist; else keep soft end hint
        if !existing.ending_conditions.is_empty() {
            existing.end_chapter = 0;
        } else if existing.end_chapter == 0 && b.end_chapter > 0 {
            existing.end_chapter = b.end_chapter;
        }
        if b.completed {
            existing.completed = true;
        }
    } else {
        bounds.push(b);
    }
}

/// Arc outline wins for termination criteria / display name (not `completed` flag).
fn force_upsert_arc_bound(bounds: &mut Vec<VolumeBound>, b: VolumeBound) {
    if let Some(existing) = bounds.iter_mut().find(|x| x.volume_index == b.volume_index) {
        if !b.name.is_empty() {
            existing.name = b.name.clone();
        }
        if !b.ending_conditions.is_empty() {
            existing.ending_conditions = b.ending_conditions.clone();
            existing.end_chapter = 0;
        }
        if existing.start_chapter == 0 && b.start_chapter > 0 {
            existing.start_chapter = b.start_chapter;
        }
        if existing.goal.is_empty() && !b.goal.is_empty() {
            existing.goal = b.goal.clone();
        }
    } else {
        bounds.push(b);
    }
}

/// Rename legacy `arc_planner.md` → `arc_outline.md` once (no-op if outline exists).
pub fn migrate_legacy_arc_planner(project_dir: &Path) {
    let art = project_dir.join("artifacts");
    let outline = art.join("arc_outline.md");
    let planner = art.join("arc_planner.md");
    if outline.exists() {
        // Prefer canonical; drop duplicate legacy file if both exist.
        if planner.exists() {
            let _ = std::fs::remove_file(&planner);
        }
        return;
    }
    if planner.exists() {
        let _ = std::fs::rename(&planner, &outline);
    }
}

/// Per-volume 卷纲 directory: `artifacts/arc_outlines/{NN}.md`.
pub fn arc_outline_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("artifacts/arc_outlines")
}

pub fn arc_outline_path(project_dir: &Path, volume: u32) -> PathBuf {
    arc_outline_dir(project_dir).join(format!("{:02}.md", volume.max(1)))
}

/// Migrate legacy single `arc_outline.md` / `arc_planner.md` into per-volume files.
/// Idempotent: existing `arc_outlines/{NN}.md` are kept; legacy file is copied once then removed.
pub fn migrate_arc_outlines(project_dir: &Path) {
    migrate_legacy_arc_planner(project_dir);
    let legacy = project_dir.join("artifacts/arc_outline.md");
    if !legacy.is_file() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&legacy) else {
        return;
    };
    if text.trim().chars().count() < 20 {
        let _ = std::fs::remove_file(&legacy);
        return;
    }
    let vol = parse_volume_meta_from_markdown(&text)
        .into_iter()
        .next()
        .map(|b| b.volume_index)
        .or_else(|| {
            text.lines()
                .find(|l| l.trim_start().starts_with('#'))
                .and_then(|l| extract_volume_index(l.trim()))
        })
        .unwrap_or(1)
        .max(1);
    let dest = arc_outline_path(project_dir, vol);
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !dest.is_file() {
        let _ = std::fs::write(&dest, &text);
    }
    // Drop flat file so UI/tools cannot treat a single overwritten blob as "the" 卷纲.
    let _ = std::fs::remove_file(&legacy);
}

pub fn list_arc_outline_volumes(project_dir: &Path) -> Vec<u32> {
    migrate_arc_outlines(project_dir);
    let dir = arc_outline_dir(project_dir);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(n) = stem.parse::<u32>() {
            if n >= 1 {
                out.push(n);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn has_any_arc_outline(project_dir: &Path) -> bool {
    !list_arc_outline_volumes(project_dir).is_empty()
        || path_nonempty_file(&project_dir.join("artifacts/arc_outline.md"))
}

fn path_nonempty_file(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|t| t.trim().chars().count() > 20)
        .unwrap_or(false)
}

pub fn read_arc_outline_text(project_dir: &Path, volume: u32) -> Option<String> {
    migrate_arc_outlines(project_dir);
    let path = arc_outline_path(project_dir, volume);
    let text = std::fs::read_to_string(&path).ok()?;
    let trimmed = text.trim();
    if trimmed.chars().count() < 20 {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn write_arc_outline_text(project_dir: &Path, volume: u32, text: &str) -> anyhow::Result<()> {
    migrate_arc_outlines(project_dir);
    let path = arc_outline_path(project_dir, volume.max(1));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text)?;
    // Remove legacy flat file if present (prevents "vol2 overwrote vol1" illusions).
    let _ = std::fs::remove_file(project_dir.join("artifacts/arc_outline.md"));
    Ok(())
}

/// Best-effort volume for plot/context reads: active volume for `next_chapter`, else latest file.
pub fn resolve_arc_outline_volume(project_dir: &Path, hint: Option<u32>) -> u32 {
    if let Some(v) = hint.filter(|n| *n >= 1) {
        return v;
    }
    let chapter = crate::load_project_state(project_dir)
        .ok()
        .map(|s| s.next_chapter.max(1))
        .unwrap_or(1);
    if let Some(b) = active_volume_for_chapter(project_dir, chapter) {
        return b.volume_index.max(1);
    }
    list_arc_outline_volumes(project_dir)
        .into_iter()
        .next_back()
        .unwrap_or(1)
}

/// Preview rows for the reader: one tab per volume outline.
pub fn list_arc_outlines_for_preview(project_dir: &Path) -> Vec<serde_json::Value> {
    migrate_arc_outlines(project_dir);
    let mut rows = Vec::new();
    for vi in list_arc_outline_volumes(project_dir) {
        let Some(text) = read_arc_outline_text(project_dir, vi) else {
            continue;
        };
        let title = text
            .lines()
            .find(|l| l.trim_start().starts_with('#'))
            .map(|l| l.trim().trim_start_matches('#').trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("第{vi}卷"));
        rows.push(serde_json::json!({
            "volume": vi,
            "title": title,
            "markdown": crate::display_arc_outline(&text),
        }));
    }
    rows
}

fn max_volume_index(project_dir: &Path) -> Option<u32> {
    let outline_path = project_dir.join("artifacts/story_outline.json");
    let text = std::fs::read_to_string(outline_path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("acts")?
        .as_array()?
        .iter()
        .filter_map(|a| a.get("volume_index").and_then(|x| x.as_u64()).map(|n| n as u32))
        .max()
}

/// Persist bounds / ending conditions onto matching act.
pub fn sync_act_chapter_bounds(project_dir: &Path, bound: &VolumeBound) -> anyhow::Result<()> {
    sync_act_fields(project_dir, bound, false)
}

/// Undo a mistaken volume completion so writing can continue on this volume.
pub fn reopen_volume_act(project_dir: &Path, volume_index: u32) -> anyhow::Result<()> {
    let path = project_dir.join("artifacts/story_outline.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let Some(acts) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("acts"))
        .and_then(|a| a.as_array_mut())
    else {
        return Ok(());
    };
    for act in acts.iter_mut() {
        let vi = act
            .get("volume_index")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32;
        if vi != volume_index {
            continue;
        }
        if let Some(obj) = act.as_object_mut() {
            obj.insert("completed".into(), Value::Bool(false));
            obj.insert("status".into(), Value::String("in_progress".into()));
            // Clear hard end so open-ended writing resumes under 卷纲终止条件.
            obj.insert("end_chapter".into(), Value::from(0u32));
        }
        break;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

/// Mark volume completed at `end_chapter` and open next act start.
pub fn mark_volume_completed(
    project_dir: &Path,
    bound: &VolumeBound,
    end_chapter: u32,
) -> anyhow::Result<()> {
    let mut b = bound.clone();
    b.end_chapter = end_chapter;
    b.completed = true;
    if b.start_chapter == 0 {
        b.start_chapter = 1;
    }
    sync_act_fields(project_dir, &b, true)?;
    // Open next volume start_chapter
    let next_vi = bound.volume_index + 1;
    let path = project_dir.join("artifacts/story_outline.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    if let Some(acts) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("acts"))
        .and_then(|a| a.as_array_mut())
    {
        for act in acts.iter_mut() {
            let vi = act
                .get("volume_index")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            if vi == next_vi {
                if let Some(obj) = act.as_object_mut() {
                    let cur = obj
                        .get("start_chapter")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    if cur == 0 {
                        obj.insert("start_chapter".into(), Value::from(end_chapter + 1));
                    }
                    obj.insert("completed".into(), Value::Bool(false));
                }
                break;
            }
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

fn sync_act_fields(
    project_dir: &Path,
    bound: &VolumeBound,
    set_completed: bool,
) -> anyhow::Result<()> {
    let path = project_dir.join("artifacts/story_outline.json");
    let mut root: Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        json!({"acts": [], "markdown": ""})
    };
    let acts = root
        .as_object_mut()
        .and_then(|o| o.get_mut("acts"))
        .and_then(|a| a.as_array_mut());
    if let Some(acts) = acts {
        let mut found = false;
        for act in acts.iter_mut() {
            let vi = act
                .get("volume_index")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            if vi == bound.volume_index {
                if let Some(obj) = act.as_object_mut() {
                    if bound.start_chapter > 0 {
                        obj.insert("start_chapter".into(), Value::from(bound.start_chapter));
                    }
                    if bound.end_chapter > 0 {
                        obj.insert("end_chapter".into(), Value::from(bound.end_chapter));
                    }
                    if !bound.name.is_empty()
                        && obj
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .is_empty()
                    {
                        obj.insert("name".into(), Value::String(bound.name.clone()));
                    }
                    if !bound.ending_conditions.is_empty() {
                        obj.insert(
                            "ending_condition".into(),
                            Value::String(bound.ending_conditions.join("\n")),
                        );
                    }
                    if set_completed || bound.completed {
                        obj.insert("completed".into(), Value::Bool(true));
                        obj.insert("status".into(), Value::String("completed".into()));
                    }
                }
                found = true;
                break;
            }
        }
        if !found {
            acts.push(json!({
                "name": bound.name,
                "volume_index": bound.volume_index,
                "start_chapter": bound.start_chapter,
                "end_chapter": bound.end_chapter,
                "goal": bound.goal,
                "ending_condition": bound.ending_conditions.join("\n"),
                "completed": set_completed || bound.completed,
                "progress_notes": "",
            }));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_flat_arc_outline_to_per_volume_file() {
        let root = std::env::temp_dir().join(format!(
            "novelx_arc_migrate_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("artifacts")).unwrap();
        std::fs::write(
            root.join("artifacts/arc_outline.md"),
            "# 第2卷 · 承卷\n\n## 卷定位\n- x\n\n## 开卷状态\n- y\n\n## 冲突升级阶梯\n1. a\n\n## 关键节点\n- n\n\n## 人物弧\n- p\n\n## 伏笔\n- f\n\n## 卷末终止条件\n- c1\n- c2\n\n## 卷末交付\n- d\n",
        )
        .unwrap();
        migrate_arc_outlines(&root);
        assert!(!root.join("artifacts/arc_outline.md").exists());
        assert!(arc_outline_path(&root, 2).is_file());
        assert!(read_arc_outline_text(&root, 2).unwrap().contains("第2卷"));
        assert!(read_arc_outline_text(&root, 1).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_ending_conditions_section() {
        let md = r#"# 第1卷 · 开局

## 卷末终止条件
- 主角完成关键能力觉醒并被对立双方同时锁定
- 身份秘密进入公开边缘
- 卷高潮冲突事件已发生

## 其他
- 忽略
"#;
        let bounds = parse_volume_meta_from_markdown(md);
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].volume_index, 1);
        assert_eq!(bounds[0].ending_conditions.len(), 3);
        assert_eq!(bounds[0].end_chapter, 0, "有终止条件时不定章数");
    }

    #[test]
    fn soft_range_without_conditions_kept() {
        let md = "# 第1卷 · 裂隙呼吸\n\n- **对应总纲幕/阶段**：第一幕：裂隙呼吸（第1–15章）\n";
        let bounds = parse_volume_meta_from_markdown(md);
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].start_chapter, 1);
        assert_eq!(bounds[0].end_chapter, 15);
    }

    #[test]
    fn legacy_volume_end_only_without_conditions() {
        let with_cond = VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 15,
            name: "第一幕".into(),
            ending_conditions: vec!["完成觉醒".into()],
            goal: String::new(),
            completed: false,
        };
        let legacy = VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 15,
            name: "第一幕".into(),
            ending_conditions: vec![],
            goal: String::new(),
            completed: false,
        };
        assert!(volume_just_ended(15, &[with_cond]).is_none());
        assert!(volume_just_ended(15, &[legacy]).is_some());
    }

    #[test]
    fn active_prefers_incomplete_open_volume() {
        let bounds = vec![
            VolumeBound {
                volume_index: 1,
                start_chapter: 1,
                end_chapter: 8,
                name: "已完".into(),
                ending_conditions: vec![],
                goal: String::new(),
                completed: true,
            },
            VolumeBound {
                volume_index: 2,
                start_chapter: 9,
                end_chapter: 0,
                name: "进行中".into(),
                ending_conditions: vec!["中点质变".into()],
                goal: String::new(),
                completed: false,
            },
        ];
        let active = active_volume_from_bounds(&bounds, 12).unwrap();
        assert_eq!(active.volume_index, 2);
    }

    #[test]
    fn split_conditions_bullets() {
        let v = split_ending_conditions("- A\n- B；C");
        assert_eq!(v, vec!["A", "B", "C"]);
    }
}
