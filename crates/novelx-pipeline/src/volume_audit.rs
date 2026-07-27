//! Volume-level review (L1): scan chapter summaries, suggest deep-audit chapters.
//! Does not read full chapter drafts — use audit_chapters for L2/L3.

use crate::volume::{volume_chapter_span, VolumeBound};
use anyhow::Result;
use novelx_llm::LlmClient;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

const VOLUME_AUDITOR_SYSTEM: &str = r#"你是小说「卷级」复盘员。只根据本卷各章摘要、卷纲终止条件与滚动记忆做跨章检查，不要假装读过全文。
输出一份 JSON（不要 Markdown 围栏）：
{
  "passed": true,
  "summary": "本卷复盘结论（2–5句）",
  "issues":[{"type":"TIMELINE|CHARACTER|ITEM|PLOT|HOOK|OUTLINE","severity":"BLOCKER|WARN","description":"…","chapters":[1,2]}],
  "suggested_chapters":[1,3,5],
  "ending_check":"卷终止条件是否兑现（简述）"
}
规则：
- suggested_chapters：建议深读正文的章号（通常 3–8 个，可含卷首/卷末/问题章）；必须是本卷范围内已有摘要的章。
- 无严重问题时 passed=true，suggested_chapters 仍可含卷首+卷末抽样。
- 只输出 JSON。"#;

#[derive(Debug, Clone)]
pub struct VolumeAuditReport {
    pub volume_index: u32,
    pub from: u32,
    pub to: u32,
    pub passed: bool,
    pub report_markdown: String,
    pub suggested_chapters: Vec<u32>,
    pub issues: Vec<Value>,
}

/// Heuristic pick: first/last + failed prior audits + missing summaries neighbors.
pub fn heuristic_deep_audit_chapters(project_dir: &Path, volume: &VolumeBound) -> Vec<u32> {
    let upto = if volume.end_chapter > 0 {
        volume.end_chapter
    } else {
        max_chapter_with_content(project_dir).max(1)
    };
    let (from, to) = volume_chapter_span(volume, upto);
    if from > to {
        return Vec::new();
    }
    let mut set = BTreeSet::new();
    set.insert(from);
    set.insert(to);
    for ch in from..=to {
        let dir = project_dir.join("chapters").join(format!("{ch:03}"));
        let audit_path = dir.join("audit.json");
        if let Ok(text) = std::fs::read_to_string(&audit_path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v.get("passed").and_then(|x| x.as_bool()) == Some(false) {
                    set.insert(ch);
                }
            }
        }
        if !dir.join("summary.json").exists() && dir.join("draft.md").exists() {
            set.insert(ch);
        }
    }
    // Cap heuristic list.
    let mut out: Vec<u32> = set.into_iter().collect();
    if out.len() > 8 {
        let mid = out[out.len() / 2];
        out = vec![from, mid, to];
        out.sort_unstable();
        out.dedup();
    }
    out
}

