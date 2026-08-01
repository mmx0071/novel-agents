//! Deterministic volume-memory checklist before confirm_volume_memory.

use crate::cards::load_markdown_cards;
use crate::foreshadow::{foreshadow_debt_breakdown, load_foreshadow_index};
use crate::memory::load_memory;
use crate::project::{list_chapter_numbers, read_chapter_draft};
use crate::schemas::draft_body_chars;
use novelx_harness::{ChapterBudget, LengthAssessment};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub severity: String, // OK | WARN | BLOCKER
    pub code: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMemoryChecklist {
    pub volume_index: u32,
    pub items: Vec<ChecklistItem>,
    pub blocker_count: usize,
    pub warn_count: usize,
}

impl VolumeMemoryChecklist {
    pub fn summary_markdown(&self) -> String {
        if self.items.is_empty() {
            return "卷记忆核对：未发现异常。".into();
        }
        let mut lines = vec![format!(
            "卷记忆核对（第{}卷）：{} 阻断 · {} 警告",
            self.volume_index, self.blocker_count, self.warn_count
        )];
        for it in &self.items {
            lines.push(format!("- [{}] {} — {}", it.severity, it.code, it.note));
        }
        lines.join("\n")
    }
}

/// Run deterministic checks for a volume chapter span before confirming rollup.
pub fn run_volume_memory_checklist(
    project_dir: &Path,
    volume_index: u32,
    start_chapter: u32,
    end_chapter: u32,
) -> VolumeMemoryChecklist {
    let mut items = Vec::new();
    let from = start_chapter.max(1);
    let to = end_chapter.max(from);
    let budget = ChapterBudget::default();

    // 1) Soft/hard short rate in volume span.
    let mut soft = 0u32;
    let mut hard = 0u32;
    let mut ok = 0u32;
    for ch in list_chapter_numbers(project_dir) {
        if ch < from || ch > to {
            continue;
        }
        let Some(draft) = read_chapter_draft(project_dir, ch)
            .or_else(|| crate::cold_archive::read_chapter_draft_resolved(project_dir, ch))
        else {
            continue;
        };
        match budget.assess_body_chars(draft_body_chars(&draft)) {
            LengthAssessment::HardShort => hard += 1,
            LengthAssessment::SoftShort => soft += 1,
            LengthAssessment::HardLong | LengthAssessment::SoftLong | LengthAssessment::Ok => {
                ok += 1
            }
        }
    }
    let n = soft + hard + ok;
    if hard > 0 {
        items.push(ChecklistItem {
            severity: "WARN".into(),
            code: "hard_short_in_volume".into(),
            note: format!("本卷有 {hard} 章曾低于硬门字数（已发布则可能为历史稿）"),
        });
    }
    if n > 0 && (soft as f64 / n as f64) > 0.35 {
        items.push(ChecklistItem {
            severity: "WARN".into(),
            code: "soft_short_rate_high".into(),
            note: format!(
                "本卷偏短章比例偏高（{soft}/{n}）；后续卷建议盯字数门控"
            ),
        });
    }

    // 2) Open foreshadow debt (inventory + pressure tiers).
    let idx = load_foreshadow_index(project_dir);
    let dangling = idx.dangling.len();
    let state = crate::project::load_project_state(project_dir).ok();
    let current = state
        .as_ref()
        .map(|s| s.next_chapter.max(1))
        .unwrap_or(1);
    let lf = crate::volume_audit_gate::find_config_root(project_dir)
        .map(|r| novelx_harness::LongformConfig::load_from_config_root(&r))
        .unwrap_or_default();
    let debt = foreshadow_debt_breakdown(&idx.dangling, current, &lf.foreshadow_debt);
    if debt.pressure > 24 {
        items.push(ChecklistItem {
            severity: "WARN".into(),
            code: "foreshadow_debt".into(),
            note: format!(
                "近债/压力伏笔约 {} 条（总量 {dangling}，远期 {}），建议优先回收近期应兑现线索",
                debt.pressure, debt.far
            ),
        });
    } else if dangling > 0 {
        items.push(ChecklistItem {
            severity: "OK".into(),
            code: "foreshadow_open".into(),
            note: format!(
                "开放伏笔 {dangling} 条（近债 {} / 远期 {}，可控）",
                debt.pressure, debt.far
            ),
        });
    }

    // 3) Character cards missing status field entirely.
    let chars = load_markdown_cards(&project_dir.join("entities/characters"), "characters");
    let mut missing_status = 0usize;
    for c in &chars {
        let raw = c.meta.get("status").map(|s| s.trim()).unwrap_or("");
        if raw.is_empty() {
            missing_status += 1;
        }
    }
    if missing_status > 0 {
        items.push(ChecklistItem {
            severity: "WARN".into(),
            code: "entity_status_missing".into(),
            note: format!("有 {missing_status} 张人物卡缺少 status 字段，卷记忆确认前建议补齐"),
        });
    }

    // 4) Hot facts / open threads sanity.
    let mem = load_memory(project_dir);
    let open_hot = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .count();
    if open_hot > 40 {
        items.push(ChecklistItem {
            severity: "WARN".into(),
            code: "open_threads_hot".into(),
            note: format!("热区开放线索 {open_hot} 条偏多，确认 rollup 时请点名本卷关键线"),
        });
    }

    let blocker_count = items.iter().filter(|i| i.severity == "BLOCKER").count();
    let warn_count = items.iter().filter(|i| i.severity == "WARN").count();
    VolumeMemoryChecklist {
        volume_index,
        items,
        blocker_count,
        warn_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn checklist_flags_missing_status() {
        let dir = std::env::temp_dir().join("novelx-vol-checklist");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("entities/characters")).unwrap();
        fs::create_dir_all(dir.join("chapters/001")).unwrap();
        fs::create_dir_all(dir.join("lore")).unwrap();
        fs::write(
            dir.join("entities/characters/主角.md"),
            "---\nname: 主角\n---\n# 主角\n",
        )
        .unwrap();
        fs::write(
            dir.join("chapters/001/draft.md"),
            &format!("# 第1章\n\n{}\n", "抵达落点。".repeat(500)),
        )
        .unwrap();
        let c = run_volume_memory_checklist(&dir, 1, 1, 1);
        assert!(
            c.items.iter().any(|i| i.code == "entity_status_missing"),
            "{:?}",
            c.items
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
