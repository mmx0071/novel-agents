//! Studio-next decision cards — model-offered options with server-side tool whitelist.

use novelx_pipeline::{
    check_plot_write_gate, resolve_setup_next_step, PlotWriteGate, SetupNextStep,
};
use novelx_protocol::UserInputOption;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// Allowed tools for `offer_decisions(kind=studio_next)`.
pub const ALLOWED_TOOLS: &[&str] = &[
    "design_master_outline",
    "design_arc_outline",
    "upsert_setting",
    "supplement_setting",
    "design_entity",
    "delete_entity",
    "audit_setting",
    "list_entities",
    "list_plots",
    "get_project_status",
    "design_plot",
    "update_plot",
    "continue_writing",
    "revise_chapter",
    "revise_outline",
    "audit_chapter",
    "audit_chapters",
    "audit_volume",
    "review_expected_events",
    "resolve_expected_event",
    "confirm_setup",
    "sync_volume",
    "confirm_volume_memory",
    "list_version_nodes",
    "restore_version_node",
];

pub const ALLOWED_RESOLVES: &[&str] = &[
    "dismiss_gate",
    "continue_studio",
    "activate_plot_write",
    "skip_volume",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioNextOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve: Option<String>,
}

impl StudioNextOption {
    pub fn to_ui_option(&self) -> UserInputOption {
        UserInputOption {
            id: self.id.clone(),
            label: self.label.clone(),
        }
    }
}

/// Why Studio was nudged to offer a situational next-step card.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StudioNextContext {
    Mutation {
        #[serde(default)]
        apply_tool: String,
    },
    SettingBlocker {
        #[serde(default)]
        chapter: u32,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        resume_chapter_next: bool,
        #[serde(default)]
        published: bool,
        #[serde(default)]
        plot_accept_open: bool,
        #[serde(default)]
        content_blocked: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revise_instructions: Option<String>,
        #[serde(default)]
        offer_volume_handoff_after: bool,
    },
    PlotWrite {
        kind: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        volume: u32,
    },
    DraftExists {
        chapter: u32,
        suggest_chapter: u32,
    },
    ExpectedEvent {
        kind: String,
        #[serde(default)]
        chapter: u32,
        #[serde(default)]
        event_id: String,
        #[serde(default)]
        event_text: String,
    },
    VolumeAudit {
        #[serde(default)]
        volume: u32,
        #[serde(default)]
        chapters: Vec<u32>,
    },
    Generic,
}

/// Nudge in flight — RegularTask may drain; not a human gate yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwaitingStudioNext {
    pub project: String,
    pub reason: String,
    #[serde(default)]
    pub context: StudioNextContext,
}

impl Default for StudioNextContext {
    fn default() -> Self {
        Self::Generic
    }
}

/// Open situational card from model (or soft fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingStudioNext {
    pub project: String,
    pub prompt: String,
    pub options: Vec<StudioNextOption>,
    #[serde(default)]
    pub context: StudioNextContext,
}

pub fn tool_allowed(name: &str) -> bool {
    ALLOWED_TOOLS.contains(&name)
}

pub fn resolve_allowed(name: &str) -> bool {
    ALLOWED_RESOLVES.contains(&name)
}

