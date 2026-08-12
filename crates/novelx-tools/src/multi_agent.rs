//! Multi-agent tools (Codex spawn / send / wait / interrupt / list).

use super::{
    AgentRuntime, SendMessageRequest, SpawnAgentRequest, ToolContext, ToolHandler, ToolResult,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub struct SpawnAgent;
pub struct SendAgentMessage;
pub struct FollowupTask;
pub struct WaitAgent;
pub struct ListAgents;
pub struct InterruptAgent;

fn runtime(ctx: &ToolContext) -> Result<&Arc<dyn AgentRuntime>> {
    ctx.agent_runtime
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("agent runtime unavailable"))
}

fn parent_id(ctx: &ToolContext, args: &Value) -> Result<String> {
    if let Some(p) = args.get("parent_thread_id").and_then(|v| v.as_str()) {
        return Ok(p.to_string());
    }
    ctx.caller_thread_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("missing parent_thread_id / caller_thread_id"))
}

fn spawnable_catalog_json(config_root: &std::path::Path) -> Value {
    let entries = novelx_harness::list_spawnable_agents(config_root);
    json!(entries
        .into_iter()
        .map(|e| {
            json!({
                "id": e.id,
                "name": e.name,
                "description": e.description,
                "invocation": e.invocation.as_str(),
                "allowSpawn": true,
                "tools": e.tools,
            })
        })
        .collect::<Vec<_>>())
}

fn normalize_wait_data(thread_id: &str, summary: &str, data: &Value) -> Value {
    let artifacts = data
        .get("artifacts")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let warnings = data
        .get("warnings")
        .cloned()
        .unwrap_or_else(|| json!([]));
    json!({
        "summary": summary,
        "artifacts": artifacts,
        "warnings": warnings,
        "threadId": thread_id,
        "result": data,
    })
}

