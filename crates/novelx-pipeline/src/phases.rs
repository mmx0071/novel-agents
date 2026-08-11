//! Setup (灵感定稿) and volume-handoff phase state machines.
//!
//! - `setup_phase` lives in `meta.json`
//! - `volume_phase` lives in `state.json` → `meta.volume_phase`

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::plots::load_plot_index;
use crate::project::{load_project_state, save_project_state};

/// Feature-flagged enforcement for setup / volume / chapter-order write gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseEnforceFlags {
    pub setup: bool,
    pub volume: bool,
    pub chapter_order: bool,
    pub mutation_confirm: bool,
}

impl Default for PhaseEnforceFlags {
    fn default() -> Self {
        Self {
            setup: true,
            volume: true,
            chapter_order: true,
            mutation_confirm: true,
        }
    }
}

impl PhaseEnforceFlags {
    /// Read studio.enforce_* / require_mutation_confirm from features.yaml.
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("features.yaml");
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        #[derive(Deserialize)]
        struct FeaturesFile {
            #[serde(default)]
            features: HashMap<String, bool>,
        }
        let Ok(file) = serde_yaml::from_str::<FeaturesFile>(&raw) else {
            return Self::default();
        };
        Self {
            setup: file
                .features
                .get("studio.enforce_setup_gate")
                .copied()
                .unwrap_or(true),
            volume: file
                .features
                .get("studio.enforce_volume_phase")
                .copied()
                .unwrap_or(true),
            chapter_order: file
                .features
                .get("studio.enforce_chapter_order")
                .copied()
                .unwrap_or(true),
            mutation_confirm: file
                .features
                .get("studio.require_mutation_confirm")
                .copied()
                .unwrap_or(true),
        }
    }
}

/// Inspiration → outline confirmation before first chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupPhase {
    Collecting,
    AwaitingConfirm,
    Ready,
}

impl SetupPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Collecting => "collecting",
            Self::AwaitingConfirm => "awaiting_confirm",
            Self::Ready => "ready",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "collecting" => Some(Self::Collecting),
            "awaiting_confirm" => Some(Self::AwaitingConfirm),
            "ready" => Some(Self::Ready),
            _ => None,
        }
    }
}

/// Volume handoff after a volume ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumePhase {
    DraftingVolume,
    AwaitingSync,
    AwaitingNextArc,
    AwaitingNextPlot,
}

impl VolumePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DraftingVolume => "drafting_volume",
            Self::AwaitingSync => "awaiting_sync",
            Self::AwaitingNextArc => "awaiting_next_arc",
            Self::AwaitingNextPlot => "awaiting_next_plot",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "drafting_volume" => Some(Self::DraftingVolume),
            "awaiting_sync" => Some(Self::AwaitingSync),
            "awaiting_next_arc" => Some(Self::AwaitingNextArc),
            "awaiting_next_plot" => Some(Self::AwaitingNextPlot),
            _ => None,
        }
    }
}

fn path_nonempty(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|t| t.trim().len() > 20)
        .unwrap_or(false)
}

fn has_master_outline(project_dir: &Path) -> bool {
    path_nonempty(&project_dir.join("artifacts/master_outline.md"))
        || path_nonempty(&project_dir.join("artifacts/master_planner.md"))
        || path_nonempty(&project_dir.join("artifacts/series_outline.md"))
        || path_nonempty(&project_dir.join("artifacts/story_outline.json"))
}

fn is_short_drama(project_dir: &Path) -> bool {
    crate::project::is_short_drama(project_dir)
}

fn has_arc_outline(project_dir: &Path) -> bool {
    crate::volume::has_any_arc_outline(project_dir)
}

/// Load `meta.json` as object (empty object if missing).
pub fn load_meta_json(project_dir: &Path) -> Value {
    let path = project_dir.join("meta.json");
    if !path.exists() {
        return json!({});
    }
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|_| json!({}))
}

