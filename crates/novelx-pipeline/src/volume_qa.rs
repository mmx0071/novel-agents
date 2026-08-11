//! Volume QA phase state machine (mid-audit threshold → phase, not bare count gate).
//!
//! Stored in `state.json` → `meta.volume_qa_phase` (+ optional mid-skip marker).
//! Orthogonal to `VolumePhase` (handoff) and hard publish checks.

use crate::project::{is_short_drama, load_project_state, save_project_state};
use crate::volume::{active_volume_for_chapter, load_volume_bounds, volume_chapter_span};
use crate::volume_audit::volume_has_audit_report;
use crate::volume_audit_gate::{mid_audit_threshold_resolved, VolumeAuditGateBlock};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeQaPhase {
    Ok,
    MidDue,
    HandoffRequired,
}

impl VolumeQaPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::MidDue => "mid_due",
            Self::HandoffRequired => "handoff_required",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "ok" => Some(Self::Ok),
            "mid_due" => Some(Self::MidDue),
            "handoff_required" => Some(Self::HandoffRequired),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct FeaturesFile {
    #[serde(default)]
    features: HashMap<String, bool>,
}

fn feature_enabled(config_root: &Path, key: &str, default: bool) -> bool {
    let path = config_root.join("features.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return default;
    };
    serde_yaml::from_str::<FeaturesFile>(&raw)
        .ok()
        .and_then(|f| f.features.get(key).copied())
        .unwrap_or(default)
}

fn chapters_in_volume(project_dir: &Path, chapter: u32) -> Option<(u32, u32)> {
    let vol = active_volume_for_chapter(project_dir, chapter)?;
    let (from, to) = volume_chapter_span(&vol, chapter);
    let mut count = 0u32;
    if from <= to {
        for ch in from..=to {
            let dir = project_dir.join("chapters").join(format!("{ch:03}"));
            let has = dir.join("summary.json").exists()
                || dir.join("draft.md").exists()
                || dir.join("draft.md.gz").exists();
            if has {
                count += 1;
            }
        }
    }
    Some((vol.volume_index, count))
}

/// Volume that just ended and awaits sync — prefer highest completed bound.
fn handoff_volume_index(project_dir: &Path) -> Option<u32> {
    let bounds = load_volume_bounds(project_dir);
    bounds
        .iter()
        .filter(|b| b.completed)
        .map(|b| b.volume_index)
        .max()
        .or_else(|| bounds.iter().map(|b| b.volume_index).max())
}

fn mid_skipped_for_volume(project_dir: &Path, volume_index: u32) -> bool {
    load_project_state(project_dir)
        .ok()
        .and_then(|s| {
            s.meta
                .get("volume_qa_mid_skipped_volume")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
        })
        .is_some_and(|v| v == volume_index)
}

pub fn set_volume_qa_phase(project_dir: &Path, phase: VolumeQaPhase) -> Result<()> {
    let mut state = load_project_state(project_dir)?;
    let prev = state
        .meta
        .get("volume_qa_phase")
        .and_then(|v| v.as_str())
        .and_then(VolumeQaPhase::parse);
    if prev == Some(phase) {
        return Ok(());
    }
    state.meta.insert(
        "volume_qa_phase".into(),
        Value::String(phase.as_str().to_string()),
    );
    save_project_state(project_dir, &state)?;
    Ok(())
}

/// Human dismiss of mid-volume audit for this volume → phase `ok` until report or next volume.
pub fn mark_volume_qa_mid_skipped(project_dir: &Path, volume_index: u32) -> Result<()> {
    let mut state = load_project_state(project_dir)?;
    state.meta.insert(
        "volume_qa_mid_skipped_volume".into(),
        json!(volume_index),
    );
    state.meta.insert(
        "volume_qa_phase".into(),
        Value::String(VolumeQaPhase::Ok.as_str().to_string()),
    );
    save_project_state(project_dir, &state)?;
    Ok(())
}

