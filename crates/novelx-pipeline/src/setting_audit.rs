//! Setting-layer audit (Bible / entities / stubs / summary drift).
//! Reused by tools (`audit_setting`) and post-plot setting pass — no separate agent.

use anyhow::Result;
use novelx_llm::LlmClient;
use novelx_skills::{build_skill_injections, load_skills, SkillScope};
use serde_json::{json, Value};
use std::path::Path;

use crate::cards::collect_entity_gaps;
use crate::context::read_world_doc;

#[derive(Debug, Clone, Default)]
pub struct SettingAuditPackOpts {
    /// Completed / focus plot title (card body included when found).
    pub focus_plot: Option<String>,
    /// Include recent chapter summaries up to this chapter (inclusive).
    pub upto_chapter: Option<u32>,
    /// How many recent summaries to attach (default 5).
    pub summary_limit: usize,
}

impl SettingAuditPackOpts {
    pub fn post_plot(plot_title: &str, chapter: u32) -> Self {
        Self {
            focus_plot: Some(plot_title.to_string()),
            upto_chapter: Some(chapter),
            summary_limit: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SettingAuditResult {
    pub blocker: bool,
    /// Model failed / audit not actually run — callers must not treat as green light.
    pub skipped: bool,
    pub summary: String,
    pub report: String,
    pub raw: Value,
}

impl SettingAuditResult {
    /// Safe to auto-sync only when audit truly ran and found no BLOCKER.
    pub fn allows_auto_sync(&self) -> bool {
        !self.skipped && !self.blocker
    }
}

/// Build a pack for setting_auditor: Bible, entities, stub gaps, recent summaries, optional plot card.
pub fn build_setting_audit_pack(project_dir: &Path, opts: &SettingAuditPackOpts) -> String {
    let (_, bible) = read_world_doc(project_dir);
    let mut pack = String::new();
    pack.push_str(&format!(
        "# Bible\n{}\n",
        bible.chars().take(3000).collect::<String>()
    ));

    for group in ["characters", "items", "locations"] {
        let folder = project_dir.join("entities").join(group);
        if let Ok(rd) = std::fs::read_dir(folder) {
            for e in rd.flatten().take(12) {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    // Prefer fuller stub text so auditor can judge omissions.
                    let limit = if t.contains("complete: false")
                        || t.contains("source: volume_sync")
                        || t.contains("source: plot_sync")
                        || t.contains("## 卷末同步摘要")
                        || t.contains("## 剧情同步摘要")
                    {
                        900
                    } else {
                        400
                    };
                    pack.push_str(&format!(
                        "\n## [{group}] {}\n{}\n",
                        e.file_name().to_string_lossy(),
                        t.chars().take(limit).collect::<String>()
                    ));
                }
            }
        }
    }

    let gaps = collect_entity_gaps(project_dir);
    if gaps.is_empty() {
        pack.push_str("\n# 设定缺口（启发式）\n（无）\n");
    } else {
        pack.push_str("\n# 设定缺口（启发式）\n");
        for g in gaps.iter().take(40) {
            pack.push_str(&format!("- {g}\n"));
        }
    }

    if let Some(ch) = opts.upto_chapter {
        let limit = if opts.summary_limit == 0 {
            5
        } else {
            opts.summary_limit
        };
        let start = ch.saturating_sub(limit.saturating_sub(1) as u32).max(1);
        pack.push_str(&format!("\n# 近章摘要（第{start}–{ch}章）\n"));
        let mut any = false;
        for n in start..=ch {
            let path = project_dir
                .join("chapters")
                .join(format!("{n:03}"))
                .join("summary.json");
            if let Ok(text) = std::fs::read_to_string(path) {
                any = true;
                pack.push_str(&format!(
                    "\n## 第{n}章\n{}\n",
                    text.chars().take(900).collect::<String>()
                ));
            }
        }
        if !any {
            pack.push_str("（尚无 summary.json）\n");
        }
    }

    if let Some(title) = opts.focus_plot.as_deref() {
        if let Some(body) = read_plot_card_text(project_dir, title) {
            pack.push_str(&format!(
                "\n# 焦点剧情卡「{title}」\n{}\n",
                body.chars().take(2500).collect::<String>()
            ));
        }
    }

    pack
}

fn read_plot_card_text(project_dir: &Path, title: &str) -> Option<String> {
    let dir = project_dir.join("plots");
    let rd = std::fs::read_dir(&dir).ok()?;
    let needle = title.trim();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem == needle
            || text.contains(&format!("title: {needle}"))
            || text.contains(&format!("# {needle}"))
        {
            return Some(text);
        }
    }
    None
}

fn load_setting_auditor_skill(config_root: &Path) -> String {
    let root = config_root.join("skills/agents");
    let outcome = load_skills(&[(SkillScope::Agent, root)]);
    let inj = build_skill_injections(&outcome.skills, &["setting-auditor".into()]);
    inj.into_iter()
        .next()
        .map(|i| i.body)
        .unwrap_or_else(|| "你是设定冲突审计员。".into())
}

/// Pack for structure mutations (章纲 / 总纲 / 卷纲) before apply.
pub fn build_structure_audit_candidate(
    project_dir: &Path,
    label: &str,
    candidate: &str,
) -> String {
    let (_, bible) = read_world_doc(project_dir);
    let master = std::fs::read_to_string(project_dir.join("artifacts/master_outline.md"))
        .unwrap_or_default();
    let mut arcs = String::new();
    for vol in crate::volume::list_arc_outline_volumes(project_dir).into_iter().take(4) {
        if let Some(t) = crate::volume::read_arc_outline_text(project_dir, vol) {
            arcs.push_str(&format!(
                "\n## 第{vol}卷卷纲节选\n{}\n",
                t.chars().take(900).collect::<String>()
            ));
        }
    }
    format!(
        "# 结构审计对象：{label}\n\n## 候选内容\n{}\n\n## Canon·Bible 节选\n{}\n\n## 总纲节选\n{}\n{arcs}\n\
         请检查：与总纲/卷纲/Bible 冲突、遗漏关键设定、名单与能力规则不一致。\
         BLOCKER 仅用于硬冲突（能力/身份/已定事实矛盾）；风格建议用 WARNING。",
        candidate.chars().take(5000).collect::<String>(),
        bible.chars().take(2000).collect::<String>(),
        master.chars().take(1500).collect::<String>(),
    )
}

/// Run setting_auditor LLM over a candidate / pack.
/// Model errors → `skipped=true` (fail-open for *writes* that check `blocker` only must also check `skipped`
/// or use [`SettingAuditResult::allows_auto_sync`]).
pub async fn run_setting_audit(
    project_dir: &Path,
    config_root: &Path,
    llm: &LlmClient,
    label: &str,
    candidate: &str,
) -> Result<SettingAuditResult> {
    let (_, bible) = read_world_doc(project_dir);
    let skill = load_setting_auditor_skill(config_root);
    let prompt = format!(
        "审计对象：{label}\n\n## 拟写入或巡检包\n{}\n\n## 现有 Bible 节选\n{}\n\n\
         除冲突外，须检查遗漏（stub 缺口、摘要提及但无卡）与异常（摘要与卡漂移、退场角色再活跃等）。\
         只输出 JSON：passed/summary/conflicts（含 type/severity/title/detail/suggestion）。",
        candidate.chars().take(8000).collect::<String>(),
        bible.chars().take(2500).collect::<String>()
    );
    let (raw_text, model_failed) = match llm
        .complete(
            &skill,
            &prompt,
            Some(&llm.model_for_agent("setting_auditor")),
        )
        .await
    {
        Ok(s) if !s.trim().is_empty() => (s, false),
        Ok(_) => (
            json!({"passed":true,"summary":"审计跳过（空响应）","conflicts":[]}).to_string(),
            true,
        ),
        Err(_) => (
            json!({"passed":true,"summary":"审计跳过（模型失败）","conflicts":[]}).to_string(),
            true,
        ),
    };
    let parsed = extract_json_value(&raw_text);
    let parse_failed = parsed.is_none();
    let v = parsed.unwrap_or_else(|| {
        json!({
            "passed": true,
            "summary": raw_text.chars().take(200).collect::<String>(),
            "conflicts": []
        })
    });
    let summary = v
        .get("summary")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let skipped = model_failed
        || parse_failed
        || summary.contains("审计跳过");
    let blocker = !skipped
        && v
            .get("conflicts")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .any(|x| x.get("severity").and_then(|s| s.as_str()) == Some("BLOCKER"))
            })
            .unwrap_or(false);
    Ok(SettingAuditResult {
        blocker,
        skipped,
        summary,
        report: raw_text.chars().take(1600).collect(),
        raw: v,
    })
}

