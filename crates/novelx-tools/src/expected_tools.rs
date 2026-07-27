//! Tools for preprocess expected events.

use crate::mutation::maybe_preview;
use crate::{ToolContext, ToolHandler, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use novelx_pipeline::{
    agent_fit_actionable, build_eval_context, conditions_from_value, count_by_status,
    enqueue_expected_event, eval_hard_ok, format_conditions_line, format_events_summary,
    list_events_for_preview, list_gate_candidates, list_hard_ok_candidates, load_expected_events,
    load_project_state, project_dir, resolve_expected_event, set_event_review, update_expected_event,
    ExpectedReview,
};
use serde_json::{json, Value};

fn load_reviewer_skill(ctx: &ToolContext) -> String {
    use novelx_skills::{build_skill_injections, load_skills, SkillScope};
    let outcome = load_skills(&[
        (SkillScope::Studio, ctx.config_root.join("skills")),
        (SkillScope::Agent, ctx.config_root.join("skills/agents")),
    ]);
    let inj = build_skill_injections(&outcome.skills, &["expectation-reviewer".into()]);
    inj.into_iter()
        .next()
        .map(|i| i.body)
        .unwrap_or_else(|| {
            "你是预期检阅员。对照项目进度评估预处理预期能否纳入当前创作。只输出 JSON。".into()
        })
}

fn extract_json_object(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&t[start..=end]).ok()
}

pub struct EnqueueExpectedEvent;
pub struct UpdateExpectedEvent;
pub struct ListExpectedEvents;
pub struct ReviewExpectedEvents;
pub struct ResolveExpectedEvent;