#[async_trait]
impl ToolHandler for SpawnAgent {
    fn name(&self) -> &'static str {
        "spawn_agent"
    }
    fn description(&self) -> &'static str {
        "启动单步专精 / 只读旁路 SubAgent（仅 agents.yaml 中 allow_spawn=true 的角色，\
如 literary_editor、material_researcher、setting_auditor）。\
整章续写/修订/审校请用 continue_writing / revise_chapter / audit_chapter（或 audit_chapters）。\
禁止 mode=continue|revise|audit_only。可 spawn 目录见 list_agents(filter=spawnable)。\
完成后用 wait_agent 收取 {summary, artifacts, warnings}。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "role":{"type":"string","description":"allow_spawn 角色 id，如 literary_editor / material_researcher / setting_auditor"},
                "task":{"type":"string","description":"交给子 Agent 的任务说明"},
                "project":{"type":"string"},
                "chapter":{"type":"integer","description":"可选；勿与 mode=continue|revise|audit_only 联用"},
                "mode":{"type":"string","description":"勿填 continue|revise|audit_only（整章请用专用工具）"},
                "parent_thread_id":{"type":"string"}
            },
            "required":["role","task"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let rt = runtime(ctx)?;
        let role = args["role"].as_str().unwrap_or("").to_string();
        let task = args["task"].as_str().unwrap_or("").to_string();
        if role.is_empty() || task.is_empty() {
            anyhow::bail!("role and task are required");
        }
        let chapter = args["chapter"].as_u64().map(|n| n as u32);
        let mode = args["mode"].as_str();
        if chapter.is_some()
            && matches!(mode, Some("continue") | Some("revise") | Some("audit_only"))
        {
            anyhow::bail!(
                "整章续写/修订/审校请用 continue_writing / revise_chapter / audit_chapter（或 audit_chapters），\
勿 spawn_agent(mode=continue|revise|audit_only)；SubAgent 仅用于单步专精或只读旁路"
            );
        }
        if !novelx_harness::is_spawn_allowed(&ctx.config_root, &role) {
            anyhow::bail!(
                "role '{}' 不允许 spawn。整章请用 continue_writing / revise_chapter / audit_*；\
领域操作用对应 design_* / audit_* / research_materials。可 spawn 目录：{}",
                role,
                spawnable_catalog_json(&ctx.config_root)
            );
        }
        let resp = rt
            .spawn_agent(SpawnAgentRequest {
                parent_thread_id: parent_id(ctx, &args)?,
                role: role.clone(),
                task: task.clone(),
                project: args["project"].as_str().map(|s| s.to_string()),
                chapter,
                mode: mode.map(|s| s.to_string()),
                revision: None,
            })
            .await?;
        let meta = novelx_harness::lookup_agent(&ctx.config_root, &role);
        Ok(ToolResult {
            output: format!(
                "spawned {} as {} ({})",
                role,
                resp.agent_path.as_str(),
                resp.thread_id
            ),
            data: json!({
                "threadId": resp.thread_id,
                "agentPath": resp.agent_path.as_str(),
                "role": role,
                "invocation": meta.as_ref().map(|m| m.invocation.as_str()),
                "allowSpawn": true,
                "tools": meta.map(|m| m.tools).unwrap_or_default(),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for SendAgentMessage {
    fn name(&self) -> &'static str {
        "send_message"
    }
    fn description(&self) -> &'static str {
        "向子 Agent 邮箱投递消息（默认不立刻 trigger turn）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "recipient_thread_id":{"type":"string"},
                "content":{"type":"string"},
                "trigger_turn":{"type":"boolean"},
                "parent_thread_id":{"type":"string"}
            },
            "required":["recipient_thread_id","content"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let rt = runtime(ctx)?;
        let recipient = args["recipient_thread_id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let content = args["content"].as_str().unwrap_or("").to_string();
        rt.send_message(SendMessageRequest {
            author_thread_id: parent_id(ctx, &args)?,
            recipient_thread_id: recipient.clone(),
            content: content.clone(),
            trigger_turn: args["trigger_turn"].as_bool().unwrap_or(false),
        })
        .await?;
        Ok(ToolResult {
            output: format!("queued message → {recipient}"),
            data: json!({"recipient": recipient, "content": content}),
        })
    }
}

#[async_trait]
impl ToolHandler for FollowupTask {
    fn name(&self) -> &'static str {
        "followup_task"
    }
    fn description(&self) -> &'static str {
        "向子 Agent 追加任务并 trigger_turn=true"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "recipient_thread_id":{"type":"string"},
                "content":{"type":"string"},
                "parent_thread_id":{"type":"string"}
            },
            "required":["recipient_thread_id","content"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let rt = runtime(ctx)?;
        let recipient = args["recipient_thread_id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let content = args["content"].as_str().unwrap_or("").to_string();
        rt.send_message(SendMessageRequest {
            author_thread_id: parent_id(ctx, &args)?,
            recipient_thread_id: recipient.clone(),
            content: content.clone(),
            trigger_turn: true,
        })
        .await?;
        Ok(ToolResult {
            output: format!("followup triggered → {recipient}"),
            data: json!({"recipient": recipient}),
        })
    }
}

#[async_trait]
impl ToolHandler for WaitAgent {
    fn name(&self) -> &'static str {
        "wait_agent"
    }
    fn description(&self) -> &'static str {
        "等待 spawn_agent 启动的单步专精 / 只读旁路子 Agent 完成。\
返回契约：{summary, artifacts[], warnings[]}（另含 threadId / result 原始数据）。\
勿与整章 continue_writing / revise / audit 混用（整章走进程内流水线，无需 wait_agent）。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "thread_id":{"type":"string"},
                "timeout_secs":{"type":"integer","description":"默认 600"}
            },
            "required":["thread_id"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let rt = runtime(ctx)?;
        let tid = args["thread_id"].as_str().unwrap_or("").to_string();
        let timeout = Duration::from_secs(args["timeout_secs"].as_u64().unwrap_or(600));
        let resp = rt.wait_agent(&tid, timeout).await?;
        let data = normalize_wait_data(&resp.thread_id, &resp.summary, &resp.data);
        Ok(ToolResult {
            output: resp.summary.clone(),
            data,
        })
    }
}

#[async_trait]
impl ToolHandler for ListAgents {
    fn name(&self) -> &'static str {
        "list_agents"
    }
    fn description(&self) -> &'static str {
        "列出子 Agent。filter=running（默认）列当前已 spawn 子线程；\
filter=spawnable 列 agents.yaml 中 allow_spawn=true 的可委派目录（含 description / tools）；\
filter=all 同时返回两者。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "parent_thread_id":{"type":"string"},
                "filter":{
                    "type":"string",
                    "description":"running | spawnable | all（默认 running）"
                }
            }
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let filter = args["filter"].as_str().unwrap_or("running");
        let catalog = spawnable_catalog_json(&ctx.config_root);

        if filter == "spawnable" {
            let n = catalog.as_array().map(|a| a.len()).unwrap_or(0);
            return Ok(ToolResult {
                output: if n == 0 {
                    "no spawnable agents in catalog".into()
                } else {
                    format!("spawnable catalog ({n}): {}", catalog)
                },
                data: json!({
                    "filter": "spawnable",
                    "spawnable": catalog,
                }),
            });
        }

        let rt = runtime(ctx)?;
        let parent = parent_id(ctx, &args)?;
        let list = rt.list_agents(&parent).await?;
        let lines: Vec<String> = list
            .iter()
            .map(|a| format!("{} {} [{}]", a.agent_path, a.role, a.lifecycle))
            .collect();
        let running_out = if lines.is_empty() {
            "no child agents".into()
        } else {
            lines.join("\n")
        };

        if filter == "all" {
            return Ok(ToolResult {
                output: format!("{running_out}\n--- spawnable ---\n{catalog}"),
                data: json!({
                    "filter": "all",
                    "agents": list,
                    "spawnable": catalog,
                }),
            });
        }

        // Enrich running agents with catalog invocation/allowSpawn when known.
        let agents_enriched: Vec<Value> = list
            .iter()
            .map(|a| {
                let meta = novelx_harness::lookup_agent(&ctx.config_root, &a.role);
                json!({
                    "threadId": a.thread_id,
                    "role": a.role,
                    "agentPath": a.agent_path,
                    "lifecycle": a.lifecycle,
                    "invocation": meta.as_ref().map(|m| m.invocation.as_str()),
                    "allowSpawn": meta.as_ref().map(|m| m.allow_spawn).unwrap_or(false),
                })
            })
            .collect();

        Ok(ToolResult {
            output: running_out,
            data: json!({
                "filter": "running",
                "agents": agents_enriched,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for InterruptAgent {
    fn name(&self) -> &'static str {
        "interrupt_agent"
    }
    fn description(&self) -> &'static str {
        "中断指定子 Agent turn"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "thread_id":{"type":"string"}
            },
            "required":["thread_id"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let rt = runtime(ctx)?;
        let tid = args["thread_id"].as_str().unwrap_or("");
        rt.interrupt_agent(tid).await?;
        Ok(ToolResult {
            output: format!("interrupted {tid}"),
            data: json!({"threadId": tid}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_wait_contract() {
        let data = json!({
            "summary": "ok",
            "artifacts": [{"agent":"literary_editor","summary":"polished"}],
            "warnings": ["needs_user_choice"],
            "step_ran": true
        });
        let out = normalize_wait_data("t1", "ok", &data);
        assert_eq!(out["summary"], "ok");
        assert!(out["artifacts"].as_array().unwrap().len() == 1);
        assert!(out["warnings"].as_array().unwrap().len() == 1);
        assert_eq!(out["threadId"], "t1");
    }

    #[test]
    fn spawnable_catalog_from_repo() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config");
        if !root.join("agents.yaml").is_file() {
            return;
        }
        let cat = spawnable_catalog_json(&root);
        let arr = cat.as_array().expect("array");
        assert!(arr.iter().any(|e| e["id"] == "literary_editor"));
        assert!(arr.iter().any(|e| e["id"] == "material_researcher"));
        assert!(!arr.iter().any(|e| e["id"] == "writer"));
    }
}