fn extract_json_value(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "novelx-setting-audit-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("artifacts")).unwrap();
        fs::create_dir_all(root.join("entities/characters")).unwrap();
        fs::create_dir_all(root.join("chapters/001")).unwrap();
        fs::create_dir_all(root.join("plots")).unwrap();
        root
    }

    #[test]
    fn allows_auto_sync_requires_real_pass() {
        let ok = SettingAuditResult {
            blocker: false,
            skipped: false,
            summary: "通过".into(),
            report: String::new(),
            raw: json!({}),
        };
        assert!(ok.allows_auto_sync());
        let skipped = SettingAuditResult {
            blocker: false,
            skipped: true,
            summary: "审计跳过（模型失败）".into(),
            report: String::new(),
            raw: json!({}),
        };
        assert!(!skipped.allows_auto_sync());
        let blocked = SettingAuditResult {
            blocker: true,
            skipped: false,
            summary: "有冲突".into(),
            report: String::new(),
            raw: json!({}),
        };
        assert!(!blocked.allows_auto_sync());
    }

    #[test]
    fn pack_includes_gaps_and_summaries() {
        let root = tmp();
        fs::write(
            root.join("artifacts/bible.md"),
            "# 世界观 Bible\n\n## 0. 一句话世界\n测。\n\n## 1. 时代与叙事框架\n测。\n\n## 2. 全局势力与阵营\n测。\n\n## 7. 开放问题\n测。\n",
        )
        .unwrap();
        fs::write(
            root.join("entities/characters/甲.md"),
            "---\nname: 甲\ncomplete: false\nsource: plot_sync\n---\n\n# 甲\n\n## 剧情同步摘要\n短 stub。\n",
        )
        .unwrap();
        fs::write(
            root.join("chapters/001/summary.json"),
            r#"{"summary":"甲抵达落点。"}"#,
        )
        .unwrap();
        fs::write(
            root.join("plots/入局.md"),
            "---\ntitle: 入局\nstatus: completed\n---\n\n# 入局\n\n## 收束条件\n已入局\n",
        )
        .unwrap();
        let pack = build_setting_audit_pack(
            &root,
            &SettingAuditPackOpts::post_plot("入局", 1),
        );
        assert!(pack.contains("设定缺口"));
        assert!(pack.contains("近章摘要"));
        assert!(pack.contains("焦点剧情卡"));
        assert!(pack.contains("甲"));
        let _ = fs::remove_dir_all(&root);
    }
}
