//! Soft/hard gates requiring volume_audit before mid-volume continue or sync.

use crate::volume::{active_volume_for_chapter, volume_chapter_span};
use crate::volume_audit::volume_has_audit_report;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD: u32 = 20;

#[derive(Debug, Clone)]
pub struct VolumeAuditGateBlock {
    pub reason: String,
    pub message: String,
    pub volume_index: u32,
    /// `mid` | `handoff`
    pub kind: &'static str,
}

#[derive(Debug, Deserialize)]
struct FeaturesFile {
    #[serde(default)]
    features: HashMap<String, bool>,
}

const DEFAULT_THICK_VOLUME_CHAPTER_THRESHOLD: u32 = 40;

#[derive(Debug, Deserialize)]
struct VolumeConfigFile {
    #[serde(default = "default_mid_audit_threshold")]
    mid_audit_chapter_threshold: u32,
    #[serde(default = "default_thick_volume_threshold")]
    thick_volume_chapter_threshold: u32,
}

fn default_mid_audit_threshold() -> u32 {
    DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD
}

fn default_thick_volume_threshold() -> u32 {
    DEFAULT_THICK_VOLUME_CHAPTER_THRESHOLD
}

/// Resolve `<repo>/config` from `projects/<name>` (or None if layout unknown).
pub fn find_config_root(project_dir: &Path) -> Option<std::path::PathBuf> {
    // projects/<name> → <repo>/config
    project_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("config"))
        .filter(|p| p.is_dir())
}

fn load_volume_config(config_root: &Path) -> VolumeConfigFile {
    let path = config_root.join("volume.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return VolumeConfigFile {
            mid_audit_chapter_threshold: DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD,
            thick_volume_chapter_threshold: DEFAULT_THICK_VOLUME_CHAPTER_THRESHOLD,
        };
    };
    serde_yaml::from_str::<VolumeConfigFile>(&raw).unwrap_or(VolumeConfigFile {
        mid_audit_chapter_threshold: DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD,
        thick_volume_chapter_threshold: DEFAULT_THICK_VOLUME_CHAPTER_THRESHOLD,
    })
}

/// Mid-volume audit threshold from `config/volume.yaml` (project → repo config).
pub fn mid_audit_threshold_resolved(project_dir: &Path) -> u32 {
    let Some(root) = find_config_root(project_dir) else {
        return DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD;
    };
    let n = load_volume_config(&root).mid_audit_chapter_threshold;
    if n == 0 {
        DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD
    } else {
        n
    }
}

