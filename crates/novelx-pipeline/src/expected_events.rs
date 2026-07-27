//! Preprocess expected events — author/reader intents deferred until eligible.
//! Distinct from narrative foreshadow (`memory.open_threads`).

use crate::cards::{load_markdown_cards, normalize_entity_status, truncate_chars};
use crate::plots::load_plot_index;
use crate::project::load_project_state;
use crate::volume::active_volume_for_chapter;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Cap active waiting/eligible/approved events.
pub const ACTIVE_EVENT_LIMIT: usize = 48;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExpectedEventStore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub events: Vec<ExpectedEvent>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExpectedEvent {
    pub id: String,
    /// add_character | exit_character | revive_character | plot | setting | other
    #[serde(default = "default_kind")]
    pub kind: String,
    pub text: String,
    /// author | reader
    #[serde(default = "default_source")]
    pub source: String,
    /// waiting | eligible | approved | incorporated | dismissed
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub entity_ref: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub created_chapter: u32,
    #[serde(default)]
    pub resolved_chapter: u32,
    #[serde(default)]
    pub conditions: ExpectedConditions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_review: Option<ExpectedReview>,
}

fn default_kind() -> String {
    "other".into()
}
fn default_source() -> String {
    "author".into()
}
fn default_status() -> String {
    "waiting".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExpectedConditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_chapter: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chapter: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_plot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_plot_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_entity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_entity_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_event_id: Option<String>,
    /// Human-readable extra conditions (not hard-gated alone).
    #[serde(default)]
    pub freeform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExpectedReview {
    #[serde(default)]
    pub chapter: u32,
    #[serde(default)]
    pub volume: u32,
    #[serde(default)]
    pub hard_ok: bool,
    /// 高 | 中 | 低 | 不适用
    #[serde(default)]
    pub agent_fit: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub suggestion: String,
    #[serde(default)]
    pub reviewed_at: String,
}

#[derive(Debug, Clone)]
pub struct EligibilitySnapshot {
    pub hard_ok: bool,
    pub label: String,
    pub reasons: Vec<String>,
}

pub fn expected_events_path(project_dir: &Path) -> PathBuf {
    project_dir.join("lore/expected_events.json")
}

pub fn load_expected_events(project_dir: &Path) -> ExpectedEventStore {
    let path = expected_events_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ExpectedEventStore {
            version: 1,
            events: Vec::new(),
        };
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_expected_events(project_dir: &Path, store: &ExpectedEventStore) -> Result<()> {
    let path = expected_events_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(store).context("serialize expected_events")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn active_count(store: &ExpectedEventStore) -> usize {
    store
        .events
        .iter()
        .filter(|e| matches!(e.status.as_str(), "waiting" | "eligible" | "approved"))
        .count()
}

fn normalize_kind(raw: &str) -> String {
    match raw.trim() {
        "add_character" | "exit_character" | "revive_character" | "plot" | "setting" | "other" => {
            raw.trim().into()
        }
        "加角色" | "新增角色" => "add_character".into(),
        "写死" | "退场" | "杀角色" => "exit_character".into(),
        "复活" | "重新上线" => "revive_character".into(),
        "剧情" => "plot".into(),
        "设定" => "setting".into(),
        _ => "other".into(),
    }
}

fn normalize_source(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "reader" | "读者" => "reader".into(),
        _ => "author".into(),
    }
}

fn normalize_event_status(raw: &str) -> String {
    match raw.trim() {
        "waiting" | "eligible" | "approved" | "incorporated" | "dismissed" => raw.trim().into(),
        _ => "waiting".into(),
    }
}

/// Parse conditions object from tool JSON args.
pub fn conditions_from_value(v: Option<&Value>) -> ExpectedConditions {
    let Some(v) = v else {
        return ExpectedConditions::default();
    };
    ExpectedConditions {
        min_chapter: v.get("min_chapter").and_then(|x| x.as_u64()).map(|n| n as u32),
        max_chapter: v.get("max_chapter").and_then(|x| x.as_u64()).map(|n| n as u32),
        volume: v.get("volume").and_then(|x| x.as_u64()).map(|n| n as u32),
        require_plot_id: v
            .get("require_plot_id")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        require_plot_status: v
            .get("require_plot_status")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        require_entity: v
            .get("require_entity")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        require_entity_status: v
            .get("require_entity_status")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        after_event_id: v
            .get("after_event_id")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        freeform: v
            .get("freeform")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

pub fn enqueue_expected_event(
    project_dir: &Path,
    kind: &str,
    text: &str,
    source: &str,
    entity_ref: &str,
    notes: &str,
    conditions: ExpectedConditions,
    created_chapter: u32,
) -> Result<ExpectedEvent> {
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("text 必填");
    }
    let mut store = load_expected_events(project_dir);
    if active_count(&store) >= ACTIVE_EVENT_LIMIT {
        anyhow::bail!("活跃预期事件已达上限（{ACTIVE_EVENT_LIMIT}）");
    }
    let ev = ExpectedEvent {
        id: format!("ee_{}", Uuid::new_v4().simple()),
        kind: normalize_kind(kind),
        text: text.to_string(),
        source: normalize_source(source),
        status: "waiting".into(),
        entity_ref: entity_ref.trim().to_string(),
        notes: notes.trim().to_string(),
        created_chapter,
        resolved_chapter: 0,
        conditions,
        last_review: None,
    };
    store.events.push(ev.clone());
    save_expected_events(project_dir, &store)?;
    Ok(ev)
}

pub fn update_expected_event(
    project_dir: &Path,
    id: &str,
    text: Option<&str>,
    kind: Option<&str>,
    notes: Option<&str>,
    entity_ref: Option<&str>,
    conditions: Option<ExpectedConditions>,
) -> Result<ExpectedEvent> {
    let mut store = load_expected_events(project_dir);
    let ev = store
        .events
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow::anyhow!("未找到预期事件 {id}"))?;
    if matches!(ev.status.as_str(), "incorporated" | "dismissed") {
        anyhow::bail!("已结束的事件不可再改（status={}）", ev.status);
    }
    if let Some(t) = text.map(str::trim).filter(|s| !s.is_empty()) {
        ev.text = t.to_string();
    }
    if let Some(k) = kind.filter(|s| !s.trim().is_empty()) {
        ev.kind = normalize_kind(k);
    }
    if let Some(n) = notes {
        ev.notes = n.trim().to_string();
    }
    if let Some(r) = entity_ref {
        ev.entity_ref = r.trim().to_string();
    }
    if let Some(c) = conditions {
        ev.conditions = c;
    }
    let out = ev.clone();
    save_expected_events(project_dir, &store)?;
    Ok(out)
}

pub fn resolve_expected_event(
    project_dir: &Path,
    id: &str,
    status: &str,
    resolved_chapter: u32,
) -> Result<ExpectedEvent> {
    let status = normalize_event_status(status);
    if !matches!(status.as_str(), "approved" | "incorporated" | "dismissed" | "waiting" | "eligible")
    {
        anyhow::bail!("非法 status: {status}");
    }
    let mut store = load_expected_events(project_dir);
    let ev = store
        .events
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow::anyhow!("未找到预期事件 {id}"))?;
    ev.status = status.clone();
    if matches!(status.as_str(), "incorporated" | "dismissed") {
        ev.resolved_chapter = resolved_chapter;
    }
    let out = ev.clone();
    save_expected_events(project_dir, &store)?;
    Ok(out)
}

pub fn set_event_review(
    project_dir: &Path,
    id: &str,
    review: ExpectedReview,
    bump_eligible: bool,
) -> Result<ExpectedEvent> {
    let mut store = load_expected_events(project_dir);
    let ev = store
        .events
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow::anyhow!("未找到预期事件 {id}"))?;
    if bump_eligible && review.hard_ok && ev.status == "waiting" {
        ev.status = "eligible".into();
    }
    ev.last_review = Some(review);
    let out = ev.clone();
    save_expected_events(project_dir, &store)?;
    Ok(out)
}