/// After audit report lands: clear mid-skip marker and set phase ok.
pub fn clear_volume_qa_mid_due(project_dir: &Path, volume_index: u32) -> Result<()> {
    let mut state = load_project_state(project_dir)?;
    if state
        .meta
        .get("volume_qa_mid_skipped_volume")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        == Some(volume_index)
    {
        state.meta.remove("volume_qa_mid_skipped_volume");
    }
    state.meta.insert(
        "volume_qa_phase".into(),
        Value::String(VolumeQaPhase::Ok.as_str().to_string()),
    );
    save_project_state(project_dir, &state)?;
    Ok(())
}

/// Resolve volume QA phase for a chapter (status / gate checks). Read-only — does not write `state.json`.
pub fn resolve_volume_qa_phase(
    config_root: &Path,
    project_dir: &Path,
    chapter: u32,
) -> VolumeQaPhase {
    if is_short_drama(project_dir) {
        return VolumeQaPhase::Ok;
    }

    // Sync handoff without report — use completed volume, not next_chapter's volume.
    if crate::phases::resolve_volume_phase(project_dir) == crate::phases::VolumePhase::AwaitingSync
        && feature_enabled(config_root, "studio.require_volume_audit_handoff", true)
    {
        if let Some(vi) = handoff_volume_index(project_dir) {
            if !volume_has_audit_report(project_dir, vi) {
                return VolumeQaPhase::HandoffRequired;
            }
        }
    }

    let volume_index = chapters_in_volume(project_dir, chapter)
        .map(|(vi, _)| vi)
        .or_else(|| {
            active_volume_for_chapter(project_dir, chapter).map(|v| v.volume_index)
        })
        .unwrap_or(1);

    if !feature_enabled(config_root, "studio.require_volume_audit_mid", true) {
        return VolumeQaPhase::Ok;
    }
    if volume_has_audit_report(project_dir, volume_index) {
        return VolumeQaPhase::Ok;
    }
    if mid_skipped_for_volume(project_dir, volume_index) {
        return VolumeQaPhase::Ok;
    }
    let Some((_, count)) = chapters_in_volume(project_dir, chapter) else {
        return VolumeQaPhase::Ok;
    };
    let threshold = mid_threshold(config_root, project_dir);
    if count >= threshold {
        VolumeQaPhase::MidDue
    } else {
        VolumeQaPhase::Ok
    }
}

fn mid_threshold(config_root: &Path, project_dir: &Path) -> u32 {
    let path = config_root.join("volume.yaml");
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
            if let Some(n) = v
                .get("mid_audit_chapter_threshold")
                .and_then(|x| x.as_u64())
                .map(|n| n as u32)
                .filter(|&n| n > 0)
            {
                return n;
            }
        }
    }
    mid_audit_threshold_resolved(project_dir)
}

