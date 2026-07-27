//! Lightweight master-outline drift notes after volume sync (no auto-rewrite).

use crate::memory::load_memory;
use crate::volume::{read_arc_outline_text, VolumeBound};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftNote {
    pub severity: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeDriftReport {
    pub volume_index: u32,
    pub notes: Vec<DriftNote>,
    pub path: String,
}

#[derive(Debug, Clone, Copy)]
pub struct DriftCheckOpts {
    /// When false, skip ending-condition ↔ rollup comparisons (rollup may be stale).
    pub memory_confirmed: bool,
}

impl Default for DriftCheckOpts {
    fn default() -> Self {
        Self {
            memory_confirmed: true,
        }
    }
}

/// Deterministic drift check after volume sync.
pub fn run_volume_drift_check(
    project_dir: &Path,
    volume: &VolumeBound,
) -> Result<VolumeDriftReport> {
    run_volume_drift_check_with(project_dir, volume, DriftCheckOpts::default())
}

pub fn run_volume_drift_check_with(
    project_dir: &Path,
    volume: &VolumeBound,
    opts: DriftCheckOpts,
) -> Result<VolumeDriftReport> {
    let master = std::fs::read_to_string(project_dir.join("artifacts/master_outline.md"))
        .unwrap_or_default();
    let mem = load_memory(project_dir);
    let rollup = mem
        .volume_rollups
        .iter()
        .find(|r| r.volume_index == volume.volume_index)
        .map(|r| r.summary.clone())
        .unwrap_or_default();
    let arc = read_arc_outline_text(project_dir, volume.volume_index.max(1))
        .or_else(|| std::fs::read_to_string(project_dir.join("artifacts/arc_outline.md")).ok())
        .unwrap_or_default();
    // Wider haystack than rollup alone (ending-condition wording rarely copies into digests).
    let delivery_hay = format!("{rollup}\n{arc}\n{}", volume.goal);

    let mut notes = Vec::new();
    let vol_label = format!("第{}卷", volume.volume_index);
    let master_mentions_vol = master.contains(&vol_label)
        || master.contains(&format!("卷{}", volume.volume_index))
        || (!volume.name.is_empty() && master.contains(&volume.name));

    let master_nonempty = master.trim().chars().count() > 40;

    // Structural: master exists but this volume is absent from 分卷骨架.
    if master_nonempty && !master_mentions_vol {
        notes.push(DriftNote {
            severity: "BLOCKER".into(),
            note: format!(
                "总纲未明显提及{vol_label}（或卷名「{}」）；建议 Studio 调用 master_planner 人工确认修订总纲（系统不会自动改写）。",
                if volume.name.is_empty() {
                    "（未命名）"
                } else {
                    &volume.name
                }
            ),
        });
    }

    if opts.memory_confirmed {
        for cond in &volume.ending_conditions {
            if cond.trim().chars().count() < 2 {
                continue;
            }
            let in_delivery = soft_match(cond, &delivery_hay);
            let in_master = soft_match(cond, &master);
            if !in_delivery && !in_master {
                notes.push(DriftNote {
                    severity: "WARN".into(),
                    note: format!(
                        "终止条件「{cond}」在卷交付物（rollup/卷纲）与总纲中均未见明显呼应；请人工核对是否已兑现或措辞不一致。"
                    ),
                });
            } else if in_delivery && !in_master && master_nonempty {
                notes.push(DriftNote {
                    severity: "WARN".into(),
                    note: format!(
                        "终止条件「{cond}」在卷交付中有呼应，但总纲对应分卷段未见；可考虑人工修订总纲。"
                    ),
                });
            }
        }
    } else {
        notes.push(DriftNote {
            severity: "INFO".into(),
            note: "未确认卷记忆（confirm_memory=false），已跳过终止条件↔rollup 对照。".into(),
        });
    }

    if notes.is_empty() {
        notes.push(DriftNote {
            severity: "INFO".into(),
            note: "未见明显总纲偏离（结构检查 + 软匹配）。".into(),
        });
    }

    let mut md = format!(
        "# 第{}卷 · 总纲偏离报告\n\n> 只读诊断，不自动改写总纲。\n\n",
        volume.volume_index
    );
    for n in &notes {
        md.push_str(&format!("- **[{}]** {}\n", n.severity, n.note));
    }
    let path = project_dir
        .join("artifacts")
        .join(format!("volume_{}_drift.md", volume.volume_index));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &md)?;

    Ok(VolumeDriftReport {
        volume_index: volume.volume_index,
        notes,
        path: path.display().to_string(),
    })
}