/// Project snapshot used for hard eligibility.
#[derive(Debug, Clone, Default)]
pub struct ExpectedEvalContext {
    pub chapter: u32,
    pub volume: u32,
    pub plots: Vec<(String, String)>, // id_or_slug_or_title, status
    pub entities: Vec<(String, String)>, // name keys, normalized status
    pub incorporated_ids: Vec<String>,
}

pub fn build_eval_context(project_dir: &Path, chapter: u32) -> ExpectedEvalContext {
    let volume = active_volume_for_chapter(project_dir, chapter.max(1))
        .map(|b| b.volume_index)
        .unwrap_or(1);
    let index = load_plot_index(project_dir);
    let mut plots = Vec::new();
    for vol in &index.volumes {
        for p in &vol.plots {
            plots.push((p.id.clone(), p.status.clone()));
            if !p.slug.is_empty() {
                plots.push((p.slug.clone(), p.status.clone()));
            }
            if !p.title.is_empty() {
                plots.push((p.title.clone(), p.status.clone()));
            }
        }
    }
    let mut entities = Vec::new();
    for group in ["characters", "items", "locations"] {
        for c in load_markdown_cards(&project_dir.join("entities").join(group), group) {
            let st = normalize_entity_status(
                c.meta.get("status").map(|s| s.as_str()).unwrap_or(""),
            );
            for k in c.match_keys() {
                entities.push((k, st.clone()));
            }
        }
    }
    let store = load_expected_events(project_dir);
    let incorporated_ids = store
        .events
        .iter()
        .filter(|e| e.status == "incorporated")
        .map(|e| e.id.clone())
        .collect();
    ExpectedEvalContext {
        chapter,
        volume,
        plots,
        entities,
        incorporated_ids,
    }
}