/// Validate model-offered studio_next options. Injects `project` into tool args when missing.
pub fn validate_studio_next_options(
    project: &str,
    raw: &[Value],
) -> Result<Vec<StudioNextOption>, String> {
    if raw.is_empty() {
        return Err("options 不能为空".into());
    }
    if raw.len() > 6 {
        return Err("studio_next 最多 6 个选项".into());
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, raw_opt) in raw.iter().enumerate() {
        let label = raw_opt
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("options[{i}] 缺少 label"))?
            .to_string();
        let id = raw_opt
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("sn_{}", i + 1));
        if !seen.insert(id.clone()) {
            return Err(format!("重复 option id: {id}"));
        }
        let tool = raw_opt
            .get("tool")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let resolve = raw_opt
            .get("resolve")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        match (&tool, &resolve) {
            (Some(t), None) => {
                if !tool_allowed(t) {
                    return Err(format!("不允许的 tool: {t}"));
                }
                let mut args = raw_opt
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if let Some(obj) = args.as_object_mut() {
                    if !obj.contains_key("project") && !project.is_empty() {
                        obj.insert("project".into(), json!(project));
                    }
                }
                out.push(StudioNextOption {
                    id,
                    label,
                    tool: Some(t.clone()),
                    args: Some(args),
                    resolve: None,
                });
            }
            (None, Some(r)) => {
                if !resolve_allowed(r) {
                    return Err(format!("不允许的 resolve: {r}"));
                }
                out.push(StudioNextOption {
                    id,
                    label,
                    tool: None,
                    args: None,
                    resolve: Some(r.clone()),
                });
            }
            (Some(_), Some(_)) => {
                return Err(format!("options[{i}] 不能同时有 tool 与 resolve"));
            }
            (None, None) => {
                return Err(format!("options[{i}] 须含 tool 或 resolve"));
            }
        }
    }
    Ok(out)
}

pub fn studio_next_nudge(project: &str, reason: &str) -> String {
    format!(
        "[系统] 项目「{project}」需要你收尾本轮。原因：{reason}\n\
         1) 先写用户可见正文：本轮小结（2–4 句）+ 建议下一步；\n\
         2) 立即调用 offer_decisions(kind=\"studio_next\", project=\"{project}\", \
         prompt=含「本轮小结+推荐处理」的短 Markdown, options=[...])。\n\
         每项 options 须含 id、label，以及 tool+args 或 resolve（dismiss_gate|continue_studio）。\n\
         根据未完成计划出 2–5 个可区分可执行选项（如确认定稿/补实体/设计剧情卡）；\
         禁止在正文伪造编号卡；禁止把「继续推进」当作唯一实义项；\
         除非用户明确要写章，否则不要把 continue_writing 当作唯一选项。"
    )
}

fn opt_tool(id: &str, label: impl Into<String>, tool: &str, args: Value) -> StudioNextOption {
    StudioNextOption {
        id: id.into(),
        label: label.into(),
        tool: Some(tool.into()),
        args: Some(args),
        resolve: None,
    }
}

fn opt_resolve(id: &str, label: impl Into<String>, resolve: &str) -> StudioNextOption {
    StudioNextOption {
        id: id.into(),
        label: label.into(),
        tool: None,
        args: None,
        resolve: Some(resolve.into()),
    }
}