pub fn save_meta_json(project_dir: &Path, meta: &Value) -> Result<()> {
    let path = project_dir.join("meta.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(meta)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Resolve setup phase from disk; migrate legacy projects that already wrote chapters.
pub fn resolve_setup_phase(project_dir: &Path) -> SetupPhase {
    let meta_path = project_dir.join("meta.json");
    let state_path = project_dir.join("state.json");
    // Bare fixtures (unit tests with only plots/) — do not force collecting.
    if !meta_path.exists() && !state_path.exists() {
        return SetupPhase::Ready;
    }
    // Already publishing → ready (legacy projects / mid-serial).
    if let Ok(state) = load_project_state(project_dir) {
        if state.published_count >= 1 {
            return SetupPhase::Ready;
        }
    }
    let meta = load_meta_json(project_dir);
    if let Some(s) = meta
        .get("setup_phase")
        .and_then(|v| v.as_str())
        .and_then(SetupPhase::parse)
    {
        return s;
    }
    let outlines_ready = if is_short_drama(project_dir) {
        has_master_outline(project_dir)
    } else {
        has_master_outline(project_dir) && has_arc_outline(project_dir)
    };
    if outlines_ready {
        return SetupPhase::AwaitingConfirm;
    }
    // Has project files but no outlines yet.
    if meta_path.exists() || state_path.exists() {
        return SetupPhase::Collecting;
    }
    SetupPhase::Ready
}

pub fn set_setup_phase(project_dir: &Path, phase: SetupPhase) -> Result<()> {
    let mut meta = load_meta_json(project_dir);
    let obj = meta.as_object_mut().unwrap();
    obj.insert(
        "setup_phase".into(),
        Value::String(phase.as_str().to_string()),
    );
    save_meta_json(project_dir, &meta)
}

pub fn lock_brief(project_dir: &Path, brief: &str) -> Result<()> {
    let mut meta = load_meta_json(project_dir);
    let obj = meta.as_object_mut().unwrap();
    obj.insert("brief".into(), Value::String(brief.trim().to_string()));
    // Locking brief keeps collecting until outlines exist + user confirms.
    let current = resolve_setup_phase(project_dir);
    if current == SetupPhase::Ready {
        // Do not demote ready projects when refreshing brief.
    } else if current != SetupPhase::AwaitingConfirm {
        obj.insert(
            "setup_phase".into(),
            Value::String(SetupPhase::Collecting.as_str().to_string()),
        );
    }
    save_meta_json(project_dir, &meta)
}

/// After master+arc outlines are on disk, move collecting → awaiting_confirm (if not ready).
pub fn maybe_advance_setup_after_outlines(project_dir: &Path) -> Result<SetupPhase> {
    let current = resolve_setup_phase(project_dir);
    if current == SetupPhase::Ready {
        return Ok(current);
    }
    let outlines_ready = if is_short_drama(project_dir) {
        has_master_outline(project_dir)
    } else {
        has_master_outline(project_dir) && has_arc_outline(project_dir)
    };
    if outlines_ready {
        set_setup_phase(project_dir, SetupPhase::AwaitingConfirm)?;
        return Ok(SetupPhase::AwaitingConfirm);
    }
    if current == SetupPhase::AwaitingConfirm {
        // Outlines incomplete again → back to collecting.
        set_setup_phase(project_dir, SetupPhase::Collecting)?;
        return Ok(SetupPhase::Collecting);
    }
    Ok(current)
}

/// Bible meets schema minimum (H1 + required sections 0/1/2/7).
pub fn has_valid_bible(project_dir: &Path) -> bool {
    let path = project_dir.join("artifacts/bible.md");
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    crate::schemas::validate_bible(&text).is_ok()
}

/// Human-readable why Bible fails the confirm_setup hard gate.
pub fn bible_setup_gap_reason(project_dir: &Path) -> Option<String> {
    let path = project_dir.join("artifacts/bible.md");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Some(
                "世界观 Bible 缺失（artifacts/bible.md）。请先 upsert_setting / supplement_setting 或激活 world_architect 补齐必填节后再确认定稿。"
                    .into(),
            );
        }
    };
    match crate::schemas::validate_bible(&text) {
        Ok(_) => None,
        Err(e) => Some(format!(
            "世界观 Bible 未达最低完备度：{}。须含 H1「世界观…」及 ## 0./1./2./7. 节；请 upsert_setting 补齐后再确认定稿。",
            e.message
        )),
    }
}