pub fn eval_hard_ok(ev: &ExpectedEvent, ctx: &ExpectedEvalContext) -> EligibilitySnapshot {
    let mut reasons = Vec::new();
    let mut ok = true;
    let c = &ev.conditions;

    if let Some(min) = c.min_chapter {
        if ctx.chapter < min {
            ok = false;
            reasons.push(format!("未到最小章（当前第{}章 < {min}）", ctx.chapter));
        }
    }
    if let Some(max) = c.max_chapter {
        if ctx.chapter > max {
            ok = false;
            reasons.push(format!("已过最大章（当前第{}章 > {max}）", ctx.chapter));
        }
    }
    if let Some(vol) = c.volume {
        if ctx.volume < vol {
            ok = false;
            reasons.push(format!("未到目标卷（当前第{}卷 < {vol}）", ctx.volume));
        }
    }
    if let Some(ref pid) = c.require_plot_id {
        let want = c
            .require_plot_status
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let found = ctx.plots.iter().find(|(id, _)| id == pid || id.contains(pid));
        match found {
            None => {
                ok = false;
                reasons.push(format!("缺少剧情卡「{pid}」"));
            }
            Some((_, st)) if !want.is_empty() && st != &want => {
                ok = false;
                reasons.push(format!("剧情卡「{pid}」状态为 {st}，需要 {want}"));
            }
            Some(_) => {}
        }
    } else if let Some(ref want) = c.require_plot_status {
        // Status-only constraint without id: any plot with that status counts.
        if !want.is_empty() && !ctx.plots.iter().any(|(_, st)| st == want) {
            ok = false;
            reasons.push(format!("尚无 status={want} 的剧情卡"));
        }
    }
    if let Some(ref ename) = c.require_entity {
        let found = ctx.entities.iter().find(|(n, _)| {
            n == ename || n.contains(ename) || ename.contains(n.as_str())
        });
        let want = c
            .require_entity_status
            .as_deref()
            .map(normalize_entity_status)
            .unwrap_or_default();
        match found {
            None => {
                ok = false;
                reasons.push(format!("缺少实体「{ename}」"));
            }
            Some((_, st)) if !want.is_empty() && st != &want => {
                ok = false;
                reasons.push(format!(
                    "实体「{ename}」状态为 {st}，需要 {want}"
                ));
            }
            Some(_) => {}
        }
    }
    if let Some(ref after) = c.after_event_id {
        if !ctx.incorporated_ids.iter().any(|id| id == after) {
            ok = false;
            reasons.push(format!("依赖事件 {after} 尚未 incorporated"));
        }
    }

    let label = match ev.status.as_str() {
        "approved" => "已批准",
        "incorporated" => "已纳入",
        "dismissed" => "已搁置",
        "eligible" if ok => "可检阅",
        "waiting" if ok => "可检阅",
        "eligible" | "waiting" => "等待中",
        _ => "等待中",
    };

    if ok && reasons.is_empty() {
        reasons.push("结构化条件已满足".into());
    }

    EligibilitySnapshot {
        hard_ok: ok,
        label: label.into(),
        reasons,
    }
}