/// Thick-volume sampling / warning threshold from `config/volume.yaml`.
pub fn thick_volume_threshold_resolved(project_dir: &Path) -> u32 {
    let Some(root) = find_config_root(project_dir) else {
        return DEFAULT_THICK_VOLUME_CHAPTER_THRESHOLD;
    };
    let n = load_volume_config(&root).thick_volume_chapter_threshold;
    if n == 0 {
        DEFAULT_THICK_VOLUME_CHAPTER_THRESHOLD
    } else {
        n
    }
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

fn mid_audit_chapter_threshold(config_root: &Path) -> u32 {
    let n = load_volume_config(config_root).mid_audit_chapter_threshold;
    if n == 0 {
        DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD
    } else {
        n
    }
}

fn chapters_in_volume(project_dir: &Path, chapter: u32) -> Option<(u32, u32)> {
    let vol = active_volume_for_chapter(project_dir, chapter)?;
    let (from, to) = volume_chapter_span(&vol, chapter);
    // Count chapters that actually have content (draft/summary), not bare span width.
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

/// Soft-block continue_writing when mid-volume and no audit report yet.
pub fn check_volume_audit_for_continue(
    config_root: &Path,
    project_dir: &Path,
    chapter: u32,
    skip: bool,
) -> Option<VolumeAuditGateBlock> {
    if skip {
        return None;
    }
    if crate::project::is_short_drama(project_dir) {
        return None;
    }
    if !feature_enabled(config_root, "studio.require_volume_audit_mid", true) {
        return None;
    }
    let (volume_index, count) = chapters_in_volume(project_dir, chapter)?;
    let threshold = mid_audit_chapter_threshold(config_root);
    if count < threshold {
        return None;
    }
    if volume_has_audit_report(project_dir, volume_index) {
        return None;
    }
    Some(VolumeAuditGateBlock {
        reason: "need_volume_audit_mid".into(),
        kind: "mid",
        volume_index,
        message: format!(
            "本卷第{volume_index}卷已写约 {count} 章（≥{threshold}），尚未做整卷摘要复盘。\n\
             - 先审：audit_volume(volume={volume_index})\n\
             - 跳过本次：continue_writing 传 confirm_skip_volume_audit=true"
        ),
    })
}

/// Block sync_volume when handoff gate enabled and no audit report.
pub fn check_volume_audit_for_sync(
    config_root: &Path,
    project_dir: &Path,
    volume_index: u32,
    skip: bool,
) -> Option<VolumeAuditGateBlock> {
    if skip {
        return None;
    }
    if crate::project::is_short_drama(project_dir) {
        return None;
    }
    if !feature_enabled(config_root, "studio.require_volume_audit_handoff", true) {
        return None;
    }
    if volume_has_audit_report(project_dir, volume_index) {
        return None;
    }
    Some(VolumeAuditGateBlock {
        reason: "need_volume_audit_handoff".into(),
        kind: "handoff",
        volume_index,
        message: format!(
            "卷末同步前须先完成第{volume_index}卷摘要复盘。\n\
             - 先审：audit_volume(volume={volume_index})\n\
             - 跳过：sync_volume 传 confirm_skip_volume_audit=true"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::init_project;
    use std::fs;

    #[test]
    fn mid_gate_blocks_without_report() {
        let root = std::env::temp_dir().join(format!(
            "novelx_vol_gate_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
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
        // Default threshold 20: 20 chapters should soft-block mid-audit.
        for ch in 1..=20 {
            let d = dir.join("chapters").join(format!("{ch:03}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("draft.md"), format!("# 第{ch}章\n正文")).unwrap();
            fs::write(
                d.join("summary.json"),
                r#"{"event_summary":"事件"}"#,
            )
            .unwrap();
        }
        let block = check_volume_audit_for_continue(&config, &dir, 21, false);
        assert!(block.is_some());
        assert_eq!(block.as_ref().unwrap().kind, "mid");
        assert!(block.unwrap().message.contains("≥20"));
        let report_dir = dir.join("artifacts/volume_audits");
        fs::create_dir_all(&report_dir).unwrap();
        fs::write(
            report_dir.join("01.md"),
            "# 第1卷复盘（L1·摘要层）\n\n范围：第1–20章\n\n## 结论\n\n未见阻断级问题。\n",
        )
        .unwrap();
        assert!(check_volume_audit_for_continue(&config, &dir, 21, false).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mid_gate_defaults_threshold_without_volume_yaml() {
        let root = std::env::temp_dir().join(format!(
            "novelx_vol_gate_default_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        let projects = root.join("projects");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("features.yaml"),
            "features:\n  studio.require_volume_audit_mid: true\n",
        )
        .unwrap();
        // No volume.yaml → DEFAULT_MID_AUDIT_CHAPTER_THRESHOLD (20).
        init_project(&projects, "sample-novel", "未定", 900).unwrap();
        let dir = projects.join("sample-novel");
        for ch in 1..=20 {
            let d = dir.join("chapters").join(format!("{ch:03}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("draft.md"), "正文").unwrap();
        }
        let block = check_volume_audit_for_continue(&config, &dir, 21, false);
        assert!(block.is_some());
        assert!(block.unwrap().message.contains("≥20"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mid_gate_respects_custom_threshold() {
        let root = std::env::temp_dir().join(format!(
            "novelx_vol_gate_custom_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
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
            "mid_audit_chapter_threshold: 40\n",
        )
        .unwrap();
        init_project(&projects, "sample-novel", "未定", 900).unwrap();
        let dir = projects.join("sample-novel");
        for ch in 1..=25 {
            let d = dir.join("chapters").join(format!("{ch:03}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("draft.md"), "正文").unwrap();
        }
        assert!(check_volume_audit_for_continue(&config, &dir, 26, false).is_none());
        for ch in 26..=40 {
            let d = dir.join("chapters").join(format!("{ch:03}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("draft.md"), "正文").unwrap();
        }
        let block = check_volume_audit_for_continue(&config, &dir, 41, false);
        assert!(block.is_some());
        assert!(block.unwrap().message.contains("≥40"));
        let _ = fs::remove_dir_all(&root);
    }
}
