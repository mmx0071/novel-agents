//! Foreshadow index: durable dangling list for CanonContext + tracker read-back.
//! Longform: age-boosted display (oldest first) + cold archive never hard-drops.
//! Debt tiers: batch brake uses pressure (overdue near/mid), not total / far-horizon.

use crate::cards::truncate_chars;
use crate::memory::{
    load_foreshadow_archive, load_memory, select_dangling_age_boosted, OpenThread, ProjectMemory,
};
use anyhow::{Context, Result};
use novelx_harness::ForeshadowDebtConfig;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Batch-relevant debt class for one open foreshadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeshadowDebtClass {
    /// Within grace after plant — inventory only.
    Fresh,
    /// Overdue near-term pressure — counts for batch brake.
    Near,
    /// Older pressure band — counts when `batch_count_mid`.
    Mid,
    /// Long-horizon / low urgency — never counts for batch brake.
    Far,
}

impl ForeshadowDebtClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Near => "near",
            Self::Mid => "mid",
            Self::Far => "far",
        }
    }

    pub fn counts_for_batch(self, cfg: &ForeshadowDebtConfig) -> bool {
        match self {
            Self::Near => true,
            Self::Mid => cfg.batch_count_mid,
            Self::Fresh | Self::Far => false,
        }
    }
}

/// Normalize tracker `horizon` / `urgency` into near|mid|far (empty if unknown).
pub fn normalize_foreshadow_horizon(horizon: &str, urgency: &str) -> String {
    let h = horizon.trim().to_ascii_lowercase();
    if matches!(h.as_str(), "near" | "mid" | "far") {
        return h;
    }
    match urgency.trim().to_ascii_lowercase().as_str() {
        "high" => "near".into(),
        "mid" | "medium" => "mid".into(),
        "low" => "far".into(),
        _ => String::new(),
    }
}

/// Classify one open thread relative to the chapter about to be written.
pub fn classify_foreshadow_debt(
    planted_chapter: u32,
    horizon: &str,
    urgency: &str,
    current_chapter: u32,
    cfg: &ForeshadowDebtConfig,
) -> ForeshadowDebtClass {
    let explicit = normalize_foreshadow_horizon(horizon, urgency);
    if explicit == "far" {
        return ForeshadowDebtClass::Far;
    }
    let age = if planted_chapter == 0 || current_chapter <= planted_chapter {
        0
    } else {
        current_chapter.saturating_sub(planted_chapter)
    };
    let grace = cfg.grace_chapters;
    let far_after = cfg.far_after_chapters.max(grace.saturating_add(1));

    if explicit == "near" {
        return if age <= grace {
            ForeshadowDebtClass::Fresh
        } else {
            ForeshadowDebtClass::Near
        };
    }
    if explicit == "mid" {
        return if age <= grace {
            ForeshadowDebtClass::Fresh
        } else if age > far_after {
            ForeshadowDebtClass::Far
        } else {
            ForeshadowDebtClass::Mid
        };
    }

    // Auto (untagged): grace → fresh; (grace, far_after] → near pressure; older → far.
    if age <= grace {
        ForeshadowDebtClass::Fresh
    } else if age > far_after {
        ForeshadowDebtClass::Far
    } else {
        ForeshadowDebtClass::Near
    }
}

#[derive(Debug, Clone, Default)]
pub struct ForeshadowDebtBreakdown {
    pub total: u32,
    pub fresh: u32,
    pub near: u32,
    pub mid: u32,
    pub far: u32,
    /// Threads that count toward `batch_max_dangling_foreshadow`.
    pub pressure: u32,
}