pub fn agent_fit_actionable(fit: &str) -> bool {
    matches!(fit.trim(), "高" | "中" | "high" | "mid" | "medium")
}

/// Waiting/eligible events whose hard conditions pass for `chapter`.
pub fn list_hard_ok_candidates(project_dir: &Path, chapter: u32) -> Vec<ExpectedEvent> {
    let store = load_expected_events(project_dir);
    let ctx = build_eval_context(project_dir, chapter);
    store
        .events
        .into_iter()
        .filter(|e| matches!(e.status.as_str(), "waiting" | "eligible"))
        .filter(|e| eval_hard_ok(e, &ctx).hard_ok)
        .collect()
}

/// Candidates ready for user gate: hard_ok + last_review agent_fit 高/中 for this chapter.
pub fn list_gate_candidates(project_dir: &Path, chapter: u32) -> Vec<ExpectedEvent> {
    list_hard_ok_candidates(project_dir, chapter)
        .into_iter()
        .filter(|e| {
            e.last_review.as_ref().is_some_and(|r| {
                r.chapter == chapter && r.hard_ok && agent_fit_actionable(&r.agent_fit)
            })
        })
        .collect()
}

pub fn needs_fresh_review(project_dir: &Path, chapter: u32) -> bool {
    list_hard_ok_candidates(project_dir, chapter)
        .iter()
        .any(|e| {
            e.last_review
                .as_ref()
                .map(|r| r.chapter != chapter)
                .unwrap_or(true)
        })
}

pub fn count_by_status(project_dir: &Path) -> (usize, usize, usize) {
    let store = load_expected_events(project_dir);
    let pending = store
        .events
        .iter()
        .filter(|e| matches!(e.status.as_str(), "waiting" | "eligible"))
        .count();
    let approved = store
        .events
        .iter()
        .filter(|e| e.status == "approved")
        .count();
    let due = {
        let ch = load_project_state(project_dir)
            .map(|s| s.next_chapter.max(1))
            .unwrap_or(1);
        list_hard_ok_candidates(project_dir, ch).len()
    };
    (pending, approved, due)
}

fn sort_rank(ev: &ExpectedEvent, hard_ok: bool) -> u8 {
    match ev.status.as_str() {
        "approved" => 0,
        "eligible" if hard_ok => 1,
        "waiting" if hard_ok => 1,
        "waiting" | "eligible" => 2,
        "incorporated" => 3,
        "dismissed" => 4,
        _ => 5,
    }
}