pub fn confirm_setup_approve(project_dir: &Path) -> Result<SetupPhase> {
    if is_short_drama(project_dir) {
        if !has_master_outline(project_dir) {
            anyhow::bail!(
                "系列/短剧总纲尚未齐全，无法确认定稿。请先 design_master_outline（可写 series_outline.md）。"
            );
        }
    } else if !has_master_outline(project_dir) || !has_arc_outline(project_dir) {
        anyhow::bail!("总纲与卷纲尚未齐全，无法确认定稿。请先 design_master_outline / design_arc_outline。");
    }
    if let Some(reason) = bible_setup_gap_reason(project_dir) {
        anyhow::bail!("{reason}");
    }
    set_setup_phase(project_dir, SetupPhase::Ready)?;
    Ok(SetupPhase::Ready)
}

pub fn confirm_setup_revise(project_dir: &Path) -> Result<SetupPhase> {
    set_setup_phase(project_dir, SetupPhase::Collecting)?;
    Ok(SetupPhase::Collecting)
}

/// Next concrete action to finish inspiration setup (None = ready to write).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupNextStep {
    NeedBrief,
    NeedMaster,
    NeedArc,
    NeedBible,
    Confirm,
}

impl SetupNextStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedBrief => "need_brief",
            Self::NeedMaster => "need_master",
            Self::NeedArc => "need_arc",
            Self::NeedBible => "need_bible",
            Self::Confirm => "confirm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "need_brief" => Some(Self::NeedBrief),
            "need_master" => Some(Self::NeedMaster),
            "need_arc" => Some(Self::NeedArc),
            "need_bible" => Some(Self::NeedBible),
            "confirm" => Some(Self::Confirm),
            _ => None,
        }
    }

    /// `gates.yaml` key for the approval card.
    pub fn gate_name(self) -> &'static str {
        match self {
            Self::NeedBrief => "setup_need_brief",
            Self::NeedMaster => "setup_need_master",
            Self::NeedArc => "setup_need_arc",
            Self::NeedBible => "setup_need_bible",
            Self::Confirm => "setup_confirm",
        }
    }

    pub fn prompt(self, project: &str) -> String {
        match self {
            Self::NeedBrief => format!(
                "《{project}》尚未锁定灵感。请直接发送一句话卖点/灵感，或查看项目状态。"
            ),
            Self::NeedMaster => format!(
                "《{project}》灵感已锁定，下一步请生成总纲。"
            ),
            Self::NeedArc => format!(
                "《{project}》总纲已就绪，下一步请生成卷纲。"
            ),
            Self::NeedBible => format!(
                "《{project}》总纲与卷纲已就绪，下一步请补齐世界观 Bible（须含 ## 0./1./2./7.）。"
            ),
            Self::Confirm => format!(
                "《{project}》总纲、卷纲与世界观 Bible 已就绪，是否确认定稿并进入写章？"
            ),
        }
    }

    pub fn summary_hint(self) -> &'static str {
        match self {
            Self::NeedBrief => "请直接发送灵感文案，或选择：查看项目状态 / 暂不处理。",
            Self::NeedMaster => "请选择：生成总纲 / 暂不处理。",
            Self::NeedArc => "请选择：生成卷纲 / 暂不处理。",
            Self::NeedBible => "请选择：生成世界观 Bible / 暂不处理。",
            Self::Confirm => "请选择：确认定稿 / 修改再生成。",
        }
    }
}

