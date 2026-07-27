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

#[async_trait]
impl ToolHandler for SpawnAgent {
    fn name(&self) -> &'static str {
        "spawn_agent"
    }
    fn description(&self) -> &'static str {
        "启动子 Agent 线程（章纲规划/正文写作等）。完成后用 wait_agent 收取结果。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "role":{"type":"string","description":"agents.yaml 角色 id，如 writer"},
                "task":{"type":"string","description":"交给子 Agent 的任务说明"},
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "mode":{"type":"string","description":"continue|revise|audit_only"},
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
        let resp = rt
            .spawn_agent(SpawnAgentRequest {
                parent_thread_id: parent_id(ctx, &args)?,
                role: role.clone(),
                task: task.clone(),
                project: args["project"].as_str().map(|s| s.to_string()),
                chapter: args["chapter"].as_u64().map(|n| n as u32),
                mode: args["mode"].as_str().map(|s| s.to_string()),
                revision: None,
            })
            .await?;
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
        "等待子 Agent 完成并返回结果摘要"
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
        Ok(ToolResult {
            output: resp.summary.clone(),
            data: json!({
                "threadId": resp.thread_id,
                "summary": resp.summary,
                "result": resp.data,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for ListAgents {
    fn name(&self) -> &'static str {
        "list_agents"
    }
    fn description(&self) -> &'static str {
        "列出当前 root 下已 spawn 的子 Agent"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "parent_thread_id":{"type":"string"}
            }
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let rt = runtime(ctx)?;
        let parent = parent_id(ctx, &args)?;
        let list = rt.list_agents(&parent).await?;
        let lines: Vec<String> = list
            .iter()
            .map(|a| format!("{} {} [{}]", a.agent_path, a.role, a.lifecycle))
            .collect();
        Ok(ToolResult {
            output: if lines.is_empty() {
                "no child agents".into()
            } else {
                lines.join("\n")
            },
            data: json!({"agents": list}),
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
