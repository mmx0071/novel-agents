//! Foreshadow index: durable dangling list for CanonContext + tracker read-back.
//! Longform: age-boosted display (oldest first) + cold archive never hard-drops.

use crate::cards::truncate_chars;
use crate::memory::{
    load_foreshadow_archive, load_memory, select_dangling_age_boosted, OpenThread, ProjectMemory,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
    use std::fs;

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
                resolved_chapter: 0,
            }],
            archived_threads: vec![OpenThread {
                id: "b".into(),
                text: "归档旧伏笔".into(),
                status: "open".into(),
                planted_chapter: 1,
                resolved_chapter: 0,
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
                    resolved_chapter: 0,
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
                resolved_chapter: 0,
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
