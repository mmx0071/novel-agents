//! Studio-next decision cards — model-offered options with server-side tool whitelist.

use novelx_protocol::UserInputOption;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
        "[系统] 项目「{project}」需要你给出下一步审批卡。原因：{reason}\n\
         请立即调用 offer_decisions(kind=\"studio_next\", project=\"{project}\", prompt=短提示, options=[...])。\n\
         每项 options 须含 id、label，以及 tool+args 或 resolve（dismiss_gate|continue_studio）。\n\
         根据对话未完成计划出 2–5 个可区分选项；禁止在正文伪造编号卡；\
         除非用户明确要写章，否则不要把 continue_writing 当作唯一选项。"
    )
}

fn opt_tool(id: &str, label: &str, tool: &str, args: Value) -> StudioNextOption {
    StudioNextOption {
        id: id.into(),
        label: label.into(),
        tool: Some(tool.into()),
        args: Some(args),
        resolve: None,
    }
}

fn opt_resolve(id: &str, label: &str, resolve: &str) -> StudioNextOption {
    StudioNextOption {
        id: id.into(),
        label: label.into(),
        tool: None,
        args: None,
        resolve: Some(resolve.into()),
    }
}

/// Soft fallback when Studio fails to offer a card — write-blocking contexts get
/// actionable tools/resolves, not only「继续推进」.
pub fn contextual_fallback_options(
    project: &str,
    context: &StudioNextContext,
) -> (String, Vec<StudioNextOption>) {
    match context {
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
            format!("第{chapter}章已有未发布正文（模型未出卡）。请选择："),
            vec![
                opt_tool(
                    "fb_audit",
                    "审校本章",
                    "audit_chapter",
                    json!({ "project": project, "chapter": chapter }),
                ),
                opt_tool(
                    "fb_revise",
                    "修订本章",
                    "revise_chapter",
                    json!({
                        "project": project,
                        "chapter": chapter,
                        "instructions": "在现有正文基础上修订本章：保持情节与设定一致，修补明显问题，便于审校发布。",
                    }),
                ),
                opt_tool(
                    "fb_next",
                    &format!("写第{suggest_chapter}章"),
                    "continue_writing",
                    json!({ "project": project, "chapter": suggest_chapter }),
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
        StudioNextContext::Mutation { .. } | StudioNextContext::Generic => (
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
    fn draft_fallback_offers_write_next() {
        let (_, opts) = contextual_fallback_options(
            "demo",
            &StudioNextContext::DraftExists {
                chapter: 3,
                suggest_chapter: 4,
            },
        );
        assert!(opts.iter().any(|o| o.tool.as_deref() == Some("continue_writing")));
    }
}