#[async_trait]
impl ToolHandler for EnqueueExpectedEvent {
    fn name(&self) -> &'static str {
        "enqueue_expected_event"
    }
    fn description(&self) -> &'static str {
        "登记预处理预期事件（以后再处理的加角色/退场/复活/剧情等）；结合项目结构写触发条件，须用户确认后落盘"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "text":{"type":"string","description":"预期描述（必填）"},
                "kind":{"type":"string","description":"add_character|exit_character|revive_character|plot|setting|other"},
                "source":{"type":"string","description":"author|reader"},
                "entity_ref":{"type":"string"},
                "notes":{"type":"string"},
                "conditions":{
                    "type":"object",
                    "properties":{
                        "min_chapter":{"type":"integer"},
                        "max_chapter":{"type":"integer"},
                        "volume":{"type":"integer"},
                        "require_plot_id":{"type":"string"},
                        "require_plot_status":{"type":"string"},
                        "require_entity":{"type":"string"},
                        "require_entity_status":{"type":"string"},
                        "after_event_id":{"type":"string"},
                        "freeform":{"type":"string"}
                    }
                },
                "apply":{"type":"boolean"},
                "mutation_id":{"type":"string"},
                "confirm_skip":{"type":"boolean"}
            },
            "required":["project","text"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let text = args["text"].as_str().unwrap_or("").trim();
        if project.is_empty() || text.is_empty() {
            anyhow::bail!("project 与 text 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let created = load_project_state(&dir)
            .map(|s| s.next_chapter.max(1))
            .unwrap_or(1);
        let kind = args["kind"].as_str().unwrap_or("other");
        let source = args["source"].as_str().unwrap_or("author");
        let entity_ref = args["entity_ref"].as_str().unwrap_or("");
        let notes = args["notes"].as_str().unwrap_or("");
        let conditions = conditions_from_value(args.get("conditions"));
        let cond_line = format_conditions_line(&conditions);
        if let Some(prev) = maybe_preview(
            &ctx.config_root,
            &args,
            "enqueue_expected_event",
            &format!("将登记预期：{}", text.chars().take(80).collect::<String>()),
            json!({
                "kind": kind,
                "text": text,
                "source": source,
                "entity_ref": entity_ref,
                "notes": notes,
                "conditions": conditions,
                "conditions_line": cond_line,
            }),
            "enqueue_expected_event",
            args.clone(),
        ) {
            return Ok(prev);
        }
        let ev = enqueue_expected_event(
            &dir, kind, text, source, entity_ref, notes, conditions, created,
        )?;
        Ok(ToolResult {
            output: format!(
                "已登记预期 `{}` [{}]：{}（条件：{}）",
                ev.id,
                ev.kind,
                ev.text,
                format_conditions_line(&ev.conditions)
            ),
            data: json!({
                "event": ev,
                "project": project,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for UpdateExpectedEvent {
    fn name(&self) -> &'static str {
        "update_expected_event"
    }
    fn description(&self) -> &'static str {
        "更新预处理预期事件的描述/条件/备注（未结束状态）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "id":{"type":"string"},
                "text":{"type":"string"},
                "kind":{"type":"string"},
                "entity_ref":{"type":"string"},
                "notes":{"type":"string"},
                "conditions":{"type":"object"},
                "apply":{"type":"boolean"},
                "mutation_id":{"type":"string"},
                "confirm_skip":{"type":"boolean"}
            },
            "required":["project","id"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let id = args["id"].as_str().unwrap_or("");
        if project.is_empty() || id.is_empty() {
            anyhow::bail!("project 与 id 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let conditions = if args.get("conditions").is_some() {
            Some(conditions_from_value(args.get("conditions")))
        } else {
            None
        };
        if let Some(prev) = maybe_preview(
            &ctx.config_root,
            &args,
            "update_expected_event",
            &format!("将更新预期 {id}"),
            json!({
                "id": id,
                "text": args.get("text"),
                "kind": args.get("kind"),
                "notes": args.get("notes"),
                "entity_ref": args.get("entity_ref"),
                "conditions": conditions,
            }),
            "update_expected_event",
            args.clone(),
        ) {
            return Ok(prev);
        }
        let ev = update_expected_event(
            &dir,
            id,
            args["text"].as_str(),
            args["kind"].as_str(),
            args["notes"].as_str(),
            args["entity_ref"].as_str(),
            conditions,
        )?;
        Ok(ToolResult {
            output: format!("已更新预期 `{}`：{}", ev.id, ev.text),
            data: json!({"event": ev, "project": project}),
        })
    }
}

#[async_trait]
impl ToolHandler for ListExpectedEvents {
    fn name(&self) -> &'static str {
        "list_expected_events"
    }
    fn description(&self) -> &'static str {
        "列出预处理预期事件（可按 status 过滤；含实时 eligibility）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "status":{"type":"string","description":"waiting|eligible|approved|incorporated|dismissed"},
                "chapter":{"type":"integer","description":"求值章号，默认 next_chapter"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let chapter = args["chapter"]
            .as_u64()
            .map(|n| n as u32)
            .unwrap_or_else(|| {
                load_project_state(&dir)
                    .map(|s| s.next_chapter.max(1))
                    .unwrap_or(1)
            });
        let status = args["status"].as_str();
        let output = format_events_summary(&dir, chapter, status);
        let events = list_events_for_preview(&dir, chapter);
        let (pending, approved, due) = count_by_status(&dir);
        Ok(ToolResult {
            output,
            data: json!({
                "events": events,
                "chapter": chapter,
                "deferred_pending": pending,
                "deferred_approved": approved,
                "deferred_due": due,
                "project": project,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for ReviewExpectedEvents {
    fn name(&self) -> &'static str {
        "review_expected_events"
    }
    fn description(&self) -> &'static str {
        "检阅预处理预期：先求硬条件，必要时跑 expectation_reviewer；有可纳入候选时请用户决策（纳入/跳过/稍后）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "scope":{"type":"string","description":"chapter|volume（提示检阅范围）"},
                "skip_llm":{"type":"boolean","description":"仅硬条件筛，不调用检阅 Agent"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let chapter = args["chapter"]
            .as_u64()
            .map(|n| n as u32)
            .filter(|n| *n > 0)
            .unwrap_or_else(|| {
                load_project_state(&dir)
                    .map(|s| s.next_chapter.max(1))
                    .unwrap_or(1)
            });
        let scope = args["scope"].as_str().unwrap_or("chapter");
        let skip_llm = args["skip_llm"].as_bool().unwrap_or(false);
        let hard = list_hard_ok_candidates(&dir, chapter);
        if hard.is_empty() {
            let waiting = load_expected_events(&dir)
                .events
                .iter()
                .filter(|e| matches!(e.status.as_str(), "waiting" | "eligible"))
                .count();
            return Ok(ToolResult {
                output: if waiting == 0 {
                    "无预处理预期事件，可直接写章。".into()
                } else {
                    format!("有 {waiting} 条等待中的预期，但当前硬条件均未满足，继续等待触发。")
                },
                data: json!({
                    "project": project,
                    "chapter": chapter,
                    "scope": scope,
                    "hard_ok_count": 0,
                    "needs_user_choice": false,
                }),
            });
        }

        let eval = build_eval_context(&dir, chapter);
        let mut reviews: Vec<Value> = Vec::new();
        let now = chrono::Utc::now().to_rfc3339();

        if skip_llm {
            for ev in &hard {
                let elig = eval_hard_ok(ev, &eval);
                let review = ExpectedReview {
                    chapter,
                    volume: eval.volume,
                    hard_ok: elig.hard_ok,
                    agent_fit: "中".into(),
                    reason: "仅硬条件筛：条件已满足，建议用户确认是否纳入。".into(),
                    suggestion: "若纳入：先用 design_entity/design_plot/update_plot 等现有工具落地，再 resolve 为 incorporated。".into(),
                    reviewed_at: now.clone(),
                };
                let updated = set_event_review(&dir, &ev.id, review.clone(), true)?;
                reviews.push(json!({
                    "id": updated.id,
                    "text": updated.text,
                    "kind": updated.kind,
                    "agent_fit": review.agent_fit,
                    "reason": review.reason,
                    "suggestion": review.suggestion,
                }));
            }
        } else {
            let skill = load_reviewer_skill(ctx);
            let mut catalog = String::new();
            for ev in &hard {
                let elig = eval_hard_ok(ev, &eval);
                catalog.push_str(&format!(
                    "- id={} kind={} status={} text={} entity_ref={} conditions={} hard_reasons={:?}\n",
                    ev.id,
                    ev.kind,
                    ev.status,
                    ev.text,
                    ev.entity_ref,
                    format_conditions_line(&ev.conditions),
                    elig.reasons
                ));
            }
            let prompt = format!(
                "当前检阅范围：{scope}；目标章：第{chapter}章；卷：第{}卷。\n\
                 下列预期事件硬条件已通过，请评估每条是否适合纳入本次创作。\n\
                 只输出 JSON：{{\"reviews\":[{{\"id\":\"ee_...\",\"agent_fit\":\"高|中|低|不适用\",\"reason\":\"...\",\"suggestion\":\"...\"}}]}}\n\
                 最多选出 3 条 agent_fit 为 高 或 中 的建议用户决策项；其余可标 低。\n\n候选：\n{catalog}",
                eval.volume
            );
            let raw = ctx
                .llm
                .complete(
                    &skill,
                    &prompt,
                    Some(&ctx.llm.model_for_agent("expectation_reviewer")),
                )
                .await
                .unwrap_or_default();
            let parsed = extract_json_object(&raw);
            let arr = parsed
                .as_ref()
                .and_then(|v| v.get("reviews"))
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default();

            for ev in &hard {
                let hit = arr.iter().find(|r| {
                    r.get("id").and_then(|v| v.as_str()) == Some(ev.id.as_str())
                });
                let (fit, reason, suggestion) = if let Some(h) = hit {
                    (
                        h.get("agent_fit")
                            .and_then(|v| v.as_str())
                            .unwrap_or("中")
                            .to_string(),
                        h.get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("检阅完成")
                            .to_string(),
                        h.get("suggestion")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    )
                } else if arr.is_empty() {
                    // LLM failed — treat hard_ok as mid fit so user can still decide.
                    (
                        "中".into(),
                        "检阅模型无结构化输出，硬条件已满足，交由用户决定。".into(),
                        "确认纳入后用现有设定/剧情工具落地。".into(),
                    )
                } else {
                    (
                        "低".into(),
                        "未列入本次优先候选。".into(),
                        String::new(),
                    )
                };
                let review = ExpectedReview {
                    chapter,
                    volume: eval.volume,
                    hard_ok: true,
                    agent_fit: fit.clone(),
                    reason: reason.clone(),
                    suggestion: suggestion.clone(),
                    reviewed_at: now.clone(),
                };
                let updated = set_event_review(&dir, &ev.id, review, true)?;
                if agent_fit_actionable(&fit) {
                    reviews.push(json!({
                        "id": updated.id,
                        "text": updated.text,
                        "kind": updated.kind,
                        "agent_fit": fit,
                        "reason": reason,
                        "suggestion": suggestion,
                    }));
                }
            }
            // Cap gate candidates at 3 (prefer 高 over 中).
            reviews.sort_by(|a, b| {
                let fa = a.get("agent_fit").and_then(|v| v.as_str()).unwrap_or("");
                let fb = b.get("agent_fit").and_then(|v| v.as_str()).unwrap_or("");
                fit_rank(fa).cmp(&fit_rank(fb))
            });
            reviews.truncate(3);
        }

        let primary = reviews.first().cloned();
        let needs = primary.is_some();
        let output = if needs {
            let p = primary.as_ref().unwrap();
            format!(
                "检阅完成：{} 条硬条件通过，建议用户决策「{}」（{}）。请在审批卡选择纳入/跳过/稍后。",
                hard.len(),
                p.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                p.get("agent_fit").and_then(|v| v.as_str()).unwrap_or("")
            )
        } else {
            format!(
                "检阅完成：{} 条硬条件通过，但无「高/中」拟合项，暂不弹决策卡。",
                hard.len()
            )
        };

        Ok(ToolResult {
            output,
            data: json!({
                "project": project,
                "chapter": chapter,
                "scope": scope,
                "hard_ok_count": hard.len(),
                "candidates": reviews,
                "event_id": primary.as_ref().and_then(|p| p.get("id").cloned()),
                "event_text": primary.as_ref().and_then(|p| p.get("text").cloned()),
                "event_kind": primary.as_ref().and_then(|p| p.get("kind").cloned()),
                "needs_user_choice": needs,
                "gate": "expected_event",
            }),
        })
    }
}

fn fit_rank(fit: &str) -> u8 {
    match fit.trim() {
        "高" | "high" => 0,
        "中" | "mid" | "medium" => 1,
        _ => 2,
    }
}

#[async_trait]
impl ToolHandler for ResolveExpectedEvent {
    fn name(&self) -> &'static str {
        "resolve_expected_event"
    }
    fn description(&self) -> &'static str {
        "处理预期事件：approved=纳入本次创作；incorporated=已落地；dismissed=放弃；action=skip_once=本次跳过（不改盘）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "id":{"type":"string"},
                "status":{"type":"string","description":"approved|incorporated|dismissed|waiting|eligible"},
                "action":{"type":"string","description":"skip_once：本回合跳过，不改 status"},
                "chapter":{"type":"integer"},
                "apply":{"type":"boolean"},
                "mutation_id":{"type":"string"},
                "confirm_skip":{"type":"boolean"}
            },
            "required":["project","id"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let id = args["id"].as_str().unwrap_or("");
        if project.is_empty() || id.is_empty() {
            anyhow::bail!("project 与 id 必填");
        }
        let action = args["action"].as_str().unwrap_or("");
        if action == "skip_once" {
            return Ok(ToolResult {
                output: format!("已跳过预期 `{id}`（本回合不再提示，状态仍 waiting/eligible）。"),
                data: json!({
                    "project": project,
                    "event_id": id,
                    "skip_once": true,
                    "dismiss_expected_gate": true,
                }),
            });
        }
        let status = args["status"].as_str().unwrap_or("").trim();
        if status.is_empty() {
            anyhow::bail!("status 或 action=skip_once 必填其一");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let chapter = args["chapter"]
            .as_u64()
            .map(|n| n as u32)
            .unwrap_or_else(|| {
                load_project_state(&dir)
                    .map(|s| s.next_chapter.max(1))
                    .unwrap_or(1)
            });
        if let Some(prev) = maybe_preview(
            &ctx.config_root,
            &args,
            "resolve_expected_event",
            &format!("将把预期 {id} 标为 {status}"),
            json!({"id": id, "status": status, "chapter": chapter}),
            "resolve_expected_event",
            args.clone(),
        ) {
            return Ok(prev);
        }
        let ev = resolve_expected_event(&dir, id, status, chapter)?;
        Ok(ToolResult {
            output: format!("预期 `{}` 已标为 {}：{}", ev.id, ev.status, ev.text),
            data: json!({
                "event": ev,
                "project": project,
                "dismiss_expected_gate": true,
                "approved": status == "approved",
            }),
        })
    }
}

/// Soft-block payload for continue_writing when expected events need attention.
pub fn continue_writing_expected_block(
    project_dir: &std::path::Path,
    project: &str,
    chapter: u32,
    skipped_ids: &[String],
    skip_expected: bool,
) -> Option<ToolResult> {
    if skip_expected {
        return None;
    }
    let hard = list_hard_ok_candidates(project_dir, chapter)
        .into_iter()
        .filter(|e| !skipped_ids.iter().any(|s| s == &e.id))
        .collect::<Vec<_>>();
    if hard.is_empty() {
        return None;
    }
    let needs_review = hard.iter().any(|e| {
        e.last_review
            .as_ref()
            .map(|r| r.chapter != chapter)
            .unwrap_or(true)
    });
    if needs_review {
        return Some(ToolResult {
            output: format!(
                "⛔ 写章已拦截\n\n有 {} 条预处理预期硬条件已满足，请先检阅是否纳入本次创作。",
                hard.len()
            ),
            data: json!({
                "blocked": true,
                "reason": "need_expected_review",
                "project": project,
                "chapter": chapter,
                "hard_ok_count": hard.len(),
                "candidate_ids": hard.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
            }),
        });
    }
    let gate = list_gate_candidates(project_dir, chapter)
        .into_iter()
        .filter(|e| !skipped_ids.iter().any(|s| s == &e.id))
        .collect::<Vec<_>>();
    let Some(primary) = gate.first() else {
        return None;
    };
    let reason = primary
        .last_review
        .as_ref()
        .map(|r| r.reason.as_str())
        .unwrap_or("检阅建议纳入");
    Some(ToolResult {
        output: format!(
            "⛔ 写章已拦截\n\n预处理预期「{}」可纳入本次创作（{}）。请选择：纳入本次 / 本次跳过 / 稍后处理。",
            primary.text, reason
        ),
        data: json!({
            "blocked": true,
            "reason": "expected_event_pending",
            "project": project,
            "chapter": chapter,
            "event_id": primary.id,
            "event_text": primary.text,
            "event_kind": primary.kind,
            "needs_user_choice": true,
            "gate": "expected_event",
        }),
    })
}
