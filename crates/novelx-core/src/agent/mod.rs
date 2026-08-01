pub mod registry;

use crate::NovelxCore;
use anyhow::Result;
use novelx_protocol::{
    AgentLifecycle, AgentStatus, EventMsg, InterAgentCommunication, InterAgentKind,
    SessionSource, ThreadId, UserInput,
};
use novelx_tools::{
    AgentListEntry, AgentRuntime, SendMessageRequest, SpawnAgentRequest, SpawnAgentResponse,
    WaitAgentResponse,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};

#[derive(Default)]
struct WaitSlot {
    result: Option<WaitAgentResponse>,
    waiters: Vec<oneshot::Sender<WaitAgentResponse>>,
}

#[derive(Default)]
pub struct AgentHub {
    /// parent → children
    children: Mutex<HashMap<ThreadId, Vec<ThreadId>>>,
    /// Single-mutex slot: result + waiters must be updated atomically (avoids lost wakeups).
    slots: Mutex<HashMap<ThreadId, WaitSlot>>,
    core: Mutex<Option<Weak<NovelxCore>>>,
}

impl AgentHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind_core_sync(&self, core: Weak<NovelxCore>) {
        if let Ok(mut g) = self.core.try_lock() {
            *g = Some(core);
        }
    }

    fn upgrade(&self, guard: &Option<Weak<NovelxCore>>) -> Result<Arc<NovelxCore>> {
        guard
            .as_ref()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| anyhow::anyhow!("core runtime gone"))
    }

    pub async fn publish_result(&self, thread_id: &str, summary: String, data: Value) {
        let resp = WaitAgentResponse {
            thread_id: thread_id.to_string(),
            summary,
            data,
        };
        let waiters = {
            let mut slots = self.slots.lock().await;
            let slot = slots.entry(thread_id.to_string()).or_default();
            slot.result = Some(resp.clone());
            std::mem::take(&mut slot.waiters)
        };
        for tx in waiters {
            let _ = tx.send(resp.clone());
        }
    }

    pub async fn register_child(&self, parent: &str, child: &str) {
        self.children
            .lock()
            .await
            .entry(parent.to_string())
            .or_default()
            .push(child.to_string());
    }

    pub async fn children_of(&self, parent: &str) -> Vec<ThreadId> {
        self.children
            .lock()
            .await
            .get(parent)
            .cloned()
            .unwrap_or_default()
    }
}

pub struct CoreAgentRuntime {
    hub: Arc<AgentHub>,
}

impl CoreAgentRuntime {
    pub fn new(hub: Arc<AgentHub>) -> Self {
        Self { hub }
    }
}

#[async_trait::async_trait]
impl AgentRuntime for CoreAgentRuntime {
    async fn spawn_agent(&self, req: SpawnAgentRequest) -> Result<SpawnAgentResponse> {
        if !registry::is_spawnable_role(&req.role) {
            anyhow::bail!("role '{}' is not spawnable", req.role);
        }
        let core = {
            let g = self.hub.core.lock().await;
            self.hub.upgrade(&g)?
        };
        // Chapter pipeline children must be pipeline roles (writer, auditor, …).
        // Use disk `pipeline.yaml` (same as execute), not compile-time defaults only.
        if req.chapter.is_some()
            && req.mode.as_deref().is_some_and(|m| {
                matches!(m, "continue" | "revise" | "audit_only")
            })
            && !registry::is_pipeline_role(&req.role, &core.roots.config_root)
        {
            anyhow::bail!(
                "role '{}' is not a chapter-pipeline agent",
                req.role
            );
        }
        let parent = core
            .get_session_source(&req.parent_thread_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("unknown parent thread"))?;
        let depth = parent.depth() + 1;
        if depth > registry::max_spawn_depth() {
            anyhow::bail!("max spawn depth exceeded");
        }
        let kids = self
            .hub
            .children
            .lock()
            .await
            .get(&req.parent_thread_id)
            .map(|v| v.len())
            .unwrap_or(0);
        if kids >= registry::max_children() {
            anyhow::bail!("max children exceeded");
        }

        let agent_path = parent.agent_path().child(&req.role);
        let project = req
            .project
            .clone()
            .or(core.thread_project(&req.parent_thread_id).await);
        let source = SessionSource::SubAgent {
            parent_thread_id: req.parent_thread_id.clone(),
            role: req.role.clone(),
            depth,
            agent_path: agent_path.clone(),
        };
        let (child_id, _) = core
            .spawn_thread_with_source(project.clone(), false, source.clone())
            .await?;
        self.hub
            .register_child(&req.parent_thread_id, &child_id)
            .await;

        let status = AgentStatus {
            thread_id: child_id.clone(),
            agent_path: agent_path.clone(),
            role: Some(req.role.clone()),
            parent_thread_id: Some(req.parent_thread_id.clone()),
            lifecycle: AgentLifecycle::Spawned,
            summary: Some(req.task.clone()),
        };
        core.emit_global(EventMsg::AgentStatusChanged {
            status: status.clone(),
        })
        .await;

        // Stash pipeline step metadata for the child turn.
        core.set_subagent_job(
            &child_id,
            SubagentJob {
                role: req.role.clone(),
                task: req.task.clone(),
                project: project.clone(),
                chapter: req.chapter,
                mode: req.mode.clone(),
                revision: req.revision.clone(),
            },
        )
        .await;

        let mut items = vec![UserInput::text(req.task.clone())];
        if let Some(skill) = Some(registry::skill_name_for_role(&req.role)) {
            items.push(UserInput::Skill {
                name: skill,
                path: None,
            });
        }
        let _sub = core
            .submit(
                &child_id,
                novelx_protocol::Op::UserInput {
                    thread_id: child_id.clone(),
                    items,
                    skills: vec![registry::skill_name_for_role(&req.role)],
                },
            )
            .await?;

        // Notify parent turn UI.
        let _ = core
            .emit_to_thread(
                &req.parent_thread_id,
                EventMsg::ItemCompleted {
                    thread_id: req.parent_thread_id.clone(),
                    turn_id: core
                        .active_turn_id(&req.parent_thread_id)
                        .await
                        .unwrap_or_else(|| "turn_spawn".into()),
                    item: novelx_protocol::TurnItem::AgentSpawn {
                        id: novelx_protocol::new_id("item"),
                        child_thread_id: child_id.clone(),
                        role: req.role.clone(),
                        agent_path: agent_path.clone(),
                        status: novelx_protocol::ItemStatus::Completed,
                    },
                },
            )
            .await;

        Ok(SpawnAgentResponse {
            thread_id: child_id,
            agent_path,
        })
    }