/// Soft match: full needle, punctuation tokens, or 3–4 char sliding windows (Chinese phrases).
fn soft_match(cond: &str, hay: &str) -> bool {
    let cond = cond.trim();
    if cond.is_empty() || hay.is_empty() {
        return false;
    }
    if hay.contains(cond) {
        return true;
    }
    for tok in cond_tokens(cond) {
        if tok.chars().count() >= 2 && hay.contains(&tok) {
            return true;
        }
    }
    let chars: Vec<char> = cond.chars().collect();
    if chars.len() < 3 {
        return chars.len() >= 2 && hay.contains(&chars.iter().collect::<String>());
    }
    // Prefer 4-char windows, then 3-char — avoids 2-char noise ("推进"等).
    for w in [4usize, 3] {
        if chars.len() < w {
            continue;
        }
        for i in 0..=chars.len() - w {
            let s: String = chars[i..i + w].iter().collect();
            if hay.contains(&s) {
                return true;
            }
        }
    }
    false
}

fn cond_tokens(cond: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in cond.chars() {
        if "，。；、：:；|/\\（）()【】[]「」『』\"' \t\n".contains(ch) {
            if cur.chars().count() >= 2 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        } else {
            cur.push(ch);
        }
    }
    if cur.chars().count() >= 2 {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::VolumeBound;
    use std::fs;

    fn sample_vol(conds: Vec<&str>) -> VolumeBound {
        VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 10,
            name: String::new(),
            ending_conditions: conds.into_iter().map(|s| s.to_string()).collect(),
            goal: "推进".into(),
            completed: true,
        }
    }

    #[test]
    fn writes_drift_file() {
        let root = std::env::temp_dir().join(format!(
            "novelx_drift_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("artifacts")).unwrap();
        fs::create_dir_all(root.join("lore")).unwrap();
        fs::write(
            root.join("artifacts/master_outline.md"),
            "# 总纲\n\n## 分卷\n\n### 第1卷\n目标：抵达落点\n",
        )
        .unwrap();
        fs::write(root.join("lore/memory.json"), r#"{"volume_rollups":[]}"#).unwrap();
        let report = run_volume_drift_check(&root, &sample_vol(vec!["抵达落点"])).unwrap();
        assert!(std::path::Path::new(&report.path).exists());
        assert!(!report.notes.iter().any(|n| n.severity == "BLOCKER"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ending_condition_mismatches_stay_warn_not_blocker() {
        let root = std::env::temp_dir().join(format!(
            "novelx_drift_warn_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("artifacts")).unwrap();
        fs::create_dir_all(root.join("lore")).unwrap();
        // Master mentions vol 1, but wording differs from ending conditions.
        fs::write(
            root.join("artifacts/master_outline.md"),
            "# 总纲\n\n## 分卷\n\n### 第1卷\n主角完成试炼并离开故土。\n更多说明填充字数。\n",
        )
        .unwrap();
        fs::write(
            root.join("lore/memory.json"),
            r#"{"volume_rollups":[{"volume_index":1,"name":"","summary":"主角离开家乡去远行","open_thread_ids":[],"chapter_end":10}]}"#,
        )
        .unwrap();
        let report = run_volume_drift_check(
            &root,
            &sample_vol(vec![
                "拿下城防指挥权",
                "揭露幕后主使身份",
                "盟友公开站队",
            ]),
        )
        .unwrap();
        let warns = report
            .notes
            .iter()
            .filter(|n| n.severity == "WARN")
            .count();
        assert!(warns >= 1);
        assert!(
            !report.notes.iter().any(|n| n.severity == "BLOCKER"),
            "ending-condition keyword misses must not escalate to BLOCKER: {:?}",
            report.notes
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn skip_rollup_checks_without_memory_confirm() {
        let root = std::env::temp_dir().join(format!(
            "novelx_drift_skip_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("artifacts")).unwrap();
        fs::create_dir_all(root.join("lore")).unwrap();
        fs::write(
            root.join("artifacts/master_outline.md"),
            "# 总纲\n\n## 分卷\n\n### 第1卷\n目标说明填充。\n",
        )
        .unwrap();
        fs::write(root.join("lore/memory.json"), r#"{"volume_rollups":[]}"#).unwrap();
        let report = run_volume_drift_check_with(
            &root,
            &sample_vol(vec!["完全不相干的终止条件甲", "完全不相干的终止条件乙"]),
            DriftCheckOpts {
                memory_confirmed: false,
            },
        )
        .unwrap();
        assert!(report.notes.iter().any(|n| n.note.contains("跳过")));
        assert!(!report.notes.iter().any(|n| {
            n.severity == "WARN" && n.note.contains("终止条件")
        }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn soft_match_hits_arc_outline_token() {
        assert!(soft_match("抵达落点并站稳脚跟", "卷纲写明：主角抵达落点"));
        assert!(!soft_match("拿下城防", "日常赶路吃饭睡觉"));
    }
}