/// Preview JSON rows for Web / list tool.
pub fn list_events_for_preview(project_dir: &Path, chapter: u32) -> Vec<Value> {
    let store = load_expected_events(project_dir);
    if store.events.is_empty() {
        return Vec::new();
    }
    let ctx = build_eval_context(project_dir, chapter);
    let mut rows: Vec<(u8, Value)> = store
        .events
        .iter()
        .map(|ev| {
            let elig = eval_hard_ok(ev, &ctx);
            let rank = sort_rank(ev, elig.hard_ok);
            let row = json!({
                "id": ev.id,
                "kind": ev.kind,
                "text": ev.text,
                "source": ev.source,
                "status": ev.status,
                "entity_ref": ev.entity_ref,
                "notes": ev.notes,
                "created_chapter": ev.created_chapter,
                "resolved_chapter": ev.resolved_chapter,
                "conditions": ev.conditions,
                "last_review": ev.last_review,
                "eligibility": {
                    "hard_ok": elig.hard_ok,
                    "label": elig.label,
                    "reasons": elig.reasons,
                },
            });
            (rank, row)
        })
        .collect();
    rows.sort_by_key(|(r, _)| *r);
    rows.into_iter().map(|(_, v)| v).collect()
}

pub fn format_conditions_line(c: &ExpectedConditions) -> String {
    let mut parts = Vec::new();
    if let Some(n) = c.min_chapter {
        parts.push(format!("≥第{n}章"));
    }
    if let Some(n) = c.max_chapter {
        parts.push(format!("≤第{n}章"));
    }
    if let Some(v) = c.volume {
        parts.push(format!("第{v}卷"));
    }
    if let Some(ref p) = c.require_plot_id {
        let st = c
            .require_plot_status
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(":{s}"))
            .unwrap_or_default();
        parts.push(format!("剧情卡{p}{st}"));
    } else if let Some(ref st) = c.require_plot_status {
        parts.push(format!("剧情status={st}"));
    }
    if let Some(ref e) = c.require_entity {
        let st = c
            .require_entity_status
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(":{s}"))
            .unwrap_or_default();
        parts.push(format!("实体{e}{st}"));
    }
    if let Some(ref a) = c.after_event_id {
        parts.push(format!("依赖{a}"));
    }
    if !c.freeform.trim().is_empty() {
        parts.push(truncate_chars(c.freeform.trim(), 40));
    }
    if parts.is_empty() {
        "无硬条件（随时可检阅）".into()
    } else {
        parts.join(" · ")
    }
}

fn foreshadow_hint_for_kind(kind: &str) -> String {
    match kind.trim().to_ascii_lowercase().as_str() {
        "add_character" | "add_entity" | "introduce" => " → 章纲可标「埋设」".into(),
        "kill" | "death" | "remove" | "reveal" | "resolve" => " → 章纲可标「回收/兑现」".into(),
        "resurrect" | "return" => " → 章纲可标「回收旧线/再埋」".into(),
        _ => " → 章纲可标「埋设或兑现」".into(),
    }
}