    async fn send_message(&self, req: SendMessageRequest) -> Result<()> {
        let core = {
            let g = self.hub.core.lock().await;
            self.hub.upgrade(&g)?
        };
        let kind = if req.trigger_turn {
            InterAgentKind::Followup
        } else {
            InterAgentKind::Message
        };
        core.submit(
            &req.recipient_thread_id,
            novelx_protocol::Op::InterAgentCommunication {
                communication: InterAgentCommunication {
                    author_thread_id: req.author_thread_id,
                    recipient_thread_id: req.recipient_thread_id.clone(),
                    content: req.content,
                    trigger_turn: req.trigger_turn,
                    kind,
                },
            },
        )
        .await?;
        Ok(())
    }

    async fn wait_agent(&self, thread_id: &str, timeout: Duration) -> Result<WaitAgentResponse> {
        // Poll the result slot — more reliable than oneshot under nested runtime/load
        // (studio turns were stranded after child publish_result with no waiter wake).
        let start = std::time::Instant::now();
        let mut notified = {
            let mut slots = self.hub.slots.lock().await;
            let slot = slots.entry(thread_id.to_string()).or_default();
            if let Some(done) = slot.result.clone() {
                return Ok(done);
            }
            let (tx, rx) = oneshot::channel();
            slot.waiters.push(tx);
            rx
        };
        loop {
            if let Some(done) = self
                .hub
                .slots
                .lock()
                .await
                .get(thread_id)
                .and_then(|s| s.result.clone())
            {
                return Ok(done);
            }
            if start.elapsed() >= timeout {
                anyhow::bail!("wait_agent timed out for {thread_id}");
            }
            let slice = timeout.saturating_sub(start.elapsed()).min(Duration::from_millis(100));
            tokio::select! {
                biased;
                r = &mut notified => {
                    match r {
                        Ok(resp) => return Ok(resp),
                        Err(_) => {
                            // oneshot closed — keep polling slot
                        }
                    }
                }
                _ = tokio::time::sleep(slice) => {}
            }
        }
    }

    async fn interrupt_agent(&self, thread_id: &str) -> Result<()> {
        let core = {
            let g = self.hub.core.lock().await;
            self.hub.upgrade(&g)?
        };
        core.submit(
            thread_id,
            novelx_protocol::Op::InterruptTurn {
                thread_id: thread_id.to_string(),
                turn_id: None,
            },
        )
        .await?;
        Ok(())
    }

    async fn list_agents(&self, parent_thread_id: &str) -> Result<Vec<AgentListEntry>> {
        let core = {
            let g = self.hub.core.lock().await;
            self.hub.upgrade(&g)?
        };
        let children = self
            .hub
            .children
            .lock()
            .await
            .get(parent_thread_id)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for cid in children {
            if let Some(src) = core.get_session_source(&cid).await {
                let life = core.agent_lifecycle(&cid).await;
                out.push(AgentListEntry {
                    thread_id: cid,
                    role: src.role().unwrap_or("").to_string(),
                    agent_path: src.agent_path().to_string(),
                    lifecycle: format!("{life:?}").to_ascii_lowercase(),
                });
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
pub struct SubagentJob {
    pub role: String,
    pub task: String,
    pub project: Option<String>,
    pub chapter: Option<u32>,
    pub mode: Option<String>,
    pub revision: Option<novelx_pipeline::RevisionOptions>,
}

pub fn result_mail(author: &str, parent: &str, summary: &str) -> InterAgentCommunication {
    InterAgentCommunication {
        author_thread_id: author.to_string(),
        recipient_thread_id: parent.to_string(),
        content: summary.to_string(),
        trigger_turn: false,
        kind: InterAgentKind::Result,
    }
}

pub fn json_step_result(summary: &str, run: &novelx_pipeline::PipelineRun) -> Value {
    json!({
        "step_ran": true,
        "summary": summary,
        "project": run.project,
        "chapter": run.chapter,
        "consistency_passed": run.consistency_passed,
        "needs_user_choice": run.needs_user_choice,
        "revision_applied_via_patch": run.revision_applied_via_patch,
        "report": run.report,
        "status": run.status,
        "volume_ended": run.volume_ended,
        "volume_ended_name": run.volume_ended_name,
        "volume_ended_start": run.volume_ended_start,
        "volume_ended_end": run.volume_ended_end,
        "plots_completed": run.plots_completed,
        "plot_setting_blocker": run.plot_setting_blocker,
    })
}
