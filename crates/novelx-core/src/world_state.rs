//! Prompt world-state sections — Codex-inspired ordering.
//!
//! Capabilities (skills catalog) are rendered before permission / tool
//! constraints so the model sees what it can do before what it must not.

use novelx_skills::{SkillInjection, SkillMetadata, SkillRenderReport};

/// Fixed section order for the Studio / worker system prompt.
///
/// Catalog (capabilities metadata) precedes constraints; hard tool/gate
/// constraints stay before activated skill bodies so a large `studio.md`
/// cannot bury revise / offer_decisions rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionId {
    /// Agent identity / role framing.
    Identity,
    /// Bound project and session facts.
    Project,
    /// Progressive skill catalog (metadata only).
    SkillsCatalog,
    /// Tool-calling and gate constraints.
    Constraints,
    /// Activated skill bodies (studio / role / explicit).
    ActivatedSkills,
}

#[derive(Debug, Clone)]
pub struct WorldState {
    sections: Vec<(SectionId, String)>,
}

impl WorldState {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    pub fn set(&mut self, id: SectionId, body: impl Into<String>) {
        let body = body.into();
        if body.trim().is_empty() {
            self.sections.retain(|(s, _)| *s != id);
            return;
        }
        if let Some((_, existing)) = self.sections.iter_mut().find(|(s, _)| *s == id) {
            *existing = body;
        } else {
            self.sections.push((id, body));
            self.sections.sort_by_key(|(s, _)| *s);
        }
    }