pub fn gather_volume_audit_pack(project_dir: &Path, volume: &VolumeBound) -> Result<String> {
    let upto = if volume.end_chapter > 0 {
        volume.end_chapter
    } else {
        max_chapter_with_content(project_dir).max(1)
    };
    let (from, to) = volume_chapter_span(volume, upto);
    let mut parts = Vec::new();
    parts.push(format!(
        "# 卷范围\nvolume_index={} 章 {}–{}\ngoal: {}\nending_conditions:\n{}",
        volume.volume_index,
        from,
        to,
        volume.goal,
        if volume.ending_conditions.is_empty() {
            "- （无）".into()
        } else {
            volume
                .ending_conditions
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    ));

    let mut summary_parts = Vec::new();
    for ch in from..=to {
        let path = project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("summary.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let excerpt: String = text.chars().take(900).collect();
            summary_parts.push(format!("## 第{ch}章\n{excerpt}"));
        } else if project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("draft.md")
            .exists()
        {
            summary_parts.push(format!("## 第{ch}章\n（有正文但无 summary.json）"));
        }
    }
    if summary_parts.is_empty() {
        summary_parts.push("（本卷尚无章节摘要）".into());
    }
    parts.push(format!("# 各章摘要\n{}", summary_parts.join("\n\n")));

    let arc_vol = volume.volume_index.max(1);
    let arc = crate::volume::read_arc_outline_text(project_dir, arc_vol)
        .or_else(|| std::fs::read_to_string(project_dir.join("artifacts/arc_outline.md")).ok())
        .unwrap_or_default();
    if arc.trim().chars().count() > 20 {
        let excerpt: String = arc.chars().take(1800).collect();
        parts.push(format!("# 卷纲节选\n{excerpt}"));
    }

    let mem = crate::memory::load_memory(project_dir);
    if let Ok(s) = serde_json::to_string_pretty(&mem) {
        let excerpt: String = s.chars().take(1800).collect();
        parts.push(format!("# 滚动记忆\n{excerpt}"));
    }

    Ok(parts.join("\n\n"))
}

pub async fn run_volume_audit(
    project_dir: &Path,
    volume: &VolumeBound,
    llm: Arc<LlmClient>,
    skill: &str,
) -> Result<VolumeAuditReport> {
    let upto = if volume.end_chapter > 0 {
        volume.end_chapter
    } else {
        max_chapter_with_content(project_dir).max(1)
    };
    let (from, to) = volume_chapter_span(volume, upto);
    let pack = gather_volume_audit_pack(project_dir, volume)?;
    let heuristic = heuristic_deep_audit_chapters(project_dir, volume);

    let user = format!(
        "请复盘第{}卷（第{from}–{to}章）。\n\
         启发式建议深审章（可调整）：{}\n\n{pack}",
        volume.volume_index,
        heuristic
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let raw = llm
        .complete_limited(
            if skill.trim().is_empty() {
                VOLUME_AUDITOR_SYSTEM
            } else {
                skill
            },
            &user,
            Some(&llm.model_for_agent("volume_auditor")),
            Some(llm.max_tokens_for_agent("volume_auditor").max(2048)),
        )
        .await
        .unwrap_or_default();

    let parsed = extract_json_object(&raw).unwrap_or_else(|| {
        json!({
            "passed": true,
            "summary": if raw.trim().is_empty() {
                "模型未返回有效复盘，已回退启发式深审章列表。"
            } else {
                "模型输出非 JSON，已保留原文并回退启发式深审章列表。"
            },
            "issues": [],
            "suggested_chapters": heuristic,
            "ending_check": ""
        })
    });

    let mut suggested = parse_chapter_list(parsed.get("suggested_chapters")).unwrap_or_default();
    if suggested.is_empty() {
        suggested = heuristic.clone();
    }
    suggested.retain(|&c| c >= from && c <= to);
    suggested.sort_unstable();
    suggested.dedup();
    // Always ensure first/last when volume has chapters.
    if from <= to {
        if !suggested.contains(&from) {
            suggested.insert(0, from);
        }
        if !suggested.contains(&to) {
            suggested.push(to);
        }
        suggested.sort_unstable();
        suggested.dedup();
    }
    if suggested.len() > 10 {
        suggested.truncate(10);
    }

    let passed = parsed
        .get("passed")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let issues = parsed
        .get("issues")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let summary = parsed
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let ending = parsed
        .get("ending_check")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let mut md = format!(
        "# 第{}卷复盘（L1·摘要层）\n\n范围：第{from}–{to}章\n\n## 结论\n\n{}\n",
        volume.volume_index,
        if summary.is_empty() {
            if passed {
                "未见阻断级跨章问题（基于摘要）。"
            } else {
                "存在需关注的跨章问题（见下）。"
            }
        } else {
            summary.as_str()
        }
    );
    if !ending.is_empty() {
        md.push_str(&format!("\n## 卷终止条件\n\n{ending}\n"));
    }
    if !issues.is_empty() {
        md.push_str("\n## 问题\n\n");
        for (i, iss) in issues.iter().enumerate() {
            let ty = iss.get("type").and_then(|x| x.as_str()).unwrap_or("?");
            let sev = iss
                .get("severity")
                .and_then(|x| x.as_str())
                .unwrap_or("WARN");
            let desc = iss
                .get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let chs = parse_chapter_list(iss.get("chapters"))
                .map(|v| {
                    v.iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            md.push_str(&format!(
                "{}. [{}][{}] {}{}\n",
                i + 1,
                sev,
                ty,
                desc,
                if chs.is_empty() {
                    String::new()
                } else {
                    format!("（章 {chs}）")
                }
            ));
        }
    }
    md.push_str(&format!(
        "\n## 建议深审章\n\n{}\n\n> 深审将走既有逐章审阅队列（读正文）。全量终检请显式「审阅第{from}-{to}章」。\n",
        if suggested.is_empty() {
            "（无）".into()
        } else {
            suggested
                .iter()
                .map(|c| format!("- 第{c}章"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    ));

    Ok(VolumeAuditReport {
        volume_index: volume.volume_index,
        from,
        to,
        passed,
        report_markdown: md,
        suggested_chapters: suggested,
        issues,
    })
}

fn max_chapter_with_content(project_dir: &Path) -> u32 {
    let root = project_dir.join("chapters");
    let Ok(rd) = std::fs::read_dir(&root) else {
        return 1;
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .max()
        .unwrap_or(1)
}

fn parse_chapter_list(v: Option<&Value>) -> Option<Vec<u32>> {
    let arr = v?.as_array()?;
    let mut out = Vec::new();
    for x in arr {
        if let Some(n) = x.as_u64() {
            if n >= 1 {
                out.push(n as u32);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn extract_json_object(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    let unfenced = if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let rest = rest.trim_start_matches('\n');
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    };
    if let Ok(v) = serde_json::from_str::<Value>(unfenced) {
        return Some(v);
    }
    let start = unfenced.find('{')?;
    let end = unfenced.rfind('}')?;
    if end > start {
        serde_json::from_str(&unfenced[start..=end]).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::VolumeBound;

    #[test]
    fn heuristic_picks_ends_and_failed_audit() {
        let root = std::env::temp_dir().join(format!(
            "novelx_vol_audit_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("chapters/001")).unwrap();
        std::fs::create_dir_all(root.join("chapters/002")).unwrap();
        std::fs::create_dir_all(root.join("chapters/005")).unwrap();
        std::fs::write(root.join("chapters/001/summary.json"), r#"{"event_summary":"a"}"#)
            .unwrap();
        std::fs::write(root.join("chapters/002/summary.json"), r#"{"event_summary":"b"}"#)
            .unwrap();
        std::fs::write(root.join("chapters/005/summary.json"), r#"{"event_summary":"c"}"#)
            .unwrap();
        std::fs::write(
            root.join("chapters/002/audit.json"),
            r#"{"passed":false,"issues":[]}"#,
        )
        .unwrap();
        let vol = VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 5,
            name: "测试卷".into(),
            ending_conditions: vec!["收束".into()],
            goal: String::new(),
            completed: false,
        };
        let chs = heuristic_deep_audit_chapters(&root, &vol);
        assert!(chs.contains(&1));
        assert!(chs.contains(&2));
        assert!(chs.contains(&5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gather_pack_includes_summaries() {
        let root = std::env::temp_dir().join(format!(
            "novelx_vol_pack_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("chapters/003")).unwrap();
        std::fs::write(
            root.join("chapters/003/summary.json"),
            r#"{"event_summary":"主角抵达边境","ending_hook":"门外有脚步"}"#,
        )
        .unwrap();
        let vol = VolumeBound {
            volume_index: 1,
            start_chapter: 3,
            end_chapter: 3,
            name: String::new(),
            ending_conditions: vec![],
            goal: "推进".into(),
            completed: false,
        };
        let pack = gather_volume_audit_pack(&root, &vol).unwrap();
        assert!(pack.contains("第3章"));
        assert!(pack.contains("边境"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