pub fn foreshadow_debt_breakdown(
    dangling: &[OpenThread],
    current_chapter: u32,
    cfg: &ForeshadowDebtConfig,
) -> ForeshadowDebtBreakdown {
    let mut out = ForeshadowDebtBreakdown {
        total: dangling.len() as u32,
        ..Default::default()
    };
    for t in dangling {
        let class = classify_foreshadow_debt(
            t.planted_chapter,
            &t.horizon,
            &t.urgency,
            current_chapter,
            cfg,
        );
        match class {
            ForeshadowDebtClass::Fresh => out.fresh += 1,
            ForeshadowDebtClass::Near => out.near += 1,
            ForeshadowDebtClass::Mid => out.mid += 1,
            ForeshadowDebtClass::Far => out.far += 1,
        }
        if class.counts_for_batch(cfg) {
            out.pressure += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForeshadowIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_chapter: u32,
    /// Hot + archived + cold open threads (oldest planted first for longform debt).
    #[serde(default)]
    pub dangling: Vec<OpenThread>,
}

fn default_version() -> u32 {
    1
}

pub fn foreshadow_index_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/foreshadow_index.json")
}

pub fn load_foreshadow_index(project_dir: &Path) -> ForeshadowIndex {
    let path = foreshadow_index_path(project_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return rebuild_index_from_memory(project_dir, &load_memory(project_dir));
    };
    serde_json::from_str(&text)
        .unwrap_or_else(|_| rebuild_index_from_memory(project_dir, &load_memory(project_dir)))
}

pub fn rebuild_foreshadow_index(project_dir: &Path) -> Result<ForeshadowIndex> {
    let mem = load_memory(project_dir);
    let idx = rebuild_index_from_memory(project_dir, &mem);
    let path = foreshadow_index_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&idx).context("serialize foreshadow index")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(idx)
}

fn rebuild_index_from_memory(project_dir: &Path, mem: &ProjectMemory) -> ForeshadowIndex {
    let resolved_keys: std::collections::HashSet<String> = mem
        .open_threads
        .iter()
        .chain(mem.archived_threads.iter())
        .filter(|t| t.status == "resolved")
        .map(|t| {
            if t.id.is_empty() {
                t.text.clone()
            } else {
                t.id.clone()
            }
        })
        .collect();
    let mut dangling: Vec<OpenThread> = mem
        .open_threads
        .iter()
        .chain(mem.archived_threads.iter())
        .filter(|t| t.status == "open" || t.status.is_empty())
        .cloned()
        .collect();
    for t in load_foreshadow_archive(project_dir) {
        if !(t.status == "open" || t.status.is_empty()) {
            continue;
        }
        let key = if t.id.is_empty() {
            t.text.clone()
        } else {
            t.id.clone()
        };
        // Hot-zone resolve must suppress stale cold open rows.
        if resolved_keys.contains(&key) {
            continue;
        }
        if dangling.iter().any(|a| a.id == t.id || a.text == t.text) {
            continue;
        }
        dangling.push(t);
    }
    // Age boost: oldest planted first (longform debt surfaces before new plants).
    dangling = select_dangling_age_boosted(&dangling, dangling.len());
    // Dedup by id/text (keep first = oldest after sort).
    let mut seen = std::collections::HashSet::new();
    dangling.retain(|t| {
        let key = if t.id.is_empty() {
            t.text.clone()
        } else {
            t.id.clone()
        };
        seen.insert(key)
    });
    ForeshadowIndex {
        version: 1,
        updated_chapter: mem.last_chapter,
        dangling,
    }
}

/// Format dangling foreshadows for CanonContext / tracker prompts (oldest first).
pub fn format_dangling_for_context(project_dir: &Path, limit: usize) -> String {
    let idx = load_foreshadow_index(project_dir);
    if idx.dangling.is_empty() || limit == 0 {
        return String::new();
    }
    let selected = select_dangling_age_boosted(&idx.dangling, limit);
    let lines: Vec<String> = selected
        .iter()
        .map(|t| {
            let plant = if t.planted_chapter > 0 {
                format!("（埋于第{}章·未收）", t.planted_chapter)
            } else {
                String::new()
            };
            format!("- [{}] {}{}", t.id, truncate_chars(&t.text, 80), plant)
        })
        .collect();
    format!(
        "未回收伏笔（按埋线年龄优先；须承接或明确延期，勿无故遗忘）：\n{}",
        lines.join("\n")
    )
}

