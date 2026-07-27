//! Foreshadow index: durable dangling list for CanonContext + tracker read-back.

use crate::cards::truncate_chars;
use crate::memory::{load_memory, OpenThread, ProjectMemory};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForeshadowIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_chapter: u32,
    /// Hot + archived open threads (newest planted first).
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
        return rebuild_index_from_memory(&load_memory(project_dir));
    };
    serde_json::from_str(&text).unwrap_or_else(|_| rebuild_index_from_memory(&load_memory(project_dir)))
}

pub fn rebuild_foreshadow_index(project_dir: &Path) -> Result<ForeshadowIndex> {
    let mem = load_memory(project_dir);
    let idx = rebuild_index_from_memory(&mem);
    let path = foreshadow_index_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&idx).context("serialize foreshadow index")?;
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(idx)
}

fn rebuild_index_from_memory(mem: &ProjectMemory) -> ForeshadowIndex {
    let mut dangling: Vec<OpenThread> = mem
        .open_threads
        .iter()
        .chain(mem.archived_threads.iter())
        .filter(|t| t.status == "open" || t.status.is_empty())
        .cloned()
        .collect();
    dangling.sort_by_key(|t| std::cmp::Reverse(t.planted_chapter));
    // Dedup by id/text.
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

/// Format dangling foreshadows for CanonContext / tracker prompts.
pub fn format_dangling_for_context(project_dir: &Path, limit: usize) -> String {
    let idx = load_foreshadow_index(project_dir);
    if idx.dangling.is_empty() || limit == 0 {
        return String::new();
    }
    let lines: Vec<String> = idx
        .dangling
        .iter()
        .take(limit)
        .map(|t| {
            let plant = if t.planted_chapter > 0 {
                format!("（埋于第{}章）", t.planted_chapter)
            } else {
                String::new()
            };
            format!("- [{}] {}{}", t.id, truncate_chars(&t.text, 80), plant)
        })
        .collect();
    format!(
        "未回收伏笔（须承接或明确延期，勿无故遗忘）：\n{}",
        lines.join("\n")
    )
}

/// Compact list for foreshadow_tracker user prompt (includes ids).
pub fn format_dangling_for_tracker(project_dir: &Path, limit: usize) -> String {
    let idx = load_foreshadow_index(project_dir);
    if idx.dangling.is_empty() {
        return "（当前无登记未收伏笔）".into();
    }
    idx.dangling
        .iter()
        .take(limit)
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
    use crate::memory::{save_memory, OpenThread, ProjectMemory};
    use std::fs;

    #[test]
    fn rebuild_includes_archived() {
        let dir = std::env::temp_dir().join("novelx-fs-index");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        let mem = ProjectMemory {
            last_chapter: 5,
            open_threads: vec![OpenThread {
                id: "a".into(),
                text: "热窗口伏笔".into(),
                status: "open".into(),
                planted_chapter: 5,
                resolved_chapter: 0,
            }],
            archived_threads: vec![OpenThread {
                id: "b".into(),
                text: "归档伏笔".into(),
                status: "open".into(),
                planted_chapter: 1,
                resolved_chapter: 0,
            }],
            ..Default::default()
        };
        save_memory(&dir, &mem).unwrap();
        let idx = rebuild_foreshadow_index(&dir).unwrap();
        assert_eq!(idx.dangling.len(), 2);
        assert!(idx.dangling.iter().any(|t| t.text.contains("归档")));
        let md = format_dangling_for_context(&dir, 8);
        assert!(md.contains("归档伏笔"));
        let _ = fs::remove_dir_all(&dir);
    }
}