/// Canon injection: approved constraints + short waiting list.
pub fn format_expected_for_context(project_dir: &Path, chapter: u32, pacing: bool) -> String {
    let store = load_expected_events(project_dir);
    if store.events.is_empty() {
        return String::new();
    }
    let ctx = build_eval_context(project_dir, chapter);
    let mut parts = Vec::new();

    let approved: Vec<_> = store
        .events
        .iter()
        .filter(|e| e.status == "approved")
        .take(if pacing { 3 } else { 8 })
        .collect();
    if !approved.is_empty() {
        let lines: Vec<String> = approved
            .iter()
            .map(|e| {
                let hint = foreshadow_hint_for_kind(&e.kind);
                format!(
                    "- [{}] {}{}{}",
                    e.kind,
                    truncate_chars(&e.text, 120),
                    if e.entity_ref.is_empty() {
                        String::new()
                    } else {
                        format!("（实体：{}）", e.entity_ref)
                    },
                    hint
                )
            })
            .collect();
        parts.push(format!(
            "已批准预期（须纳入本章创作，勿擅自改设定落盘）：\n{}",
            lines.join("\n")
        ));
        if !pacing {
            parts.push(
                "预期↔伏笔提示：章纲可标注「埋设/回收」对应上列已批准项；\
                 正文兑现后由伏笔追踪或用户 resolve_expected_event，禁止静默改设定。"
                    .into(),
            );
        }
    }

    if pacing {
        return parts.join("\n\n");
    }

    let waiting: Vec<_> = store
        .events
        .iter()
        .filter(|e| matches!(e.status.as_str(), "waiting" | "eligible"))
        .take(6)
        .collect();
    if !waiting.is_empty() {
        let lines: Vec<String> = waiting
            .iter()
            .map(|e| {
                let elig = eval_hard_ok(e, &ctx);
                format!(
                    "- [{}|{}] {}（{}）",
                    e.status,
                    if elig.hard_ok { "条件满" } else { "等待" },
                    truncate_chars(&e.text, 80),
                    format_conditions_line(&e.conditions)
                )
            })
            .collect();
        parts.push(format!(
            "等待中的预期（禁止擅自兑现，须用户批准）：\n{}",
            lines.join("\n")
        ));
    }

    parts.join("\n\n")
}

pub fn format_expected_for_lore(project_dir: &Path, chapter: u32) -> String {
    let text = format_expected_for_context(project_dir, chapter, false);
    if text.is_empty() {
        String::new()
    } else {
        format!("## Lore·预设预期\n{text}")
    }
}