/// Soft-block continue when phase is `mid_due` (unless skip).
///
/// - `skip=true, persist_skip=false`: ephemeral (unattended / one-shot) — do not mark volume.
/// - `skip=true, persist_skip=true`: human dismiss for this volume via `mark_volume_qa_mid_skipped`.
pub fn check_volume_qa_for_continue(
    config_root: &Path,
    project_dir: &Path,
    chapter: u32,
    skip: bool,
    persist_skip: bool,
) -> Option<VolumeAuditGateBlock> {
    if skip {
        if persist_skip {
            if let Some((volume_index, _)) = chapters_in_volume(project_dir, chapter) {
                let _ = mark_volume_qa_mid_skipped(project_dir, volume_index);
            }
        }
        return None;
    }
    let phase = resolve_volume_qa_phase(config_root, project_dir, chapter);
    if phase != VolumeQaPhase::MidDue {
        return None;
    }
    let (volume_index, count) = chapters_in_volume(project_dir, chapter)?;
    let threshold = mid_threshold(config_root, project_dir);
    Some(VolumeAuditGateBlock {
        reason: "need_volume_audit_mid".into(),
        kind: "mid",
        volume_index,
        message: format!(
            "卷 QA 相位 mid_due：第{volume_index}卷已写约 {count} 章（≥{threshold}），尚未做整卷摘要复盘。\n\
             - 先审：audit_volume(volume={volume_index})\n\
             - 本次放行：continue_writing(confirm_skip_volume_audit=true)\n\
             - 本卷不再提醒：再加 persist_volume_qa_skip=true"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::init_project;
    use std::fs;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "novelx-vqa-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn mid_due_ephemeral_skip_does_not_persist() {
        let root = tmp("mid");
        let projects = root.join("projects");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("features.yaml"),
            "features:\n  studio.require_volume_audit_mid: true\n",
        )
        .unwrap();
        fs::write(
            config.join("volume.yaml"),
            "mid_audit_chapter_threshold: 20\n",
        )
        .unwrap();
        init_project(&projects, "sample-novel", "未定", 900).unwrap();
        let dir = projects.join("sample-novel");
        for ch in 1..=20 {
            let d = dir.join("chapters").join(format!("{ch:03}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("draft.md"), "正文").unwrap();
        }
        assert_eq!(
            resolve_volume_qa_phase(&config, &dir, 21),
            VolumeQaPhase::MidDue
        );
        let block = check_volume_qa_for_continue(&config, &dir, 21, false, false);
        assert!(block.is_some());
        // Unattended-style skip: do not permanently dismiss.
        assert!(check_volume_qa_for_continue(&config, &dir, 21, true, false).is_none());
        assert_eq!(
            resolve_volume_qa_phase(&config, &dir, 21),
            VolumeQaPhase::MidDue
        );
        // Human persist skip.
        assert!(check_volume_qa_for_continue(&config, &dir, 21, true, true).is_none());
        assert_eq!(
            resolve_volume_qa_phase(&config, &dir, 21),
            VolumeQaPhase::Ok
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn handoff_uses_completed_volume_not_next_chapter() {
        let root = tmp("handoff");
        let projects = root.join("projects");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("features.yaml"),
            "features:\n  studio.require_volume_audit_handoff: true\n",
        )
        .unwrap();
        init_project(&projects, "sample-novel", "未定", 900).unwrap();
        let dir = projects.join("sample-novel");
        // Volume 1 completed, awaiting sync; next_chapter already in vol 2 range.
        crate::phases::set_volume_phase(&dir, crate::phases::VolumePhase::AwaitingSync).unwrap();
        let mut state = load_project_state(&dir).unwrap();
        state.next_chapter = 21;
        save_project_state(&dir, &state).unwrap();
        let art = dir.join("artifacts");
        fs::create_dir_all(&art).unwrap();
        fs::write(
            art.join("story_outline.json"),
            r#"{
              "acts": [
                {"volume_index": 1, "start_chapter": 1, "end_chapter": 20, "completed": true, "name": "第1卷"},
                {"volume_index": 2, "start_chapter": 21, "end_chapter": 40, "completed": false, "name": "第2卷"}
              ]
            }"#,
        )
        .unwrap();
        // Vol1 has no audit report → handoff_required even though chapter 21 is vol2.
        let phase = resolve_volume_qa_phase(&config, &dir, 21);
        assert_eq!(phase, VolumeQaPhase::HandoffRequired);
        // After report for vol 1, not handoff_required from missing vol2 report.
        let report_dir = dir.join("artifacts/volume_audits");
        fs::create_dir_all(&report_dir).unwrap();
        fs::write(
            report_dir.join("01.md"),
            "# 第1卷复盘（L1·摘要层）\n\n范围：第1–20章\n\n## 结论\n\n未见阻断级问题。\n",
        )
        .unwrap();
        let phase2 = resolve_volume_qa_phase(&config, &dir, 21);
        assert_ne!(phase2, VolumeQaPhase::HandoffRequired);
        let _ = fs::remove_dir_all(&root);
    }
}
