//! Chapter-by-chapter audit queue (persisted under projects/<name>/.novelx/).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditQueueStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
    Revised,
}

impl AuditQueueStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "待审",
            Self::Passed => "通过",
            Self::Failed => "未通过",
            Self::Skipped => "已跳过",
            Self::Revised => "已修订",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditQueueItem {
    pub chapter: u32,
    pub status: AuditQueueStatus,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditQueue {
    pub project: String,
    pub chapters: Vec<u32>,
    /// Index into `chapters` for the chapter currently being / about to be audited.
    pub index: usize,
    pub results: Vec<AuditQueueItem>,
}

impl AuditQueue {
    pub fn new(project: &str, from: u32, to: u32) -> Self {
        let from = from.max(1);
        let to = to.max(from);
        Self::from_chapters(project, (from..=to).collect())
    }

    /// Arbitrary chapter list (deduped, sorted). Used by volume L1 → deep audit.
    pub fn from_chapters(project: &str, chapters: Vec<u32>) -> Self {
        let mut chapters: Vec<u32> = chapters.into_iter().filter(|&c| c >= 1).collect();
        chapters.sort_unstable();
        chapters.dedup();
        if chapters.is_empty() {
            chapters.push(1);
        }
        let results = chapters
            .iter()
            .map(|&chapter| AuditQueueItem {
                chapter,
                status: AuditQueueStatus::Pending,
                summary: String::new(),
            })
            .collect();
        Self {
            project: project.to_string(),
            chapters,
            index: 0,
            results,
        }
    }

    pub fn current_chapter(&self) -> Option<u32> {
        self.chapters.get(self.index).copied()
    }

    pub fn set_current(&mut self, status: AuditQueueStatus, summary: impl Into<String>) {
        if let Some(item) = self.results.get_mut(self.index) {
            item.status = status;
            item.summary = summary.into();
        }
    }

    /// Advance to next pending chapter. Returns false if queue is finished.
    pub fn advance(&mut self) -> bool {
        self.index += 1;
        self.index < self.chapters.len()
    }

    pub fn is_finished(&self) -> bool {
        self.index >= self.chapters.len()
    }

    /// Codex-style todos: exactly one `in_progress` until the queue finishes.
    pub fn to_codex_todos(&self) -> Vec<serde_json::Value> {
        self.results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let (status, note) = if self.is_finished() {
                    (
                        "completed",
                        match r.status {
                            AuditQueueStatus::Skipped => "已跳过",
                            AuditQueueStatus::Revised => "已修订",
                            AuditQueueStatus::Failed => "未通过",
                            AuditQueueStatus::Passed => "通过",
                            AuditQueueStatus::Pending => "完成",
                        },
                    )
                } else if i == self.index {
                    ("in_progress", match r.status {
                        AuditQueueStatus::Failed => "未通过 · 待处理",
                        _ => "进行中",
                    })
                } else if i < self.index {
                    (
                        "completed",
                        match r.status {
                            AuditQueueStatus::Skipped => "已跳过",
                            AuditQueueStatus::Revised => "已修订",
                            AuditQueueStatus::Failed => "未通过",
                            _ => "通过",
                        },
                    )
                } else {
                    ("pending", "待审")
                };
                json!({
                    "content": format!("审校第{}章（{}）", r.chapter, note),
                    "status": status,
                })
            })
            .collect()
    }

    pub fn checklist_markdown(&self) -> String {
        let total = self.chapters.len();
        let cur = self.current_chapter();
        let mut lines = vec![format!(
            "## 审阅队列《{}》（{}/{}）",
            self.project,
            self.index.min(total) + if self.is_finished() { 0 } else { 1 },
            total
        )];
        if let Some(c) = cur {
            if !self.is_finished() {
                lines.push(format!("当前：第{c}章"));
            }
        }
        for (i, item) in self.results.iter().enumerate() {
            let mark = if !self.is_finished() && i == self.index {
                "▶"
            } else {
                match item.status {
                    AuditQueueStatus::Passed | AuditQueueStatus::Revised => "✓",
                    AuditQueueStatus::Failed => "✕",
                    AuditQueueStatus::Skipped => "–",
                    AuditQueueStatus::Pending => "·",
                }
            };
            let extra = if item.summary.is_empty() {
                String::new()
            } else {
                format!(" — {}", item.summary.chars().take(60).collect::<String>())
            };
            lines.push(format!(
                "{mark} 第{}章 · {}{extra}",
                item.chapter,
                item.status.label()
            ));
        }
        lines.join("\n")
    }

    pub fn summary_markdown(&self) -> String {
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut skipped = 0u32;
        let mut revised = 0u32;
        let mut pending = 0u32;
        for r in &self.results {
            match r.status {
                AuditQueueStatus::Passed => passed += 1,
                AuditQueueStatus::Failed => failed += 1,
                AuditQueueStatus::Skipped => skipped += 1,
                AuditQueueStatus::Revised => revised += 1,
                AuditQueueStatus::Pending => pending += 1,
            }
        }
        format!(
            "{}\n\n——\n审阅结束：通过 {passed} · 未通过 {failed} · 已修订 {revised} · 跳过 {skipped} · 未审 {pending}",
            self.checklist_markdown()
        )
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(json!({}))
    }
}

fn queue_path(projects_root: &Path, project: &str) -> PathBuf {
    projects_root
        .join(project)
        .join(".novelx")
        .join("audit_queue.json")
}

pub fn load_audit_queue(projects_root: &Path, project: &str) -> Option<AuditQueue> {
    let path = queue_path(projects_root, project);
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_audit_queue(projects_root: &Path, queue: &AuditQueue) -> Result<()> {
    let path = queue_path(projects_root, &queue.project);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(queue)?)?;
    Ok(())
}

pub fn clear_audit_queue(projects_root: &Path, project: &str) -> Result<()> {
    let path = queue_path(projects_root, project);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
