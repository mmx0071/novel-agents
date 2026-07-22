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

/// Feature-flagged enforcement for setup / volume write gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseEnforceFlags {
    pub setup: bool,
    pub volume: bool,
}

impl Default for PhaseEnforceFlags {
    fn default() -> Self {
        Self {
            setup: true,
            volume: true,
        }
    }
}

impl PhaseEnforceFlags {
    /// Read `studio.enforce_setup_gate` / `studio.enforce_volume_phase` from features.yaml.
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

    /// Writing is only allowed while drafting a volume.
    pub fn allows_writing(self) -> bool {
        matches!(self, Self::DraftingVolume)
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
        || path_nonempty(&project_dir.join("artifacts/story_outline.json"))
}

fn has_arc_outline(project_dir: &Path) -> bool {
    path_nonempty(&project_dir.join("artifacts/arc_outline.md"))
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
    if has_master_outline(project_dir) && has_arc_outline(project_dir) {
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
    if has_master_outline(project_dir) && has_arc_outline(project_dir) {
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

pub fn confirm_setup_approve(project_dir: &Path) -> Result<SetupPhase> {
    if !has_master_outline(project_dir) || !has_arc_outline(project_dir) {
        anyhow::bail!("总纲与卷纲尚未齐全，无法确认定稿。请先 design_master_outline / design_arc_outline。");
    }
    set_setup_phase(project_dir, SetupPhase::Ready)?;
    Ok(SetupPhase::Ready)
}

pub fn confirm_setup_revise(project_dir: &Path) -> Result<SetupPhase> {
    set_setup_phase(project_dir, SetupPhase::Collecting)?;
    Ok(SetupPhase::Collecting)
}

/// Block message when setup is not ready for writing.
pub fn setup_write_block_reason(project_dir: &Path) -> Option<String> {
    match resolve_setup_phase(project_dir) {
        SetupPhase::Ready => None,
        SetupPhase::Collecting => Some(
            "定稿未完成（setup_phase=collecting）。请先 lock_brief → design_master_outline → design_arc_outline，再请用户确认定稿。"
                .into(),
        ),
        SetupPhase::AwaitingConfirm => Some(
            "总纲与卷纲已就绪，等待用户确认定稿（setup_phase=awaiting_confirm）。请用户选择「确认定稿」后再写章。"
                .into(),
        ),
    }
}

/// Resolve volume phase from state.meta, with inference for legacy projects.
pub fn resolve_volume_phase(project_dir: &Path) -> VolumePhase {
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

/// Block message when volume phase forbids writing.
pub fn volume_write_block_reason(project_dir: &Path) -> Option<String> {
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
    fn setup_advances_and_confirm() {
        let root = tmp_proj("setup");
        let dir = init_project(&root, "book", "玄幻", 100).unwrap();
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
        confirm_setup_approve(&dir).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Ready);
        assert!(setup_write_block_reason(&dir).is_none());
        confirm_setup_revise(&dir).unwrap();
        assert_eq!(resolve_setup_phase(&dir), SetupPhase::Collecting);
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
            "version: 1\nfeatures:\n  studio.enforce_setup_gate: false\n  studio.enforce_volume_phase: false\n",
        )
        .unwrap();
        let f = PhaseEnforceFlags::load(&root);
        assert!(!f.setup);
        assert!(!f.volume);
        let _ = fs::remove_dir_all(&root);
    }
}