fn brief_nonempty(project_dir: &Path) -> bool {
    load_meta_json(project_dir)
        .get("brief")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Resolve the next setup action; `None` when writing is allowed.
pub fn resolve_setup_next_step(project_dir: &Path) -> Option<SetupNextStep> {
    match resolve_setup_phase(project_dir) {
        SetupPhase::Ready => None,
        SetupPhase::Collecting | SetupPhase::AwaitingConfirm => {
            if !brief_nonempty(project_dir) {
                return Some(SetupNextStep::NeedBrief);
            }
            if !has_master_outline(project_dir) {
                return Some(SetupNextStep::NeedMaster);
            }
            // Short-drama: series outline + bible; no volume arc required.
            if !is_short_drama(project_dir) && !has_arc_outline(project_dir) {
                return Some(SetupNextStep::NeedArc);
            }
            if !has_valid_bible(project_dir) {
                return Some(SetupNextStep::NeedBible);
            }
            Some(SetupNextStep::Confirm)
        }
    }
}

/// Block message when setup is not ready for writing.
pub fn setup_write_block_reason(project_dir: &Path) -> Option<String> {
    let next = resolve_setup_next_step(project_dir)?;
    let phase = resolve_setup_phase(project_dir);
    let short = is_short_drama(project_dir);
    let step = match next {
        SetupNextStep::NeedBrief => {
            "请先 lock_brief（在对话中发送灵感/一句话卖点）".to_string()
        }
        SetupNextStep::NeedMaster => {
            if short {
                "下一步：design_master_outline 生成系列/短剧总纲（可落盘 series_outline.md）"
                    .to_string()
            } else {
                "下一步：design_master_outline 生成总纲".to_string()
            }
        }
        SetupNextStep::NeedArc => "下一步：design_arc_outline 生成卷纲".to_string(),
        SetupNextStep::NeedBible => {
            "下一步：upsert_setting 补齐世界观 Bible（须含 ## 0./1./2./7.）".to_string()
        }
        SetupNextStep::Confirm => {
            if short {
                "请用户选择「确认定稿」后再写集".to_string()
            } else {
                "请用户选择「确认定稿」后再写章".to_string()
            }
        }
    };
    Some(format!(
        "定稿未完成（setup_phase={}）。{step}。",
        phase.as_str()
    ))
}

/// Resolve volume phase from state.meta, with inference for legacy projects.
pub fn resolve_volume_phase(project_dir: &Path) -> VolumePhase {
    // Short-drama never enters volume handoff.
    if is_short_drama(project_dir) {
        return VolumePhase::DraftingVolume;
    }
    if let Ok(state) = load_project_state(project_dir) {
        if let Some(s) = state
            .meta
            .get("volume_phase")
            .and_then(|v| v.as_str())
            .and_then(VolumePhase::parse)
        {
            return s;
        }
    }
    infer_volume_phase(project_dir)
}

fn infer_volume_phase(project_dir: &Path) -> VolumePhase {
    let index = load_plot_index(project_dir);
    let has_active = index
        .volumes
        .iter()
        .flat_map(|v| v.plots.iter())
        .any(|p| matches!(p.status.as_str(), "in_progress" | "bridging"));
    if has_active {
        return VolumePhase::DraftingVolume;
    }
    // Completed volume with no active plot → likely mid-handoff.
    let bounds = crate::volume::load_volume_bounds(project_dir);
    let any_completed = bounds.iter().any(|b| b.completed);
    let has_arc = has_arc_outline(project_dir);
    if any_completed && !has_active {
        if !has_arc {
            return VolumePhase::AwaitingNextArc;
        }
        return VolumePhase::AwaitingNextPlot;
    }
    VolumePhase::DraftingVolume
}

pub fn set_volume_phase(project_dir: &Path, phase: VolumePhase) -> Result<()> {
    let mut state = load_project_state(project_dir)?;
    state
        .meta
        .insert(
            "volume_phase".into(),
            Value::String(phase.as_str().to_string()),
        );
    save_project_state(project_dir, &state)?;
    Ok(())
}

pub fn mark_volume_sync_skipped(project_dir: &Path, skipped: bool) -> Result<()> {
    let mut meta = load_meta_json(project_dir);
    let obj = meta.as_object_mut().unwrap();
    obj.insert("volume_sync_skipped".into(), Value::Bool(skipped));
    save_meta_json(project_dir, &meta)?;
    Ok(())
}

/// If handoff/sync phase was raised while plot cards (or next_plot) remain, undo it.
/// Returns the recovered volume index when a false volume-end was cleared.
pub fn recover_false_volume_end(project_dir: &Path) -> Option<u32> {
    let phase = resolve_volume_phase(project_dir);
    if !matches!(
        phase,
        VolumePhase::AwaitingSync | VolumePhase::AwaitingNextArc | VolumePhase::AwaitingNextPlot
    ) {
        return None;
    }
    let bounds = crate::volume::load_volume_bounds(project_dir);
    // Prefer the "ended" act that still has open plot work; else any volume with open work.
    let mut candidates: Vec<u32> = bounds
        .iter()
        .filter(|b| b.completed)
        .map(|b| b.volume_index)
        .collect();
    candidates.extend(bounds.iter().map(|b| b.volume_index));
    let index = load_plot_index(project_dir);
    for v in &index.volumes {
        if !candidates.contains(&v.volume_index) {
            candidates.push(v.volume_index);
        }
    }
    let volume_index = candidates.into_iter().find(|vi| {
        *vi > 0 && crate::plots::volume_has_open_plot_work(project_dir, *vi)
    })?;
    let _ = mark_volume_sync_skipped(project_dir, false);
    let _ = set_volume_phase(project_dir, VolumePhase::DraftingVolume);
    let _ = crate::volume::reopen_volume_act(project_dir, volume_index);
    tracing::info!(
        volume = volume_index,
        from_phase = phase.as_str(),
        "recovered false volume-end; back to drafting_volume"
    );
    Some(volume_index)
}

/// Block message when volume phase forbids writing.
pub fn volume_write_block_reason(project_dir: &Path) -> Option<String> {
    if is_short_drama(project_dir) {
        return None;
    }
    let _ = recover_false_volume_end(project_dir);
    match resolve_volume_phase(project_dir) {
        VolumePhase::DraftingVolume => None,
        VolumePhase::AwaitingSync => Some(
            "卷已结束，仍待确认设定同步（volume_phase=awaiting_sync）。请先选择「同步设定库」或「跳过」，再进入下卷交接。"
                .into(),
        ),
        VolumePhase::AwaitingNextArc => Some(
            "卷间交接：须先 design_arc_outline 细化下卷（volume_phase=awaiting_next_arc），再 design_plot / 激活剧情卡后写章。"
                .into(),
        ),
        VolumePhase::AwaitingNextPlot => Some(
            "卷间交接：下卷卷纲已就绪，请先 design_plot 并 update_plot(status=in_progress, set_active_main=true)（volume_phase=awaiting_next_plot）。"
                .into(),
        ),
    }
}

/// Whether design_plot may honor `force=true` (disabled during handoff phases).
pub fn design_plot_force_allowed(project_dir: &Path) -> bool {
    matches!(
        resolve_volume_phase(project_dir),
        VolumePhase::DraftingVolume
    ) && resolve_setup_phase(project_dir) == SetupPhase::Ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{init_project, ProjectState};
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_proj(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "novelx-phases-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn setup_next_step_progresses() {
        let root = tmp_proj("setup-next");
        let dir = init_project(&root, "book", "sample-genre", 100).unwrap();
        assert_eq!(
            resolve_setup_next_step(&dir),
            Some(SetupNextStep::NeedBrief)
        );
        lock_brief(&dir, "一位少年决心离家追寻失踪的父亲。").unwrap();
        assert_eq!(
            resolve_setup_next_step(&dir),
            Some(SetupNextStep::NeedMaster)
        );
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            "# 总纲\n\n".to_string() + &"x".repeat(40),
        )
        .unwrap();
        assert_eq!(
            resolve_setup_next_step(&dir),
            Some(SetupNextStep::NeedArc)
        );
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            "# 卷纲\n\n".to_string() + &"y".repeat(40),
        )
        .unwrap();
        let _ = maybe_advance_setup_after_outlines(&dir);
        // init_project writes a schema-valid bible stub → Confirm (not NeedBible).
        assert_eq!(
            resolve_setup_next_step(&dir),
            Some(SetupNextStep::Confirm)
        );
        // Invalidate stub → NeedBible; restore full bible → Confirm again.
        fs::write(dir.join("artifacts/bible.md"), "# 不完整\n\n缺节。\n").unwrap();
        assert_eq!(
            resolve_setup_next_step(&dir),
            Some(SetupNextStep::NeedBible)
        );
        fs::write(
            dir.join("artifacts/bible.md"),
            r#"# 世界观 Bible

## 0. 一句话世界
世界。

## 1. 时代与叙事框架
时代。

## 2. 全局势力与阵营
势力。

## 7. 开放问题
待揭。
"#,
        )
        .unwrap();
        assert_eq!(
            resolve_setup_next_step(&dir),
            Some(SetupNextStep::Confirm)
        );
        let msg = setup_write_block_reason(&dir).unwrap();
        assert!(msg.contains("确认定稿"));
        confirm_setup_approve(&dir).unwrap();
        assert_eq!(resolve_setup_next_step(&dir), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn setup_advances_and_confirm() {
        let root = tmp_proj("setup");
        let dir = init_project(&root, "book", "sample-genre", 100).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Collecting);
        lock_brief(&dir, "一位少年决心离家追寻失踪的父亲。").unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Collecting);
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            "# 总纲\n\n".to_string() + &"x".repeat(40),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            "# 卷纲\n\n".to_string() + &"y".repeat(40),
        )
        .unwrap();
        assert_eq!(
            maybe_advance_setup_after_outlines(&dir).unwrap(),
            SetupPhase::AwaitingConfirm
        );
        assert!(setup_write_block_reason(&dir).is_some());
        // Stub bible from init is schema-valid; corrupt it to exercise the hard gate.
        fs::write(dir.join("artifacts/bible.md"), "# 世界观\n\n只有标题。\n").unwrap();
        assert!(confirm_setup_approve(&dir).is_err(), "bible incomplete must block");
        fs::write(
            dir.join("artifacts/bible.md"),
            r#"# 世界观 Bible

## 0. 一句话世界
世界。

## 1. 时代与叙事框架
时代。

## 2. 全局势力与阵营
势力。

## 7. 开放问题
待揭。
"#,
        )
        .unwrap();
        confirm_setup_approve(&dir).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Ready);
        assert!(setup_write_block_reason(&dir).is_none());
        confirm_setup_revise(&dir).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Collecting);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn confirm_rejects_invalid_bible() {
        let root = tmp_proj("bible-gate");
        let dir = init_project(&root, "book", "未定", 100).unwrap();
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            "# 总纲\n\n".to_string() + &"x".repeat(40),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            "# 卷纲\n\n".to_string() + &"y".repeat(40),
        )
        .unwrap();
        maybe_advance_setup_after_outlines(&dir).unwrap();
        // Replace init stub with invalid content (missing required sections).
        fs::write(dir.join("artifacts/bible.md"), "# 世界观 Bible\n\n片段。\n").unwrap();
        let err = confirm_setup_approve(&dir).unwrap_err().to_string();
        assert!(err.contains("Bible") || err.contains("世界观"));
        assert!(!has_valid_bible(&dir));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_published_is_ready() {
        let root = tmp_proj("legacy");
        let dir = init_project(&root, "old", "未定", 50).unwrap();
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            "# m\n\n".to_string() + &"a".repeat(40),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            "# a\n\n".to_string() + &"b".repeat(40),
        )
        .unwrap();
        let state = ProjectState {
            name: "old".into(),
            genre: "未定".into(),
            target_chapters: 50,
            published_count: 3,
            next_chapter: 4,
            active_agents: vec![],
            meta: HashMap::new(),
            extra: HashMap::new(),
        };
        // No setup_phase in meta.json
        save_project_state(&dir, &state).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Ready);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recovers_false_volume_end_when_next_plot_missing() {
        let root = tmp_proj("false-end");
        let dir = init_project(&root, "book", "未定", 100).unwrap();
        set_volume_phase(&dir, VolumePhase::AwaitingSync).unwrap();
        fs::create_dir_all(dir.join("plots")).unwrap();
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("plots/开局.md"),
            "---\ntitle: 开局\nvolume_index: 1\nplot_type: main\nstatus: completed\nnext_plot: 下一阶\n---\n\n# 开局\n\n## 收束条件\n已到站\n",
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/story_outline.json"),
            r#"{
  "acts": [
    {
      "volume_index": 1,
      "name": "试卷",
      "completed": true,
      "status": "completed",
      "start_chapter": 1,
      "end_chapter": 3,
      "ending_condition": "真正卷末"
    }
  ]
}"#,
        )
        .unwrap();
        let _ = crate::plots::rebuild_plot_index(&dir);
        assert!(crate::plots::volume_has_open_plot_work(&dir, 1));
        assert_eq!(recover_false_volume_end(&dir), Some(1));
        assert_eq!(resolve_volume_phase(&dir), VolumePhase::DraftingVolume);
        let so: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.join("artifacts/story_outline.json")).unwrap())
                .unwrap();
        assert_eq!(so["acts"][0]["completed"], false);
        assert!(volume_write_block_reason(&dir).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn volume_phase_transitions() {
        let root = tmp_proj("vol");
        let dir = init_project(&root, "vbook", "未定", 100).unwrap();
        assert_eq!(resolve_volume_phase(&dir), VolumePhase::DraftingVolume);
        set_volume_phase(&dir, VolumePhase::AwaitingSync).unwrap();
        assert_eq!(resolve_volume_phase(&dir), VolumePhase::AwaitingSync);
        assert!(volume_write_block_reason(&dir).is_some());
        set_volume_phase(&dir, VolumePhase::AwaitingNextArc).unwrap();
        mark_volume_sync_skipped(&dir, true).unwrap();
        let meta = load_meta_json(&dir);
        assert_eq!(meta["volume_sync_skipped"], true);
        set_volume_phase(&dir, VolumePhase::DraftingVolume).unwrap();
        assert!(volume_write_block_reason(&dir).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn enforce_flags_default_and_yaml() {
        let root = tmp_proj("flags");
        assert_eq!(PhaseEnforceFlags::load(&root), PhaseEnforceFlags::default());
        fs::write(
            root.join("features.yaml"),
            "version: 1\nfeatures:\n  studio.enforce_setup_gate: false\n  studio.enforce_volume_phase: false\n  studio.enforce_chapter_order: false\n  studio.require_mutation_confirm: false\n",
        )
        .unwrap();
        let f = PhaseEnforceFlags::load(&root);
        assert!(!f.setup);
        assert!(!f.volume);
        assert!(!f.chapter_order);
        assert!(!f.mutation_confirm);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn short_drama_skips_arc_in_setup() {
        let root = tmp_proj("short-drama-setup");
        let dir = crate::project::init_project_with_mode(
            &root,
            "demo",
            "未定",
            12,
            crate::project::ProjectMode::ShortDrama,
        )
        .unwrap();
        assert!(crate::project::is_short_drama(&dir));
        assert!(dir.join("episodes").is_dir());
        lock_brief(&dir, "一句话卖点测试").unwrap();
        // series outline counts as master
        std::fs::write(
            dir.join("artifacts/series_outline.md"),
            "# 总纲\n\n## 一句话卖点\n测\n\n## 分集骨架\n第1-3集\n\n## 主角弧\n成长\n\n## 主线冲突\n对抗\n",
        )
        .unwrap();
        // minimal valid bible
        std::fs::write(
            dir.join("artifacts/bible.md"),
            "# 世界观 Bible\n\n## 0. 总览\nx\n\n## 1. 规则\nx\n\n## 2. 力量\nx\n\n## 7. 禁忌\nx\n",
        )
        .unwrap();
        let next = resolve_setup_next_step(&dir);
        assert_eq!(next, Some(SetupNextStep::Confirm));
        assert!(volume_write_block_reason(&dir).is_none());
        confirm_setup_approve(&dir).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Ready);
        assert!(setup_write_block_reason(&dir).is_none());
    }

}