/// Disk-aware fallback after structure mutations when the model skips `offer_decisions`.
pub fn mutation_fallback_options(
    project: &str,
    project_dir: &Path,
    apply_tool: &str,
) -> (String, Vec<StudioNextOption>) {
    let tool = if apply_tool.is_empty() {
        "设定/大纲"
    } else {
        apply_tool
    };
    let mut options: Vec<StudioNextOption> = Vec::new();
    let step_hint = match resolve_setup_next_step(project_dir) {
        Some(SetupNextStep::NeedBrief) => {
            options.push(opt_tool(
                "fb_status",
                "查看项目状态",
                "get_project_status",
                json!({ "project": project }),
            ));
            "灵感尚未锁定，请先在对话中发送卖点/灵感（lock_brief）。".to_string()
        }
        Some(SetupNextStep::NeedMaster) => {
            options.push(opt_tool(
                "fb_master",
                "生成总纲",
                "design_master_outline",
                json!({ "project": project }),
            ));
            "下一步请生成总纲。".to_string()
        }
        Some(SetupNextStep::NeedArc) => {
            options.push(opt_tool(
                "fb_arc",
                "生成卷纲",
                "design_arc_outline",
                json!({ "project": project }),
            ));
            "下一步请生成卷纲。".to_string()
        }
        Some(SetupNextStep::NeedBible) => {
            options.push(opt_tool(
                "fb_bible",
                "生成世界观",
                "upsert_setting",
                json!({
                    "project": project,
                    "topic": "完整世界观",
                    "content": "根据 brief、总纲与卷纲生成完整世界观，须含 # 世界观 与 ## 0./1./2./7. 必填节。"
                }),
            ));
            "下一步请补齐世界观 Bible（须含 ## 0./1./2./7.）。".to_string()
        }
        Some(SetupNextStep::Confirm) => {
            options.push(opt_tool(
                "fb_confirm",
                "确认定稿",
                "confirm_setup",
                json!({ "project": project, "action": "approve" }),
            ));
            options.push(opt_tool(
                "fb_revise_setup",
                "修改再生成",
                "confirm_setup",
                json!({ "project": project, "action": "revise" }),
            ));
            "总纲/卷纲/世界观已就绪，建议确认定稿后再写章。".to_string()
        }
        None => {
            // setup ready — plot / writing guidance
            match check_plot_write_gate(project_dir) {
                PlotWriteGate::Allow { .. } => {
                    options.push(opt_tool(
                        "fb_continue",
                        "继续创作",
                        "continue_writing",
                        json!({ "project": project }),
                    ));
                    options.push(opt_tool(
                        "fb_plots",
                        "查看剧情进度",
                        "list_plots",
                        json!({ "project": project }),
                    ));
                    "定稿已完成且有进行中剧情卡，可继续创作或先查看进度。".to_string()
                }
                PlotWriteGate::Block {
                    reason: "planned_inactive",
                    plot_title,
                    ..
                } => {
                    let title = plot_title.unwrap_or_else(|| "下一段".into());
                    options.push(opt_tool(
                        "fb_activate",
                        format!("激活剧情卡「{title}」"),
                        "update_plot",
                        json!({
                            "project": project,
                            "title": title,
                            "status": "in_progress",
                            "set_active_main": true,
                        }),
                    ));
                    options.push(opt_tool(
                        "fb_plots",
                        "查看剧情进度",
                        "list_plots",
                        json!({ "project": project }),
                    ));
                    format!("已有规划中剧情卡「{title}」，建议先激活再写章。")
                }
                PlotWriteGate::Block {
                    reason: "need_design_plot",
                    ..
                }
                | PlotWriteGate::Block { .. } => {
                    options.push(opt_tool(
                        "fb_design_plot",
                        "设计并激活剧情卡",
                        "design_plot",
                        json!({
                            "project": project,
                            "title": "开卷第一段",
                            "act": 1,
                            "activate": true,
                        }),
                    ));
                    options.push(opt_tool(
                        "fb_plots",
                        "查看剧情进度",
                        "list_plots",
                        json!({ "project": project }),
                    ));
                    "定稿已完成，建议先设计并激活剧情卡再写章。".to_string()
                }
            }
        }
    };

    if !options
        .iter()
        .any(|o| o.tool.as_deref() == Some("get_project_status"))
    {
        options.push(opt_tool(
            "fb_status",
            "查看项目状态",
            "get_project_status",
            json!({ "project": project }),
        ));
    }
    options.push(opt_resolve("fb_later", "稍后", "dismiss_gate"));

    let prompt = format!("「{tool}」已落盘。{step_hint}\n\n## 本轮小结\n- 结构变更已写入项目盘\n- 推荐：按下方选项推进（模型未出情境卡，已按盘状态兜底）\n\n请选择：");
    (prompt, options)
}