    pub fn render(&self) -> String {
        self.sections
            .iter()
            .map(|(_, body)| body.as_str())
            .filter(|b| !b.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

impl Default for WorldState {
    fn default() -> Self {
        Self::new()
    }
}

/// Inputs for assembling the Studio / subagent system prompt.
pub struct SystemPromptParts<'a> {
    pub is_root: bool,
    pub project_bind: String,
    pub skills_catalog: String,
    pub catalog_report: SkillRenderReport,
    pub studio_body: &'a str,
    pub novel_draft_body: &'a str,
    pub injections: &'a [SkillInjection],
}

/// Build the ordered system prompt. SubAgents skip Studio orchestration
/// instructions (developer-instruction boundary).
pub fn build_system_prompt(parts: SystemPromptParts<'_>) -> (String, SkillRenderReport) {
    let mut ws = WorldState::new();

    if parts.is_root {
        ws.set(
            SectionId::Identity,
            "你是 NovelX，小说创作编排 Agent。",
        );
        ws.set(SectionId::Project, parts.project_bind);
        ws.set(SectionId::SkillsCatalog, parts.skills_catalog);
        ws.set(
            SectionId::Constraints,
            "优先局部修订正文，避免无必要时全文重写。\n\
             需要操作项目时，必须通过 API function calling 调用工具，\
             不要输出 XML、<function_calls>、<invoke> 或伪代码。\n\
             工具参数使用：project（项目名）、chapter（章节号整数）、instructions（修订说明）。\n\
             字数太少/扩写/重写/修正章节必须调用 revise_chapter，禁止用 audit_chapter 代替写章。\n\
             整卷复盘（审这一卷/卷末复盘）必须调用 audit_volume（摘要层），不要默认 audit_chapters 扫整卷。\n\
             审校多章（如1-8章）或「按建议深审」必须调用 audit_chapters（可用 chapters 列表），禁止同轮多次 audit_chapter。\n\
             递进式多步任务（多章审阅、批写、卷交接等多步）应维护 Codex 式 To-dos：列出清单 → 逐项执行 → 完成一项勾掉一项；\
             审阅队列：一致性通过后自动审下一章；评审团/修订复审必须走 audit_chapters continue（勿只用 audit_chapter）；\
             队列未结束时禁止对用户说「继续创作/审完了」。\n\
             单章审校用 audit_chapter。通过（含仅有 P1/P2）→ 勿称未通过、勿伪造审批卡；用户要改则 revise_chapter。\
             未通过 → 立即 offer_decisions（按 issue_id 给出修某条/修全部阻断/接受等），不要只给「按审校局部修订」。\
             禁止在正文里自拟编号审批卡；决策卡只经 offer_decisions / 服务端 open_gate。\n\
             操作轮收尾：有落盘/门控进展时，须先写用户可见小结与建议，再 offer_decisions(kind=studio_next)；\
             禁止工具成功后无正文无卡结束；prompt 须含本轮小结与推荐处理。",
        );

        let mut activated = String::new();
        if !parts.studio_body.trim().is_empty() {
            activated.push_str("# Skill: studio\n\n");
            activated.push_str(parts.studio_body.trim());
        }
        if !parts.novel_draft_body.trim().is_empty() {
            if !activated.is_empty() {
                activated.push_str("\n\n");
            }
            activated.push_str("# Skill: novel-draft\n\n");
            activated.push_str(parts.novel_draft_body.trim());
        }
        for inj in parts.injections {
            if inj.name == "studio" || inj.name == "novel-draft" {
                continue;
            }
            if !activated.is_empty() {
                activated.push_str("\n\n");
            }
            activated.push_str(&format!("# Skill: {}\n\n{}", inj.name, inj.body));
        }
        ws.set(SectionId::ActivatedSkills, activated);
    } else {
        // SubAgent worker: role skill only — no Studio orchestration inheritance.
        ws.set(
            SectionId::Identity,
            "你是 NovelX 子 Agent（角色执行者）。只完成当前任务，不要承接 Studio 编排职责。",
        );
        if !parts.project_bind.trim().is_empty() {
            ws.set(SectionId::Project, parts.project_bind);
        }
        ws.set(SectionId::SkillsCatalog, parts.skills_catalog);
        ws.set(
            SectionId::Constraints,
            "需要操作项目时，必须通过 API function calling 调用工具，\
             不要输出 XML、<function_calls>、<invoke> 或伪代码。\n\
             完成任务后给出简洁结果摘要；不要代替用户做跨角色编排决策。",
        );

        let mut activated = String::new();
        for inj in parts.injections {
            if inj.name == "studio" || inj.name == "novel-draft" {
                continue;
            }
            if !activated.is_empty() {
                activated.push_str("\n\n");
            }
            activated.push_str(&format!("# Skill: {}\n\n{}", inj.name, inj.body));
        }
        ws.set(SectionId::ActivatedSkills, activated);
    }

    (ws.render(), parts.catalog_report)
}

/// Studio skill-catalog char budget.
///
/// Prefer an explicit Studio cap (legacy 4000) so catalog growth does not
/// silently double when no model context window is wired yet. When a window
/// is known, use the model-aware cap but never exceed the Studio ceiling.
pub const STUDIO_SKILL_CATALOG_CHAR_BUDGET: usize = 4_000;

pub fn studio_skill_catalog_budget(context_window_tokens: Option<usize>) -> usize {
    match context_window_tokens {
        None => STUDIO_SKILL_CATALOG_CHAR_BUDGET,
        Some(window) => novelx_skills::capped_skill_metadata_char_budget(Some(window))
            .min(STUDIO_SKILL_CATALOG_CHAR_BUDGET),
    }
}

/// Filter catalog entries shown to a subagent (prefer activated + shared).
pub fn catalog_skills_for_session<'a>(
    all: &'a [SkillMetadata],
    activate: &[String],
    is_root: bool,
) -> Vec<&'a SkillMetadata> {
    if is_root {
        return all.iter().collect();
    }
    let activate_keys: Vec<String> = activate
        .iter()
        .map(|n| n.replace('_', "-"))
        .collect();
    all.iter()
        .filter(|s| {
            let key = s.name.replace('_', "-");
            activate_keys.iter().any(|a| a == &key)
                || matches!(
                    s.name.as_str(),
                    "prose-pitfalls" | "content-formats" | "volume-lifecycle"
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn meta(name: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.into(),
            description: "d".into(),
            path: PathBuf::from(format!("/tmp/{name}.md")),
            scope: novelx_skills::SkillScope::Agent,
        }
    }

    #[test]
    fn skills_catalog_before_constraints_before_activated() {
        let (prompt, _) = build_system_prompt(SystemPromptParts {
            is_root: true,
            project_bind: "项目已绑定。".into(),
            skills_catalog: "## Skills\n- writer: Write".into(),
            catalog_report: SkillRenderReport::default(),
            studio_body: "studio rules MARKER",
            novel_draft_body: "",
            injections: &[],
        });
        let skills_pos = prompt.find("## Skills").expect("skills");
        let constraints_pos = prompt.find("function calling").expect("constraints");
        let activated_pos = prompt.find("studio rules MARKER").expect("activated");
        assert!(
            skills_pos < constraints_pos,
            "skills must precede constraints:\n{prompt}"
        );
        assert!(
            constraints_pos < activated_pos,
            "constraints must precede activated skills:\n{prompt}"
        );
        assert!(prompt.contains("# Skill: studio"));
    }

    #[test]
    fn studio_catalog_budget_defaults_to_4000() {
        assert_eq!(studio_skill_catalog_budget(None), 4_000);
        // Even a huge window stays within the Studio ceiling until wiring is complete.
        assert_eq!(studio_skill_catalog_budget(Some(128_000)), 4_000);
    }

    #[test]
    fn subagent_skips_studio_orchestration() {
        let injections = [SkillInjection {
            name: "writer".into(),
            path: PathBuf::from("/tmp/writer.md"),
            body: "Write well.".into(),
        }];
        let (prompt, _) = build_system_prompt(SystemPromptParts {
            is_root: false,
            project_bind: "项目《sample-novel》。".into(),
            skills_catalog: "## Skills\n- writer: Write".into(),
            catalog_report: SkillRenderReport::default(),
            studio_body: "SHOULD NOT APPEAR",
            novel_draft_body: "ALSO NO",
            injections: &injections,
        });
        assert!(!prompt.contains("SHOULD NOT APPEAR"));
        assert!(!prompt.contains("ALSO NO"));
        assert!(!prompt.contains("小说创作编排 Agent"));
        assert!(prompt.contains("子 Agent"));
        assert!(prompt.contains("# Skill: writer"));
        assert!(prompt.contains("Write well."));
    }

    #[test]
    fn section_order_is_stable() {
        let mut ws = WorldState::new();
        ws.set(SectionId::ActivatedSkills, "A");
        ws.set(SectionId::Constraints, "C");
        ws.set(SectionId::Identity, "I");
        ws.set(SectionId::SkillsCatalog, "S");
        assert_eq!(ws.render(), "I\n\nS\n\nC\n\nA");
    }

    #[test]
    fn catalog_filter_for_subagent() {
        let all = vec![meta("writer"), meta("studio"), meta("prose-pitfalls")];
        let filtered = catalog_skills_for_session(&all, &["writer".into()], false);
        let names: Vec<_> = filtered.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"writer"));
        assert!(names.contains(&"prose-pitfalls"));
        assert!(!names.contains(&"studio"));
    }
}