/// Compact list for foreshadow_tracker user prompt (includes ids; oldest first).
pub fn format_dangling_for_tracker(project_dir: &Path, limit: usize) -> String {
    let idx = load_foreshadow_index(project_dir);
    if idx.dangling.is_empty() {
        return "（当前无登记未收伏笔）".into();
    }
    select_dangling_age_boosted(&idx.dangling, limit)
        .iter()
        .map(|t| {
            format!(
                "- id={} planted={} text={}",
                t.id,
                t.planted_chapter,
                truncate_chars(&t.text, 100)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{
        append_foreshadow_archive, save_memory, OpenThread, ProjectMemory, ARCHIVED_THREAD_LIMIT,
    };
    use novelx_harness::ForeshadowDebtConfig;
    use std::fs;

    #[test]
    fn debt_tiers_exclude_fresh_and_far_from_pressure() {
        let cfg = ForeshadowDebtConfig {
            grace_chapters: 6,
            far_after_chapters: 40,
            batch_count_mid: true,
        };
        // Recently planted → fresh, not pressure.
        assert_eq!(
            classify_foreshadow_debt(5, "", "", 7, &cfg),
            ForeshadowDebtClass::Fresh
        );
        // Overdue untagged within far_after → near pressure.
        assert_eq!(
            classify_foreshadow_debt(1, "", "", 10, &cfg),
            ForeshadowDebtClass::Near
        );
        // Very old untagged → far (long-horizon inventory).
        assert_eq!(
            classify_foreshadow_debt(1, "", "", 50, &cfg),
            ForeshadowDebtClass::Far
        );
        // Explicit far never pressures, even if "overdue".
        assert_eq!(
            classify_foreshadow_debt(1, "far", "", 20, &cfg),
            ForeshadowDebtClass::Far
        );
        // urgency=low → far.
        assert_eq!(
            classify_foreshadow_debt(1, "", "low", 20, &cfg),
            ForeshadowDebtClass::Far
        );

        let threads = vec![
            OpenThread {
                id: "a".into(),
                planted_chapter: 6,
                ..Default::default()
            },
            OpenThread {
                id: "b".into(),
                planted_chapter: 1,
                horizon: "far".into(),
                ..Default::default()
            },
            OpenThread {
                id: "c".into(),
                planted_chapter: 1,
                ..Default::default()
            },
        ];
        let b = foreshadow_debt_breakdown(&threads, 10, &cfg);
        assert_eq!(b.total, 3);
        assert_eq!(b.fresh, 1); // a age=4
        assert_eq!(b.far, 1); // b
        assert_eq!(b.near, 1); // c age=9
        assert_eq!(b.pressure, 1);
        assert!(!ForeshadowDebtClass::Fresh.counts_for_batch(&cfg));
        assert!(ForeshadowDebtClass::Near.counts_for_batch(&cfg));
    }

    #[test]
    fn rebuild_includes_archived_and_prefers_oldest() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-fs-index-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let mem = ProjectMemory {
            last_chapter: 50,
            open_threads: vec![OpenThread {
                id: "a".into(),
                text: "热窗口新伏笔".into(),
                status: "open".into(),
                planted_chapter: 50,
                ..Default::default()
            }],
            archived_threads: vec![OpenThread {
                id: "b".into(),
                text: "归档旧伏笔".into(),
                status: "open".into(),
                planted_chapter: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        save_memory(&dir, &mem).unwrap();
        let idx = rebuild_foreshadow_index(&dir).unwrap();
        assert_eq!(idx.dangling.len(), 2);
        assert!(
            idx.dangling[0].text.contains("归档旧伏笔"),
            "oldest must rank first: {:?}",
            idx.dangling
        );
        let md = format_dangling_for_context(&dir, 8);
        assert!(md.contains("归档旧伏笔"));
        let tracker = format_dangling_for_tracker(&dir, 1);
        assert!(
            tracker.contains("归档旧伏笔"),
            "tracker must surface oldest debt first: {tracker}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_includes_cold_archive() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-fs-cold-idx-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let mem = ProjectMemory {
            last_chapter: 200,
            open_threads: vec![],
            archived_threads: (1..=ARCHIVED_THREAD_LIMIT as u32)
                .map(|i| OpenThread {
                    id: format!("hot{i}"),
                    text: format!("热归档{i}"),
                    status: "open".into(),
                    planted_chapter: 100 + i,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        save_memory(&dir, &mem).unwrap();
        append_foreshadow_archive(
            &dir,
            &[OpenThread {
                id: "cold1".into(),
                text: "冷归档远古伏笔".into(),
                status: "open".into(),
                planted_chapter: 2,
                ..Default::default()
            }],
        )
        .unwrap();
        let idx = rebuild_foreshadow_index(&dir).unwrap();
        assert!(
            idx.dangling.iter().any(|t| t.text.contains("远古")),
            "cold archive must be in index"
        );
        assert_eq!(idx.dangling[0].planted_chapter, 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