/// Human summary for list tool.
pub fn format_events_summary(project_dir: &Path, chapter: u32, status_filter: Option<&str>) -> String {
    let rows = list_events_for_preview(project_dir, chapter);
    let rows: Vec<&Value> = rows
        .iter()
        .filter(|r| {
            status_filter
                .map(|s| r.get("status").and_then(|v| v.as_str()) == Some(s))
                .unwrap_or(true)
        })
        .collect();
    if rows.is_empty() {
        return "尚无预处理预期事件".into();
    }
    let mut lines = vec!["## 预处理预期事件".to_string()];
    for r in rows {
        let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let status = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let text = r.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let label = r
            .pointer("/eligibility/label")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let fit = r
            .pointer("/last_review/agent_fit")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        lines.push(format!(
            "- `{id}` [{status}/{label}][{kind}] {}（检阅拟合：{fit}）",
            truncate_chars(text, 100)
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("novelx-ee-{tag}-{stamp}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("lore")).unwrap();
        fs::create_dir_all(root.join("plots")).unwrap();
        fs::create_dir_all(root.join("entities/characters")).unwrap();
        fs::write(
            root.join("state.json"),
            r#"{"name":"sample-novel","next_chapter":5,"published_count":4}"#,
        )
        .unwrap();
        root
    }

    #[test]
    fn enqueue_load_persist() {
        let dir = sample_dir("persist");
        let ev = enqueue_expected_event(
            &dir,
            "add_character",
            "读者希望增加配角甲",
            "reader",
            "甲",
            "",
            ExpectedConditions {
                min_chapter: Some(10),
                ..Default::default()
            },
            5,
        )
        .unwrap();
        assert!(ev.id.starts_with("ee_"));
        let store = load_expected_events(&dir);
        assert_eq!(store.events.len(), 1);
        assert_eq!(store.events[0].text, "读者希望增加配角甲");
        assert_eq!(store.events[0].source, "reader");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hard_ok_chapter_window() {
        let dir = sample_dir("chapter");
        let ev = enqueue_expected_event(
            &dir,
            "other",
            "抽象落点",
            "author",
            "",
            "",
            ExpectedConditions {
                min_chapter: Some(8),
                max_chapter: Some(20),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        let early = build_eval_context(&dir, 5);
        assert!(!eval_hard_ok(&ev, &early).hard_ok);
        let ok = build_eval_context(&dir, 10);
        assert!(eval_hard_ok(&ev, &ok).hard_ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hard_ok_entity_status_and_after_event() {
        let dir = sample_dir("entity");
        fs::write(
            dir.join("entities/characters/主角.md"),
            "---\nname: 主角\nstatus: exited\n---\n\n# 主角\n",
        )
        .unwrap();
        let first = enqueue_expected_event(
            &dir,
            "exit_character",
            "主角退场",
            "author",
            "主角",
            "",
            ExpectedConditions::default(),
            1,
        )
        .unwrap();
        resolve_expected_event(&dir, &first.id, "incorporated", 3).unwrap();

        let revive = enqueue_expected_event(
            &dir,
            "revive_character",
            "主角重新上线",
            "author",
            "主角",
            "",
            ExpectedConditions {
                require_entity: Some("主角".into()),
                require_entity_status: Some("exited".into()),
                after_event_id: Some(first.id.clone()),
                min_chapter: Some(5),
                ..Default::default()
            },
            3,
        )
        .unwrap();
        let ctx = build_eval_context(&dir, 5);
        assert!(eval_hard_ok(&revive, &ctx).hard_ok);

        let ctx2 = build_eval_context(&dir, 4);
        assert!(!eval_hard_ok(&revive, &ctx2).hard_ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_and_resolve_removes_from_due() {
        let dir = sample_dir("format");
        let ev = enqueue_expected_event(
            &dir,
            "plot",
            "抵达落点",
            "author",
            "",
            "",
            ExpectedConditions {
                min_chapter: Some(1),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        assert_eq!(list_hard_ok_candidates(&dir, 2).len(), 1);
        let ctx_md = format_expected_for_context(&dir, 2, false);
        assert!(ctx_md.contains("等待中的预期"));
        resolve_expected_event(&dir, &ev.id, "approved", 0).unwrap();
        let ctx2 = format_expected_for_context(&dir, 2, false);
        assert!(ctx2.contains("已批准预期"));
        assert!(
            ctx2.contains("埋设") || ctx2.contains("兑现") || ctx2.contains("伏笔"),
            "approved expected should carry foreshadow weak hint: {ctx2}"
        );
        resolve_expected_event(&dir, &ev.id, "incorporated", 2).unwrap();
        assert!(list_hard_ok_candidates(&dir, 2).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn foreshadow_hint_maps_kinds() {
        assert!(foreshadow_hint_for_kind("add_character").contains("埋设"));
        assert!(foreshadow_hint_for_kind("kill").contains("回收"));
        assert!(foreshadow_hint_for_kind("resurrect").contains("再埋") || foreshadow_hint_for_kind("resurrect").contains("回收"));
    }

    #[test]
    fn preview_sorts_approved_first() {
        let dir = sample_dir("sort");
        let a = enqueue_expected_event(
            &dir,
            "other",
            "等待项",
            "author",
            "",
            "",
            ExpectedConditions {
                min_chapter: Some(100),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        let b = enqueue_expected_event(
            &dir,
            "other",
            "批准项",
            "author",
            "",
            "",
            ExpectedConditions::default(),
            1,
        )
        .unwrap();
        resolve_expected_event(&dir, &b.id, "approved", 0).unwrap();
        let rows = list_events_for_preview(&dir, 1);
        assert_eq!(rows[0]["id"], b.id);
        assert_eq!(rows[1]["id"], a.id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_store_zero_cost() {
        let dir = sample_dir("empty");
        assert!(list_hard_ok_candidates(&dir, 1).is_empty());
        assert!(!needs_fresh_review(&dir, 1));
        assert!(format_expected_for_context(&dir, 1, false).is_empty());
        assert!(list_events_for_preview(&dir, 1).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