/// Soft fallback when Studio fails to offer a card — write-blocking contexts get
/// actionable tools/resolves, not only「继续推进」.
pub fn contextual_fallback_options(
    project: &str,
    context: &StudioNextContext,
) -> (String, Vec<StudioNextOption>) {
    match context {
        StudioNextContext::Mutation { apply_tool } => {
            // Prefer [`mutation_fallback_options`] when a project_dir is available
            // (see maybe_offer_studio_next_fallback). Path-less call keeps a weak pair.
            let _ = apply_tool;
            (
                "需要选择下一步（模型未出卡）：".into(),
                vec![
                    opt_resolve("snf_continue", "继续推进", "continue_studio"),
                    opt_resolve("snf_later", "稍后", "dismiss_gate"),
                ],
            )
        }
        StudioNextContext::PlotWrite {
            kind,
            title,
            volume,
        } if kind == "need_design_plot" => {
            let title = if title.is_empty() {
                "下一段".to_string()
            } else {
                title.clone()
            };
            (
                "写章需要进行中的剧情卡（模型未出卡）。请选择：".into(),
                vec![
                    opt_tool(
                        "fb_design_plot",
                        "设计并激活剧情卡",
                        "design_plot",
                        json!({
                            "project": project,
                            "title": title,
                            "act": (*volume).max(1),
                            "activate": true,
                        }),
                    ),
                    opt_resolve("fb_later", "暂不写作", "dismiss_gate"),
                ],
            )
        }
        StudioNextContext::PlotWrite { kind, title, .. } if kind == "missing_exit" => {
            let label = if title.is_empty() {
                "请 Agent 补全收束条件".into()
            } else {
                format!("请 Agent 补全「{title}」收束条件")
            };
            (
                "进行中剧情卡缺少可检验的收束条件（模型未出卡）。请选择：".into(),
                vec![
                    opt_resolve("fb_complete_exit", &label, "continue_studio"),
                    opt_tool(
                        "fb_list_plots",
                        "查看剧情卡",
                        "list_plots",
                        json!({ "project": project }),
                    ),
                    opt_resolve("fb_later", "暂不写作", "dismiss_gate"),
                ],
            )
        }
        StudioNextContext::PlotWrite { title, .. } => {
            let label = if title.is_empty() {
                "激活剧情卡并写章".into()
            } else {
                format!("激活「{title}」并写章")
            };
            (
                "剧情卡尚未激活（模型未出卡）。请选择：".into(),
                vec![
                    opt_resolve("fb_activate", &label, "activate_plot_write"),
                    opt_resolve("fb_later", "暂不写作", "dismiss_gate"),
                ],
            )
        }
        StudioNextContext::DraftExists {
            chapter,
            suggest_chapter,
        } => (
            format!(
                "第{chapter}章已有未发布正文（可能为中断的连写）。请选择："
            ),
            vec![
                opt_tool(
                    "fb_batch",
                    "审校本章并继续连写",
                    "continue_writing_batch",
                    json!({
                        "project": project,
                        "confirm_skip": true,
                        "confirm_skip_expected": true,
                    }),
                ),
                opt_tool(
                    "fb_audit",
                    "仅审校本章",
                    "audit_chapter",
                    json!({
                        "project": project,
                        "chapter": chapter,
                        "confirm_skip": true,
                    }),
                ),
                opt_tool(
                    "fb_revise",
                    "修订本章",
                    "revise_chapter",
                    json!({
                        "project": project,
                        "chapter": chapter,
                        "confirm_skip": true,
                        "instructions": "在现有正文基础上修订本章：保持情节与设定一致，修补明显问题，便于审校发布。",
                    }),
                ),
                opt_tool(
                    "fb_next",
                    &format!("跳过草稿写第{suggest_chapter}章"),
                    "continue_writing",
                    json!({
                        "project": project,
                        "chapter": suggest_chapter,
                        "confirm_skip": true,
                    }),
                ),
                opt_resolve("fb_later", "稍后", "dismiss_gate"),
            ],
        ),
        StudioNextContext::ExpectedEvent {
            kind,
            chapter,
            event_id,
            ..
        } if kind == "need_review" || event_id.is_empty() => (
            format!("第{chapter}章写前有预处理预期待检阅（模型未出卡）。请选择："),
            vec![
                opt_tool(
                    "fb_review",
                    "检阅预期",
                    "review_expected_events",
                    json!({ "project": project, "chapter": chapter }),
                ),
                opt_tool(
                    "fb_skip_write",
                    "跳过检阅继续写",
                    "continue_writing",
                    json!({
                        "project": project,
                        "chapter": chapter,
                        "confirm_skip_expected": true,
                        "confirm_skip": true,
                    }),
                ),
                opt_resolve("fb_later", "稍后", "dismiss_gate"),
            ],
        ),
        StudioNextContext::ExpectedEvent {
            event_id,
            event_text,
            ..
        } => {
            let short: String = event_text.chars().take(24).collect();
            let approve_label = if short.is_empty() {
                "纳入本次创作".into()
            } else {
                format!("纳入「{short}」")
            };
            (
                "预处理预期可纳入本次创作（模型未出卡）。请选择：".into(),
                vec![
                    opt_tool(
                        "fb_approve",
                        &approve_label,
                        "resolve_expected_event",
                        json!({
                            "project": project,
                            "id": event_id,
                            "status": "approved",
                            "apply": true,
                            "confirm_skip": true,
                        }),
                    ),
                    opt_tool(
                        "fb_skip",
                        "本次跳过",
                        "resolve_expected_event",
                        json!({
                            "project": project,
                            "id": event_id,
                            "action": "skip_once",
                            "confirm_skip": true,
                        }),
                    ),
                    opt_resolve("fb_later", "稍后处理", "dismiss_gate"),
                ],
            )
        }
        StudioNextContext::VolumeAudit { chapters, volume } => {
            let list = chapters
                .iter()
                .map(|c| format!("第{c}章"))
                .collect::<Vec<_>>()
                .join("、");
            (
                format!("第{volume}卷复盘建议深审：{list}（模型未出卡）。请选择："),
                vec![
                    opt_tool(
                        "fb_deep",
                        "按建议深审",
                        "audit_chapters",
                        json!({
                            "project": project,
                            "action": "start",
                            "chapters": chapters,
                        }),
                    ),
                    opt_resolve("fb_dismiss", "结束复盘", "skip_volume"),
                ],
            )
        }
        StudioNextContext::SettingBlocker { .. } => (
            "设定审计出现 BLOCKER（模型未出卡）。请选择：".into(),
            vec![
                opt_tool(
                    "fb_entities",
                    "列出设定卡",
                    "list_entities",
                    json!({ "project": project }),
                ),
                opt_tool(
                    "fb_audit",
                    "再跑设定审计",
                    "audit_setting",
                    json!({ "project": project }),
                ),
                opt_resolve("fb_accept", "接受并继续", "dismiss_gate"),
            ],
        ),
        StudioNextContext::Generic => (
            "需要选择下一步（模型未出卡）：".into(),
            vec![
                opt_resolve("snf_continue", "继续推进", "continue_studio"),
                opt_resolve("snf_later", "稍后", "dismiss_gate"),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novelx_pipeline::{init_project, lock_brief, maybe_advance_setup_after_outlines};
    use std::fs;

    fn tmp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "novelx-studio-next-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn accepts_whitelisted_tools() {
        let raw = vec![
            json!({"id":"a","label":"重建总纲","tool":"design_master_outline"}),
            json!({"id":"b","label":"稍后","resolve":"dismiss_gate"}),
        ];
        let opts = validate_studio_next_options("demo", &raw).unwrap();
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].args.as_ref().unwrap()["project"], "demo");
    }

    #[test]
    fn rejects_unknown_tool() {
        let raw = vec![json!({"label":"坏","tool":"rm_rf"})];
        assert!(validate_studio_next_options("demo", &raw).is_err());
    }

    #[test]
    fn plot_fallback_is_actionable() {
        let (_, opts) = contextual_fallback_options(
            "demo",
            &StudioNextContext::PlotWrite {
                kind: "planned_inactive".into(),
                title: "样例卡".into(),
                volume: 1,
            },
        );
        assert!(opts.iter().any(|o| o.resolve.as_deref() == Some("activate_plot_write")));
    }

    #[test]
    fn missing_exit_fallback_offers_complete_and_dismiss() {
        let (prompt, opts) = contextual_fallback_options(
            "demo",
            &StudioNextContext::PlotWrite {
                kind: "missing_exit".into(),
                title: "样例卡".into(),
                volume: 1,
            },
        );
        assert!(prompt.contains("收束条件"), "{prompt}");
        assert!(
            opts.iter().any(|o| o.resolve.as_deref() == Some("continue_studio")),
            "{opts:?}"
        );
        assert!(
            opts.iter().any(|o| o.tool.as_deref() == Some("list_plots")),
            "{opts:?}"
        );
        assert!(
            opts.iter().any(|o| o.resolve.as_deref() == Some("dismiss_gate")),
            "{opts:?}"
        );
        assert!(
            !opts
                .iter()
                .any(|o| o.resolve.as_deref() == Some("activate_plot_write")),
            "missing_exit must not offer activate: {opts:?}"
        );
    }

    #[test]
    fn draft_fallback_offers_batch_resume_first() {
        let (_, opts) = contextual_fallback_options(
            "demo",
            &StudioNextContext::DraftExists {
                chapter: 3,
                suggest_chapter: 4,
            },
        );
        assert_eq!(
            opts[0].tool.as_deref(),
            Some("continue_writing_batch"),
            "{opts:?}"
        );
        assert!(opts.iter().any(|o| o.tool.as_deref() == Some("continue_writing")));
    }

    #[test]
    fn mutation_fallback_confirm_offers_confirm_setup() {
        let root = tmp_root("mut-confirm");
        let dir = init_project(&root, "sample-novel", "sample-genre", 100).unwrap();
        lock_brief(&dir, "一位主角决心揭开旧案。").unwrap();
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            format!("# 总纲\n\n{}", "x".repeat(40)),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            format!("# 卷纲\n\n{}", "y".repeat(40)),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/bible.md"),
            r#"# 世界观 Bible

## 0. 一句话世界
世界。

## 1. 时代与叙事框架
时代。

## 2. 全局势力与阵营
势力。

## 7. 开放问题
待揭。
"#,
        )
        .unwrap();
        let _ = maybe_advance_setup_after_outlines(&dir);

        let (prompt, opts) =
            mutation_fallback_options("sample-novel", &dir, "upsert_setting");
        assert!(prompt.contains("本轮小结"), "{prompt}");
        assert!(
            opts.iter().any(|o| {
                o.tool.as_deref() == Some("confirm_setup")
                    && o.args
                        .as_ref()
                        .and_then(|a| a.get("action"))
                        .and_then(|v| v.as_str())
                        == Some("approve")
            }),
            "expected confirm_setup approve: {opts:?}"
        );
        assert!(
            !opts
                .iter()
                .any(|o| o.resolve.as_deref() == Some("continue_studio") && opts.len() <= 2),
            "must not be only continue/later: {opts:?}"
        );
        assert!(
            opts.iter()
                .any(|o| o.tool.as_deref() == Some("get_project_status")),
            "{opts:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mutation_fallback_need_bible_offers_upsert() {
        let root = tmp_root("mut-bible");
        let dir = init_project(&root, "sample-novel", "sample-genre", 100).unwrap();
        lock_brief(&dir, "一位主角决心揭开旧案。").unwrap();
        fs::create_dir_all(dir.join("artifacts")).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            format!("# 总纲\n\n{}", "x".repeat(40)),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            format!("# 卷纲\n\n{}", "y".repeat(40)),
        )
        .unwrap();
        fs::write(dir.join("artifacts/bible.md"), "# 世界观\n\n只有标题。\n").unwrap();
        let _ = maybe_advance_setup_after_outlines(&dir);

        let (_, opts) = mutation_fallback_options("sample-novel", &dir, "design_arc_outline");
        assert!(
            opts.iter().any(|o| o.tool.as_deref() == Some("upsert_setting")),
            "{opts:?}"
        );
        assert!(
            !opts.iter().any(|o| o.tool.as_deref() == Some("confirm_setup")),
            "incomplete bible must not offer confirm: {opts:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn studio_next_nudge_requires_summary() {
        let n = studio_next_nudge("sample-novel", "upsert_setting 已落盘");
        assert!(n.contains("本轮小结"));
        assert!(n.contains("offer_decisions"));
        assert!(n.contains("继续推进"));
    }
}
