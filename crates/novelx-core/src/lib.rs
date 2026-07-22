//! NovelX core — Codex-style Submission / Session / SubAgent runtime.

mod agent;
mod features;
mod gates;
mod intent;
mod session;
mod tasks;
mod thread_store;
mod ui_sync;

use features::FeatureFlags;
use gates::{GateCatalog, GateResolve};
use intent::{parse_chapter_number, ClearHistory, IntentRouter};
use novelx_harness::StudioPolicies;
use ui_sync::{
    append_completion_ui_turn, attach_ui_approval, finish_stale_ui_turns, keep_only_ui_turn,
    mark_ui_turn_complete, sanitize_ui_turns_finish_audits, strip_ui_approvals, ui_turns_weaker_than,
    update_ui_turn_summary,
};

use agent::{json_step_result, result_mail, AgentHub, CoreAgentRuntime, SubagentJob};
use anyhow::Result;
use chrono::Utc;
use novelx_llm::{sanitize_chat_messages, ChatMessage, LlmClient};
use novelx_pipeline::{
    check_plot_write_gate_with, execute_single_agent_step, load_volume_bounds,
    mark_volume_sync_skipped, project_dir, resolve_setup_phase, resolve_volume_phase,
    set_volume_phase, PhaseEnforceFlags, PlotWriteGate, RevisionOptions, RunMode, SetupPhase,
    VolumePhase,
};
use novelx_protocol::{
    new_id, AgentLifecycle, EventMsg, ItemStatus, Op, SessionSource, Submission, ThreadId,
    ThreadSummary, TodoItem, TodoStatus, TurnItem, TurnId, UserInput, UserInputOption,
};
use novelx_skills::{
    build_available_skills, build_skill_injections, collect_explicit_skill_mentions,
    default_skill_roots, load_project_skills, load_skills, SkillMetadata,
};
use novelx_tools::{
    all_tools, dispatch, load_audit_queue, tool_output_for_ui, tool_specs, AuditQueueStatus,
    ToolContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use session::handlers::submission_loop;
use session::{InputQueue, TurnGate};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

const MAX_TOOL_ROUNDS: usize = 8;

struct ThreadRuntime {
    sub_tx: mpsc::Sender<Submission>,
    input_queue: Arc<InputQueue>,
    turn_gate: Arc<TurnGate>,
}

#[derive(Clone)]
pub struct NovelxCore {
    pub roots: ToolContext,
    tools: Vec<Arc<dyn novelx_tools::ToolHandler>>,
    threads: Arc<RwLock<HashMap<ThreadId, ThreadState>>>,
    runtimes: Arc<std::sync::RwLock<HashMap<ThreadId, ThreadRuntime>>>,
    skills: Arc<RwLock<Vec<SkillMetadata>>>,
    hub: Arc<AgentHub>,
    event_bus: broadcast::Sender<EventMsg>,
    /// Per-request event sinks (handle_op compatibility).
    local_sinks: Arc<Mutex<HashMap<ThreadId, Vec<mpsc::UnboundedSender<EventMsg>>>>>,
    self_weak: Weak<NovelxCore>,
    /// Config-driven deterministic intents (`config/intents.yaml`).
    intents: Arc<IntentRouter>,
    gates: Arc<GateCatalog>,
    features: Arc<FeatureFlags>,
    policies: Arc<StudioPolicies>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingAudit {
    pub project: String,
    pub chapter: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingVolumeSync {
    pub project: String,
    pub volume: u32,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingVolumeAudit {
    pub project: String,
    pub volume: u32,
    #[serde(default)]
    pub chapters: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingSetup {
    pub project: String,
}

/// After a chapter write finishes: continue (if published) or revise (if blocked).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingChapterNext {
    pub project: String,
    pub chapter: u32,
    /// Published → only「继续创作」; content-rule block → only「修正本章」.
    #[serde(default)]
    pub published: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ThreadState {
    pub(crate) summary: ThreadSummary,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) abort: bool,
    /// UI timeline turns (JSON) for restore after refresh.
    #[serde(default = "empty_json_array")]
    pub(crate) ui_turns: serde_json::Value,
    /// Last failed audit awaiting user choice (局部修订 / 接受问题).
    #[serde(default)]
    pub(crate) pending_audit: Option<PendingAudit>,
    /// End-of-volume sync awaiting user choice (同步设定库 / 跳过).
    #[serde(default)]
    pub(crate) pending_volume_sync: Option<PendingVolumeSync>,
    /// Volume L1 audit awaiting deep-audit / dismiss.
    #[serde(default)]
    pub(crate) pending_volume_audit: Option<PendingVolumeAudit>,
    /// Outline confirmation awaiting user choice (确认定稿 / 修改再生成).
    #[serde(default)]
    pub(crate) pending_setup: Option<PendingSetup>,
    /// Post-chapter choice: continue writing / revise / other.
    #[serde(default)]
    pub(crate) pending_chapter_next: Option<PendingChapterNext>,
    #[serde(default)]
    pub(crate) session_source: SessionSource,
    #[serde(default)]
    pub(crate) lifecycle: AgentLifecycle,
    #[serde(skip)]
    pub(crate) subagent_job: Option<SubagentJob>,
}

fn empty_json_array() -> serde_json::Value {
    serde_json::json!([])
}

pub type EventTx = mpsc::UnboundedSender<EventMsg>;

fn event_thread_id(ev: &EventMsg) -> Option<&str> {
    match ev {
        EventMsg::SessionConfigured { thread_id, .. }
        | EventMsg::TurnStarted { thread_id, .. }
        | EventMsg::TurnComplete { thread_id, .. }
        | EventMsg::TurnAborted { thread_id, .. }
        | EventMsg::ItemStarted { thread_id, .. }
        | EventMsg::ItemCompleted { thread_id, .. }
        | EventMsg::AgentMessageContentDelta { thread_id, .. }
        | EventMsg::ReasoningContentDelta { thread_id, .. }
        | EventMsg::ToolCallOutputDelta { thread_id, .. }
        | EventMsg::RequestUserInput { thread_id, .. }
        | EventMsg::TodoUpdated { thread_id, .. }
        | EventMsg::ChatHistoryReset { thread_id, .. } => Some(thread_id.as_str()),
        EventMsg::AgentStatusChanged { status } => Some(status.thread_id.as_str()),
        EventMsg::Error { thread_id, .. } | EventMsg::Warning { thread_id, .. } => {
            thread_id.as_deref()
        }
    }
}

impl NovelxCore {
    pub fn new(projects_root: PathBuf, config_root: PathBuf, llm: Arc<LlmClient>) -> Arc<Self> {
        let repo = config_root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let skill_roots = default_skill_roots(&repo);
        let outcome = load_skills(&skill_roots);
        let mut skills = outcome.skills;
        skills.extend(load_project_skills(&projects_root));
        let hub = Arc::new(AgentHub::new());
        let (event_bus, _) = broadcast::channel(512);
        let agent_runtime: Arc<dyn novelx_tools::AgentRuntime> =
            Arc::new(CoreAgentRuntime::new(hub.clone()));
        let intents = Arc::new(
            IntentRouter::load(&config_root).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to load intents.yaml");
                IntentRouter::empty()
            }),
        );
        let gates = Arc::new(
            GateCatalog::load(&config_root).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to load gates.yaml");
                GateCatalog::defaults()
            }),
        );
        let features = Arc::new(
            FeatureFlags::load(&config_root).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to load features.yaml");
                FeatureFlags::defaults()
            }),
        );
        let policies = Arc::new(StudioPolicies::load(&config_root));
        let hub_bind = hub.clone();
        Arc::new_cyclic(|weak| {
            hub_bind.bind_core_sync(weak.clone());
            Self {
                roots: ToolContext {
                    projects_root,
                    config_root,
                    llm,
                    progress: None,
                    agent_runtime: Some(agent_runtime),
                    caller_thread_id: None,
                },
                tools: all_tools(),
                threads: Arc::new(RwLock::new(HashMap::new())),
                runtimes: Arc::new(std::sync::RwLock::new(HashMap::new())),
                skills: Arc::new(RwLock::new(skills)),
                hub,
                event_bus,
                local_sinks: Arc::new(Mutex::new(HashMap::new())),
                self_weak: weak.clone(),
                intents,
                gates,
                features,
                policies,
            }
        })
    }

    fn tool_ctx(&self, thread_id: &str) -> ToolContext {
        let mut ctx = self.roots.clone();
        ctx.caller_thread_id = Some(thread_id.to_string());
        ctx
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<EventMsg> {
        self.event_bus.subscribe()
    }

    pub async fn emit_global(&self, ev: EventMsg) {
        let _ = self.event_bus.send(ev.clone());
        // Clone senders then release the lock before awaiting — holding the mutex
        // across a full channel send deadlocks other emitters (and can strand tools).
        let sinks = if let Some(tid) = event_thread_id(&ev) {
            let guard = self.local_sinks.lock().await;
            guard.get(tid).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        // Unbounded local sinks — never block the tool task on UI backpressure.
        for tx in sinks {
            let _ = tx.send(ev.clone());
        }
    }

    pub async fn emit_to_thread(&self, thread_id: &str, ev: EventMsg) {
        let _ = thread_id;
        self.emit_global(ev).await;
    }

    pub async fn set_abort(&self, thread_id: &str, abort: bool) {
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.abort = abort;
        }
    }

    /// Inject mid-turn steer text into conversation history (Codex pending_input drain).
    async fn drain_pending_steer_into_history(&self, thread_id: &str, turn_id: &str) {
        let queue = {
            let guard = match self.runtimes.read() {
                Ok(g) => g,
                Err(_) => return,
            };
            match guard.get(thread_id) {
                Some(rt) => rt.input_queue.clone(),
                None => return,
            }
        };
        if !queue.has_pending_input().await {
            return;
        }
        let pending = queue.drain_pending_input().await;
        let mut texts = Vec::new();
        for p in pending {
            match p {
                session::TurnInput::UserInput { content } => {
                    texts.push(UserInput::primary_text(&content));
                }
            }
        }
        let joined = texts
            .into_iter()
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if joined.is_empty() {
            return;
        }
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.messages.push(ChatMessage {
                role: "user".into(),
                content: joined.clone(),
                tool_call_id: None,
                tool_calls: None,
                    ..Default::default()
                });
        }
        let item = TurnItem::UserMessage {
            id: new_id("item"),
            text: joined,
        };
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemStarted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: item.clone(),
            },
        )
        .await;
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item,
            },
        )
        .await;
    }

    pub async fn get_session_source(&self, thread_id: &str) -> Option<SessionSource> {
        self.threads
            .read()
            .await
            .get(thread_id)
            .map(|t| t.session_source.clone())
    }

    pub async fn thread_project(&self, thread_id: &str) -> Option<String> {
        self.threads
            .read()
            .await
            .get(thread_id)
            .and_then(|t| t.summary.project.clone())
    }

    pub async fn agent_lifecycle(&self, thread_id: &str) -> AgentLifecycle {
        self.threads
            .read()
            .await
            .get(thread_id)
            .map(|t| t.lifecycle.clone())
            .unwrap_or(AgentLifecycle::Completed)
    }

    pub async fn active_turn_id(&self, thread_id: &str) -> Option<String> {
        let gate = {
            let guard = self.runtimes.read().ok()?;
            guard.get(thread_id)?.turn_gate.clone()
        };
        gate.active_id().await
    }

    /// True when a human gate is open — RegularTask must not auto-drain queued clicks.
    pub async fn thread_awaiting_human(&self, thread_id: &str) -> bool {
        let guard = self.threads.read().await;
        let Some(t) = guard.get(thread_id) else {
            return false;
        };
        t.pending_audit.is_some()
            || t.pending_volume_sync.is_some()
            || t.pending_volume_audit.is_some()
            || t.pending_setup.is_some()
            || t.pending_chapter_next.is_some()
    }

    async fn clear_queued_inputs(&self, thread_id: &str) {
        let queue = {
            let Ok(guard) = self.runtimes.read() else {
                return;
            };
            guard.get(thread_id).map(|r| r.input_queue.clone())
        };
        if let Some(q) = queue {
            q.clear_pending_input().await;
        }
    }

    pub async fn set_subagent_job(&self, thread_id: &str, job: SubagentJob) {
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.subagent_job = Some(job);
            t.lifecycle = AgentLifecycle::Running;
        }
    }

    pub async fn submit(&self, thread_id: &str, op: Op) -> Result<String> {
        self.start_runtime(thread_id)?;
        let op = op.into_canonical();
        let sub = Submission::new(op);
        let id = sub.id.clone();
        let tx = {
            let guard = self
                .runtimes
                .read()
                .map_err(|_| anyhow::anyhow!("runtimes lock poisoned"))?;
            guard
                .get(thread_id)
                .map(|r| r.sub_tx.clone())
                .ok_or_else(|| anyhow::anyhow!("unknown thread {thread_id}"))?
        };
        tx.send(sub)
            .await
            .map_err(|_| anyhow::anyhow!("submission channel closed"))?;
        Ok(id)
    }

    fn arc_self(&self) -> Result<Arc<Self>> {
        self.self_weak
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("NovelxCore weak upgrade failed"))
    }

    fn start_runtime(&self, thread_id: &str) -> Result<()> {
        {
            let guard = self
                .runtimes
                .read()
                .map_err(|_| anyhow::anyhow!("runtimes lock poisoned"))?;
            if guard.contains_key(thread_id) {
                return Ok(());
            }
        }
        let (tx, rx) = mpsc::channel::<Submission>(64);
        let input_queue = Arc::new(InputQueue::new());
        let turn_gate = TurnGate::new();
        {
            let mut guard = self
                .runtimes
                .write()
                .map_err(|_| anyhow::anyhow!("runtimes lock poisoned"))?;
            if guard.contains_key(thread_id) {
                return Ok(());
            }
            guard.insert(
                thread_id.to_string(),
                ThreadRuntime {
                    sub_tx: tx,
                    input_queue: input_queue.clone(),
                    turn_gate: turn_gate.clone(),
                },
            );
        }
        let core = self.arc_self()?;
        let tid = thread_id.to_string();
        tokio::spawn(async move {
            submission_loop(core, tid, rx, input_queue, turn_gate).await;
        });
        Ok(())
    }

    pub async fn reload_skills(&self) {
        let repo = self
            .roots
            .config_root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut skills = load_skills(&default_skill_roots(&repo)).skills;
        skills.extend(load_project_skills(&self.roots.projects_root));
        *self.skills.write().await = skills;
    }

    pub async fn list_skills(&self) -> Vec<SkillMetadata> {
        self.skills.read().await.clone()
    }

    /// Headless chapter run via Session + SubAgent spawn chain (CLI).
    pub async fn run_chapter_headless(
        self: &Arc<Self>,
        project: &str,
        chapter: u32,
        revise: bool,
        instructions: Option<String>,
    ) -> Result<novelx_pipeline::PipelineRun> {
        let (thread_id, _) = self
            .spawn_thread(Some(project.to_string()), false)
            .await?;
        let mode = if revise {
            RunMode::Revise
        } else {
            RunMode::Continue
        };
        let mut rev = RevisionOptions {
            prefer_local_patch: true,
            revision_mode: revise || instructions.is_some(),
            user_instructions: instructions,
            ..Default::default()
        };
        if revise {
            rev.prefer_local_patch = true;
        }
        let ctx = self.tool_ctx(&thread_id);
        // Prefer in-process pipeline; spawn/wait path has stranded studio turns.
        novelx_tools::run_pipeline_streaming(&ctx, project, chapter, mode, rev).await
    }

    pub async fn handle_op(self: &Arc<Self>, op: Op, tx: EventTx) -> Result<()> {
        match op.clone().into_canonical() {
            Op::StartThread { project } => {
                let (id, project) = self.spawn_thread(project, false).await?;
                let src = self
                    .get_session_source(&id)
                    .await
                    .unwrap_or(SessionSource::Root);
                let ev = EventMsg::session_configured(id, project, &src);
                let _ = tx.send(ev.clone());
                self.emit_global(ev).await;
            }
            Op::ResumeThread { thread_id } => {
                if !self.threads.read().await.contains_key(&thread_id) {
                    let summary = ThreadSummary {
                        id: thread_id.clone(),
                        project: None,
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        session_source: Some(SessionSource::Root),
                        agent_path: Some(novelx_protocol::AgentPath::root()),
                    };
                    self.threads.write().await.insert(
                        thread_id.clone(),
                        ThreadState {
                            summary,
                            messages: vec![],
                            abort: false,
                            ui_turns: empty_json_array(),
                            pending_audit: None,
                            pending_volume_sync: None,
                            pending_volume_audit: None,
                            pending_setup: None,
                            pending_chapter_next: None,
                            session_source: SessionSource::Root,
                            lifecycle: AgentLifecycle::Running,
                            subagent_job: None,
                        },
                    );
                    let _ = self.start_runtime(&thread_id);
                }
                let project = self.thread_project(&thread_id).await;
                let src = self
                    .get_session_source(&thread_id)
                    .await
                    .unwrap_or(SessionSource::Root);
                let ev = EventMsg::session_configured(thread_id, project, &src);
                let _ = tx.send(ev.clone());
                self.emit_global(ev).await;
            }
            Op::UserInput {
                thread_id,
                items,
                skills,
            } => {
                {
                    let mut sinks = self.local_sinks.lock().await;
                    sinks.entry(thread_id.clone()).or_default().push(tx.clone());
                }
                let _ = self
                    .submit(
                        &thread_id,
                        Op::UserInput {
                            thread_id: thread_id.clone(),
                            items,
                            skills,
                        },
                    )
                    .await?;
                self.wait_local_turn(&thread_id).await;
                self.local_sinks.lock().await.remove(&thread_id);
            }
            Op::StartTurn { .. } => unreachable!("canonicalized"),
            Op::InterruptTurn { thread_id, turn_id } => {
                let _ = self
                    .submit(
                        &thread_id,
                        Op::InterruptTurn {
                            thread_id: thread_id.clone(),
                            turn_id,
                        },
                    )
                    .await?;
            }
            Op::SteerTurn {
                thread_id,
                turn_id,
                text,
            } => {
                {
                    let mut sinks = self.local_sinks.lock().await;
                    sinks.entry(thread_id.clone()).or_default().push(tx.clone());
                }
                let _ = self
                    .submit(
                        &thread_id,
                        Op::SteerTurn {
                            thread_id: thread_id.clone(),
                            turn_id,
                            text,
                        },
                    )
                    .await?;
                self.wait_local_turn(&thread_id).await;
                self.local_sinks.lock().await.remove(&thread_id);
            }
            Op::RespondUserInput {
                thread_id,
                turn_id,
                option_id,
                free_text,
            } => {
                {
                    let mut sinks = self.local_sinks.lock().await;
                    sinks.entry(thread_id.clone()).or_default().push(tx.clone());
                }
                let _ = self
                    .submit(
                        &thread_id,
                        Op::RespondUserInput {
                            thread_id: thread_id.clone(),
                            turn_id,
                            option_id,
                            free_text,
                        },
                    )
                    .await?;
                self.wait_local_turn(&thread_id).await;
                self.local_sinks.lock().await.remove(&thread_id);
            }
            Op::InterAgentCommunication { communication } => {
                let tid = communication.recipient_thread_id.clone();
                let _ = self
                    .submit(
                        &tid,
                        Op::InterAgentCommunication { communication },
                    )
                    .await?;
            }
            Op::Shutdown { thread_id } => {
                let _ = self
                    .submit(
                        &thread_id,
                        Op::Shutdown {
                            thread_id: thread_id.clone(),
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn wait_local_turn(&self, thread_id: &str) {
        for _ in 0..3_600 {
            let gate = {
                let guard = self.runtimes.read().ok();
                guard.and_then(|g| g.get(thread_id).map(|r| r.turn_gate.clone()))
            };
            let busy = match gate {
                Some(g) => g.has_active().await,
                None => false,
            };
            if !busy {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Session turn entry used by RegularTask.
    pub async fn run_turn_session(
        &self,
        thread_id: &str,
        turn_id: &str,
        items: &[UserInput],
        extra_skills: &[String],
    ) -> Result<()> {
        let text = UserInput::primary_text(items);
        let mut skills = UserInput::collect_skills(items);
        skills.extend(extra_skills.iter().cloned());

        // SubAgent pipeline role: run single step deterministically.
        let job = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|t| t.subagent_job.clone());
        if let Some(job) = job {
            return self
                .run_subagent_pipeline_turn(thread_id, turn_id, &job)
                .await;
        }

        self.run_turn(
            thread_id.to_string(),
            text,
            skills,
            turn_id.to_string(),
        )
        .await
    }

    async fn run_subagent_pipeline_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        job: &SubagentJob,
    ) -> Result<()> {
        self.emit_to_thread(
            thread_id,
            EventMsg::TurnStarted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            },
        )
        .await;
        let project = job
            .project
            .clone()
            .or(self.thread_project(thread_id).await)
            .unwrap_or_default();
        let chapter = job.chapter.unwrap_or(1);
        let mode = match job.mode.as_deref() {
            Some("revise") => RunMode::Revise,
            Some("audit_only") => RunMode::AuditOnly,
            _ => RunMode::Continue,
        };
        let revision = job.revision.clone().unwrap_or_default();
        let step_result = execute_single_agent_step(
            &self.roots.projects_root,
            &self.roots.config_root,
            &project,
            chapter,
            &job.role,
            mode,
            revision,
            self.roots.llm.clone(),
            None,
        )
        .await;
        // Always unblock wait_agent — even when the step itself failed.
        let (summary, data) = match &step_result {
            Ok(run) => {
                let summary = run
                    .steps
                    .first()
                    .map(|s| s.summary.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("{} completed", job.role));
                (summary.clone(), json_step_result(&summary, run))
            }
            Err(e) => (
                format!("{} failed: {e}", job.role),
                json!({
                    "step_ran": false,
                    "summary": format!("{e}"),
                    "error": format!("{e}"),
                }),
            ),
        };
        tracing::info!(%thread_id, role = %job.role, %summary, "subagent step publish_result");
        // Unblock wait_agent first — never await parent I/O before this.
        self.hub
            .publish_result(thread_id, summary.clone(), data)
            .await;
        if let Some(parent) = self
            .get_session_source(thread_id)
            .await
            .and_then(|s| s.parent_thread_id().map(|p| p.to_string()))
        {
            // Fire-and-forget: awaiting parent submit while parent is inside wait_agent
            // previously stranded the child after publish on a full/slow channel.
            let mail = result_mail(thread_id, &parent, &summary);
            let core = self.arc_self().ok();
            if let Some(core) = core {
                tokio::spawn(async move {
                    let _ = core
                        .submit(
                            &parent,
                            Op::InterAgentCommunication {
                                communication: mail,
                            },
                        )
                        .await;
                });
            }
        }
        let ok = step_result.is_ok();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.lifecycle = if ok {
                AgentLifecycle::Completed
            } else {
                AgentLifecycle::Failed
            };
            t.subagent_job = None;
            t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: summary.clone(),
                tool_call_id: None,
                tool_calls: None,
                    ..Default::default()
                });
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: new_id("item"),
                    text: summary.clone(),
                    status: if ok {
                        ItemStatus::Completed
                    } else {
                        ItemStatus::Failed
                    },
                },
            },
        )
        .await;
        self.emit_to_thread(
            thread_id,
            EventMsg::TurnComplete {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            },
        )
        .await;
        if let Some(src) = self.get_session_source(thread_id).await {
            self.emit_global(EventMsg::AgentStatusChanged {
                status: novelx_protocol::AgentStatus {
                    thread_id: thread_id.to_string(),
                    agent_path: src.agent_path(),
                    role: src.role().map(|r| r.to_string()),
                    parent_thread_id: src.parent_thread_id().map(|p| p.to_string()),
                    lifecycle: if ok {
                        AgentLifecycle::Completed
                    } else {
                        AgentLifecycle::Failed
                    },
                    summary: Some(summary),
                },
            })
            .await;
        }
        // Parent already unblocked via publish_result; surface step errors in logs only.
        if let Err(e) = step_result {
            tracing::warn!(%thread_id, role = %job.role, error = %e, "subagent step failed");
        }
        Ok(())
    }

    async fn run_turn(
        &self,
        thread_id: ThreadId,
        text: String,
        extra_skills: Vec<String>,
        turn_id: TurnId,
    ) -> Result<()> {
        self.emit_to_thread(
            &thread_id,
            EventMsg::TurnStarted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            },
        )
        .await;

        {
            let mut guard = self.threads.write().await;
            if let Some(t) = guard.get_mut(&thread_id) {
                t.abort = false;
                t.messages.push(ChatMessage {
                    role: "user".into(),
                    content: text.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
            }
        }

        let user_item = TurnItem::UserMessage {
            id: new_id("item"),
            text: text.clone(),
        };
        self.emit_to_thread(
            &thread_id,
            EventMsg::ItemStarted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: user_item.clone(),
            },
        )
        .await;
        self.emit_to_thread(
            &thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: user_item,
            },
        )
        .await;

        // Progressive skill disclosure
        let skills = self.skills.read().await.clone();
        let mut activate = collect_explicit_skill_mentions(&text);
        activate.extend(extra_skills);
        for name in &activate {
            let item_id = new_id("item");
            let path = skills.iter().find(|s| s.name == *name).map(|s| s.path.display().to_string());
            let loading = TurnItem::SkillLoad {
                id: item_id.clone(),
                name: name.clone(),
                path: path.clone(),
                status: ItemStatus::InProgress,
            };
            self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: loading,
                }).await;
            let loaded = TurnItem::SkillLoad {
                id: item_id,
                name: name.clone(),
                path,
                status: ItemStatus::Completed,
            };
            self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: loaded,
                }).await;
        }

        let bound_project = self
            .threads
            .read()
            .await
            .get(&thread_id)
            .and_then(|t| t.summary.project.clone());

        // Deterministic: volume L1 audit gate (deep audit / dismiss).
        if let Some((tool_name, args)) = self
            .parse_volume_audit_op(&thread_id, &text)
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct volume audit gate");
            if tool_name == "__dismiss_volume_audit" {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.pending_volume_audit = None;
                }
                let agent_item_id = new_id("item");
                let summary = "已结束卷级复盘。需要时再说「审这一卷」或「审阅第N-M章」。".to_string();
                self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id.clone(),
                            text: String::new(),
                            status: ItemStatus::InProgress,
                        },
                    }).await;
                self.emit_to_thread(&thread_id, EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id: agent_item_id.clone(),
                        delta: summary.clone(),
                    }).await;
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: summary.clone(),
                        tool_call_id: None,
                        tool_calls: None,
                        ..Default::default()
                    });
                }
                self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id,
                            text: summary,
                            status: ItemStatus::Completed,
                        },
                    }).await;
                let _ = self.persist_thread(&thread_id).await;
                self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                    }).await;
                return Ok(());
            }
            // Deep audit → audit_chapters with chapters[]
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_volume_audit = None;
                t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
            }
            let _ = self.persist_thread(&thread_id).await;
            let agent_item_id = new_id("item");
            self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                }).await;
            let intro = format!(
                "已按卷复盘建议建立深审队列（《{}》）…",
                args.get("project").and_then(|v| v.as_str()).unwrap_or("?")
            );
            self.emit_to_thread(&thread_id, EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.clone(),
                }).await;
            let (output, data) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            let mut summary = format!("{intro}\n\n{output}");
            let asked_audit = self
                .maybe_offer_audit_fix(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            if asked_audit {
                summary.push_str("\n\n请选择下一步：按审校局部修订 / 跳过，审下一章 / 结束审阅队列。");
            }
            self.emit_todos_from_data(&thread_id, &data).await;
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
                t.ui_turns = append_completion_ui_turn(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &summary,
                    asked_audit,
                );
            }
            self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary,
                        status: ItemStatus::Completed,
                    },
                }).await;
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                }).await;
            return Ok(());
        }

        // Deterministic: post-chapter continue / revise (before setup / volume).
        if let Some((tool_name, args)) = self
            .parse_chapter_next_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct chapter_next (skip LLM routing)");
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_chapter_next = None;
                t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
            }
            // Re-enter the same intent path by synthesizing a direct tool turn below —
            // fall through using the same pattern as setup: run tool + optional next gate.
            let agent_item_id = new_id("item");
            self.emit_to_thread(
                &thread_id,
                EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                },
            )
            .await;
            let intro = format!(
                "已识别指令，正在对《{}》执行 {tool_name}…",
                bound_project.as_deref().unwrap_or("?")
            );
            self.emit_to_thread(
                &thread_id,
                EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.clone(),
                },
            )
            .await;
            // Clear only when the write will actually start (plot gate + no draft_exists block).
            if tool_name == "continue_writing"
                && self.features.clear_history_on_new_chapter()
            {
                let project = args
                    .get("project")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| bound_project.as_deref().unwrap_or(""))
                    .to_string();
                let chapter = self.resolve_continue_chapter(&project, &args);
                if !project.is_empty()
                    && chapter > 0
                    && self.continue_writing_should_clear_history(&project, &args, &text)
                {
                    self.begin_new_chapter_history(
                        &thread_id,
                        &turn_id,
                        &project,
                        chapter,
                        &text,
                    )
                    .await;
                }
            }
            let (output, data) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            let mut summary = format!("{intro}\n\n{output}");
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                // Blocked writes: keep prior turns (don't collapse history into one blob).
                if data.get("blocked").and_then(|v| v.as_bool()) != Some(true) {
                    t.ui_turns = append_completion_ui_turn(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &summary,
                        false,
                    );
                }
            }
            let asked_audit = self
                .maybe_offer_audit_fix(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            let asked_volume = if asked_audit {
                false
            } else {
                self.maybe_offer_volume_sync(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_next = if asked_audit || asked_volume {
                false
            } else {
                self.maybe_offer_chapter_next(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            if asked_audit {
                summary.push_str("\n\n请选择下一步：按审校局部修订 / 接受问题 / 其他。");
            } else if asked_volume {
                summary.push_str("\n\n本卷已结束 — 请选择：同步设定库 / 跳过。");
            } else if asked_next {
                let published = self
                    .threads
                    .read()
                    .await
                    .get(&thread_id)
                    .and_then(|t| t.pending_chapter_next.as_ref())
                    .map(|p| p.published)
                    .unwrap_or(true);
                if published {
                    summary.push_str("\n\n请选择：继续创作；或其他说明。");
                } else {
                    summary.push_str("\n\n请选择：修正本章；或其他说明。");
                }
            }
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
                t.ui_turns = update_ui_turn_summary(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &summary,
                );
            }
            self.emit_to_thread(
                &thread_id,
                EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary,
                        status: ItemStatus::Completed,
                    },
                },
            )
            .await;
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(
                &thread_id,
                EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                },
            )
            .await;
            return Ok(());
        }

        // Free-text while chapter_next gate open → dismiss gate, let studio LLM handle.
        if self
            .threads
            .read()
            .await
            .get(&thread_id)
            .and_then(|t| t.pending_chapter_next.clone())
            .is_some()
        {
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_chapter_next = None;
                t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
            }
            let _ = self.persist_thread(&thread_id).await;
        }

        // Deterministic: setup confirm / revise (before volume sync).
        if let Some((tool_name, args)) = self
            .parse_setup_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct setup confirm (skip LLM routing)");
            let agent_item_id = new_id("item");
            self.emit_to_thread(
                &thread_id,
                EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                },
            )
            .await;
            let (output, _) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let summary = if action == "approve" {
                format!("{output}\n\n下一步：design_plot → update_plot(in_progress) → continue_writing。")
            } else {
                format!("{output}\n\n请按修改意见重新 design_master_outline / design_arc_outline。")
            };
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_setup = None;
            }
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
            }
            self.emit_to_thread(
                &thread_id,
                EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary,
                        status: ItemStatus::Completed,
                    },
                },
            )
            .await;
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(
                &thread_id,
                EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                },
            )
            .await;
            return Ok(());
        }

        // Deterministic: volume-end sync / skip (before audit steer).
        if let Some((tool_name, args)) = self
            .parse_volume_sync_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct volume sync (skip LLM routing)");
            let agent_item_id = new_id("item");
            self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                }).await;
            let intro = if tool_name == "sync_volume" {
                format!(
                    "已收到选择，正在同步《{}》第{}卷设定库…",
                    args.get("project").and_then(|v| v.as_str()).unwrap_or("?"),
                    args.get("volume").and_then(|v| v.as_u64()).unwrap_or(0)
                )
            } else {
                "已跳过卷末设定同步。".into()
            };
            self.emit_to_thread(&thread_id, EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.clone(),
                }).await;
            let handoff = "\n\n---\n**卷间交接（请按序完成后再续写）**\n\
1. **必须** `design_arc_outline` — 细化下卷：开卷状态、节点阶梯、≥2 条可核验终止条件；承接上卷章末钩子与总纲「卷末修订」\n\
2. `design_plot` — 创建下卷第一张主线剧情卡（含收束条件；无卷纲会失败）\n\
3. `update_plot(..., status=in_progress, set_active_main=true)` — 激活该卡\n\
4. 再 `continue_writing`\n\
说明：`sync_volume` 不写剧情卡；实体多为短摘要（含 status/holdings），关键卡请用 `design_entity` 或在 Web「设定缺口」补全。无进行中剧情卡时续写会被拦截。";
            let summary = if tool_name == "__skip_volume_sync" {
                // Persist handoff phase even when sync is skipped.
                let project = {
                    let guard = self.threads.read().await;
                    guard.get(&thread_id).and_then(|t| {
                        t.pending_volume_sync
                            .as_ref()
                            .map(|p| p.project.clone())
                            .or_else(|| t.summary.project.clone())
                    })
                }
                .or_else(|| bound_project.clone());
                if let Some(project) = project {
                    let dir = project_dir(&self.roots.projects_root, &project);
                    let _ = mark_volume_sync_skipped(&dir, true);
                    let _ = set_volume_phase(&dir, VolumePhase::AwaitingNextArc);
                }
                format!("{intro}{handoff}")
            } else {
                let (output, _) = self
                    .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                    .await?;
                format!("{intro}\n\n{output}{handoff}")
            };
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_volume_sync = None;
            }
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
            }
            self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary,
                        status: ItemStatus::Completed,
                    },
                }).await;
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                }).await;
            return Ok(());
        }

        // Stale gate token after queue finished — do not steer / do not ask the LLM.
        if self.gates.is_known_token(&text)
            && self
                .threads
                .read()
                .await
                .get(&thread_id)
                .and_then(|t| t.pending_audit.clone())
                .is_none()
            && self
                .threads
                .read()
                .await
                .get(&thread_id)
                .and_then(|t| t.pending_volume_sync.clone())
                .is_none()
            && self
                .threads
                .read()
                .await
                .get(&thread_id)
                .and_then(|t| t.pending_volume_audit.clone())
                .is_none()
            && self
                .threads
                .read()
                .await
                .get(&thread_id)
                .and_then(|t| t.pending_setup.clone())
                .is_none()
            && self
                .threads
                .read()
                .await
                .get(&thread_id)
                .and_then(|t| t.pending_chapter_next.clone())
                .is_none()
        {
            let project = bound_project.as_deref().unwrap_or("");
            let queue_gone = project.is_empty()
                || load_audit_queue(&self.roots.projects_root, project).is_none();
            if queue_gone {
                let msg = "审阅队列已结束，没有待处理的审校选项。如需再审，请重新发送「审阅1-8章」或「审这一卷」。";
                let agent_item_id = new_id("item");
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::ItemStarted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id.clone(),
                            text: String::new(),
                            status: ItemStatus::InProgress,
                        },
                    },
                )
                .await;
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id: agent_item_id.clone(),
                        delta: msg.into(),
                    },
                )
                .await;
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
                    t.messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: msg.into(),
                        tool_call_id: None,
                        tool_calls: None,
                    ..Default::default()
                });
                }
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id,
                            text: msg.into(),
                            status: ItemStatus::Completed,
                        },
                    },
                )
                .await;
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::RequestUserInput {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        prompt: "审阅队列已结束。".into(),
                        options: vec![],
                    },
                )
                .await;
                let _ = self.persist_thread(&thread_id).await;
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::TurnComplete {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                    },
                )
                .await;
                return Ok(());
            }
        }

        // Deterministic: audit fix / queue choices (before chapter rewrite heuristics).
        if let Some((tool_name, args)) = self
            .parse_audit_steer_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct audit steer (skip LLM routing)");
            let agent_item_id = new_id("item");
            self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                }).await;
            let intro = format!(
                "已收到选择，正在对《{}》执行 {tool_name}…",
                bound_project.as_deref().unwrap_or("?")
            );
            self.emit_to_thread(&thread_id, EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.clone(),
                }).await;
            // Live progress stays on tool cards (+ mirrored status into this bubble).
            // Do not dump full tool reports into the agent message at the end.
            let (_output, mut data) = self
                .run_one_tool_mirrored(
                    &thread_id,
                    &turn_id,
                    &tool_name,
                    &args.to_string(),
                    Some(&agent_item_id),
                )
                .await?;
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_audit = None;
            }
            // After local revise: queue → always re-audit current (pipeline must not stall);
            // single chapter → audit_chapter when feature flag is on.
            let mut gate_tool = tool_name.as_str();
            if tool_name == "steer_run" {
                if let Some(project) = args.get("project").and_then(|v| v.as_str()) {
                    let chapter = args
                        .get("chapter")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1) as u32;
                    let has_queue =
                        load_audit_queue(&self.roots.projects_root, project).is_some();
                    if has_queue {
                        self.emit_to_thread(
                            &thread_id,
                            EventMsg::AgentMessageContentDelta {
                                thread_id: thread_id.clone(),
                                turn_id: turn_id.clone(),
                                item_id: agent_item_id.clone(),
                                delta: "\n\n正在接续审阅队列…".into(),
                            },
                        )
                        .await;
                        let cont = json!({
                            "project": project,
                            "action": "continue"
                        });
                        let (_out2, data2) = self
                            .run_one_tool_mirrored(
                                &thread_id,
                                &turn_id,
                                "audit_chapters",
                                &cont.to_string(),
                                Some(&agent_item_id),
                            )
                            .await?;
                        data = data2;
                        gate_tool = "audit_chapters";
                    } else if self.features.auto_reaudit_after_steer() {
                        self.emit_to_thread(
                            &thread_id,
                            EventMsg::AgentMessageContentDelta {
                                thread_id: thread_id.clone(),
                                turn_id: turn_id.clone(),
                                item_id: agent_item_id.clone(),
                                delta: format!("\n\n正在复审第{chapter}章…"),
                            },
                        )
                        .await;
                        let audit_args = json!({
                            "project": project,
                            "chapter": chapter,
                        });
                        let (_out2, data2) = self
                            .run_one_tool_mirrored(
                                &thread_id,
                                &turn_id,
                                "audit_chapter",
                                &audit_args.to_string(),
                                Some(&agent_item_id),
                            )
                            .await?;
                        data = data2;
                        gate_tool = "audit_chapter";
                    }
                }
            }
            let mut summary = intro;
            // Persist completion turn BEFORE gate attach so RequestUserInput lands on it.
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.ui_turns = append_completion_ui_turn(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &summary,
                    false,
                );
            }
            let mut asked = self
                .maybe_offer_audit_fix(
                    &thread_id,
                    &turn_id,
                    gate_tool,
                    &args,
                    &data,
                )
                .await?;
            // Safety net: revise cleared pending_audit; if queue still needs a decision,
            // rebuild the gate so the pipeline cannot silently end.
            // Never re-offer when this round's audit already passed.
            let reaudit_passed = data
                .get("consistency_passed")
                .and_then(|v| v.as_bool())
                == Some(true);
            if !asked && tool_name == "steer_run" && !reaudit_passed {
                asked = self
                    .maybe_reoffer_queue_gate(&thread_id, &turn_id)
                    .await?;
            }
            let closing = if asked {
                "\n\n请选择下一步。"
            } else if (gate_tool == "audit_chapter" || gate_tool == "audit_chapters")
                && reaudit_passed
            {
                "\n\n复审通过，本轮已正常结束。"
            } else if tool_name == "steer_run" {
                "\n\n修订已完成。"
            } else {
                ""
            };
            if !closing.is_empty() {
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id: agent_item_id.clone(),
                        delta: closing.into(),
                    },
                )
                .await;
                summary.push_str(closing);
            }
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
                // Refresh summary text on the completion turn (gate may already be attached).
                t.ui_turns = update_ui_turn_summary(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &summary,
                );
            }
            self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary.clone(),
                        status: ItemStatus::Completed,
                    },
                }).await;
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                }).await;
            return Ok(());
        }

        // Deterministic intents from config/intents.yaml (Codex-style data-driven routing).
        if self.features.deterministic_intents() {
        if let Some(intent) = self
            .intents
            .match_text(&text, bound_project.as_deref())
        {
            let tool_name = intent.tool.clone();
            let args = intent.args.clone();
            tracing::info!(
                intent = %intent.id,
                %tool_name,
                args = %args,
                "studio intent match (skip LLM routing)"
            );
            let agent_item_id = new_id("item");
            self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                }).await;
            let intro = if tool_name == "audit_chapters" {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("start");
                if action == "continue" {
                    format!(
                        "正在续跑《{}》的审阅队列…",
                        bound_project.as_deref().unwrap_or("?")
                    )
                } else {
                    format!(
                        "已建立逐章审阅队列，正在对《{}》执行…",
                        bound_project.as_deref().unwrap_or("?")
                    )
                }
            } else {
                format!(
                    "已识别指令，正在对《{}》执行 {tool_name}…",
                    bound_project.as_deref().unwrap_or("?")
                )
            };
            self.emit_to_thread(&thread_id, EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.clone(),
                }).await;
            if tool_name == "continue_writing"
                && intent.clear_history == ClearHistory::OnStart
                && self.features.clear_history_on_new_chapter()
            {
                let project = args
                    .get("project")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| bound_project.as_deref().unwrap_or(""))
                    .to_string();
                let chapter = self.resolve_continue_chapter(&project, &args);
                if !project.is_empty()
                    && chapter > 0
                    && self.continue_writing_should_clear_history(&project, &args, &text)
                {
                    self.begin_new_chapter_history(
                        &thread_id,
                        &turn_id,
                        &project,
                        chapter,
                        &text,
                    )
                    .await;
                }
            }
            let (output, data) = self
                .run_one_tool(
                    &thread_id,
                    &turn_id,
                    &tool_name,
                    &args.to_string()
                )
                .await?;
            let mut summary = format!("{intro}\n\n{output}");
            if tool_name == "continue_writing"
                && data.get("blocked").and_then(|v| v.as_bool()) != Some(true)
            {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.ui_turns = append_completion_ui_turn(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &summary,
                        false,
                    );
                }
            }
            let asked_setup = self
                .maybe_offer_setup_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            let asked_volume = if asked_setup {
                false
            } else {
                self.maybe_offer_volume_sync(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_vol_audit = if asked_setup || asked_volume {
                false
            } else {
                self.maybe_offer_volume_audit(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_audit = if asked_setup || asked_volume || asked_vol_audit {
                false
            } else {
                self.maybe_offer_audit_fix(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_next = if asked_setup || asked_volume || asked_vol_audit || asked_audit {
                false
            } else {
                self.maybe_offer_chapter_next(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            if asked_setup {
                summary.push_str("\n\n总纲与卷纲已就绪 — 请选择：确认定稿 / 修改再生成。");
            } else if asked_volume {
                summary.push_str("\n\n本卷已结束 — 请选择：同步设定库 / 跳过。");
            } else if asked_vol_audit {
                summary.push_str("\n\n请选择：按建议深审 / 结束复盘。");
            } else if asked_audit {
                if tool_name == "audit_chapters" {
                    summary.push_str("\n\n请选择下一步：按审校局部修订 / 跳过，审下一章 / 结束审阅队列。");
                } else {
                    summary.push_str("\n\n请选择下一步：按审校局部修订 / 接受问题 / 其他。");
                }
            } else if asked_next {
                let published = self
                    .threads
                    .read()
                    .await
                    .get(&thread_id)
                    .and_then(|t| t.pending_chapter_next.as_ref())
                    .map(|p| p.published)
                    .unwrap_or(true);
                if published {
                    summary.push_str("\n\n请选择：继续创作；或其他说明。");
                } else {
                    summary.push_str("\n\n请选择：修正本章；或其他说明。");
                }
            }
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
                if tool_name == "continue_writing" || tool_name == "revise_chapter" {
                    t.ui_turns = update_ui_turn_summary(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &summary,
                    );
                }
            }
            self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary,
                        status: ItemStatus::Completed,
                    },
                }).await;
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                }).await;
            return Ok(());
        }
        } // features.deterministic_intents

        // Bare「继续」while next_chapter already has a draft: ask, don't tool-call / wipe chat.
        if let Some(project) = bound_project.as_deref() {
            if let Some(clarify) = self.clarify_bare_continue(project, &text) {
                tracing::info!(%project, "bare continue with existing draft — clarify without tools");
                let agent_item_id = new_id("item");
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::ItemStarted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id.clone(),
                            text: String::new(),
                            status: ItemStatus::InProgress,
                        },
                    },
                )
                .await;
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item_id: agent_item_id.clone(),
                        delta: clarify.clone(),
                    },
                )
                .await;
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: clarify.clone(),
                        tool_call_id: None,
                        tool_calls: None,
                        ..Default::default()
                    });
                    t.ui_turns = append_completion_ui_turn(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &clarify,
                        false,
                    );
                }
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id,
                            text: clarify,
                            status: ItemStatus::Completed,
                        },
                    },
                )
                .await;
                let _ = self.persist_thread(&thread_id).await;
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::TurnComplete {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                    },
                )
                .await;
                return Ok(());
            }
        }

        let injections = build_skill_injections(&skills, &activate);
        let skills_catalog = build_available_skills(&skills, Some(4000));
        let studio_body = std::fs::read_to_string(
            self.roots.config_root.join("skills/studio.md"),
        )
        .unwrap_or_default();
        let project_bind = if let Some(p) = bound_project.as_deref() {
            format!(
                "当前会话已绑定项目《{p}》。用户指令默认针对此书。\n\
                 - 禁止无谓调用 list_projects（除非用户明确要「列出所有项目」）。\n\
             - 「修正/扩写/重写/加长第N章」→ 立即 revise_chapter(project=\"{p}\", chapter=N, instructions=用户原话或「扩写到3000-5000字」)。\n\
             - 用户只说「继续」且未点名章号：不要擅自 continue_writing(chapter=N)，先问清是写下一章 / 修订本章 / 审校。\n\
             - 不要只列项目或只口头答应就结束；同一轮必须把可执行工具跑完。\n"
            )
        } else {
            "尚未绑定项目：若用户已点名书名则直接用该 project；仅当完全不清楚时才 list_projects，然后必须继续执行原请求。\n".into()
        };
        let mut system = format!(
            "你是 NovelX，小说创作编排 Agent。\n\
             {project_bind}\
             优先局部修订正文，避免无必要时全文重写。\n\
             需要操作项目时，必须通过 API function calling 调用工具，\
             不要输出 XML、<function_calls>、<invoke> 或伪代码。\n\
             工具参数使用：project（项目名）、chapter（章节号整数）、instructions（修订说明）。\n\
             字数太少/扩写/重写/修正章节必须调用 revise_chapter，禁止用 audit_chapter 代替写章。\n\
             整卷复盘（审这一卷/卷末复盘）必须调用 audit_volume（摘要层），不要默认 audit_chapters 扫整卷。\n\
             审校多章（如1-8章）或「按建议深审」必须调用 audit_chapters（可用 chapters 列表），禁止同轮多次 audit_chapter。\n\
             单章审校用 audit_chapter。未通过后引导选择；队列模式下可选「跳过，审下一章」。\n\n{skills_catalog}"
        );
        if !studio_body.trim().is_empty() {
            system.push_str("\n\n# Skill: studio\n\n");
            system.push_str(&studio_body);
        }
        for inj in &injections {
            if inj.name == "studio" {
                continue; // already injected as default
            }
            system.push_str(&format!(
                "\n\n# Skill: {}\n\n{}",
                inj.name, inj.body
            ));
        }

        let specs = tool_specs(&self.tools);
        let agent_item_id = new_id("item");
        self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id.clone(),
                    text: String::new(),
                    status: ItemStatus::InProgress,
                },
            }).await;

        let mut final_text = String::new();
        let mut last_tool_names: Vec<String> = Vec::new();
        let mut nudged_continue = false;
        let user_needs_chapter_op = self
            .intents
            .match_text(&text, Some("__intent_probe__"))
            .is_some();
        let mut pause_for_human = false;
        let mut started_new_chapter = false;
        let mut spam_tool_name = String::new();
        let mut spam_tool_streak: usize = 0;

        for _round in 0..MAX_TOOL_ROUNDS {
            if self
                .threads
                .read()
                .await
                .get(&thread_id)
                .map(|t| t.abort)
                .unwrap_or(false)
            {
                self.emit_to_thread(&thread_id, EventMsg::TurnAborted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        reason: "interrupted".into(),
                    }).await;
                return Ok(());
            }

            // Codex: drain mid-turn steer between sample/tool rounds.
            self.drain_pending_steer_into_history(&thread_id, &turn_id)
                .await;

            let messages = {
                // Repair any stale incomplete tool_calls chains (e.g. after audit gate).
                {
                    let mut guard = self.threads.write().await;
                    if let Some(t) = guard.get_mut(&thread_id) {
                        t.messages = sanitize_chat_messages(std::mem::take(&mut t.messages));
                    }
                }
                let guard = self.threads.read().await;
                let mut msgs = vec![ChatMessage {
                    role: "system".into(),
                    content: system.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                }];
                if let Some(t) = guard.get(&thread_id) {
                    msgs.extend(t.messages.clone());
                }
                msgs
            };

            // Prefer studio task model (faster / bounded) for the tool loop.
            let studio_model = self.roots.llm.model_for_agent("studio_agent");
            let core_delta = self.clone();
            let thread_delta = thread_id.clone();
            let turn_delta = turn_id.clone();
            let item_delta = agent_item_id.clone();
            let mut streamed = String::new();
            let result = self
                .roots
                .llm
                .complete_messages_stream(
                    messages,
                    Some(&studio_model),
                    Some(&specs),
                    |delta| {
                        let core_delta = core_delta.clone();
                        let thread_delta = thread_delta.clone();
                        let turn_delta = turn_delta.clone();
                        let item_delta = item_delta.clone();
                        async move {
                            core_delta
                                .emit_to_thread(
                                    &thread_delta,
                                    EventMsg::AgentMessageContentDelta {
                                        thread_id: thread_delta.clone(),
                                        turn_id: turn_delta,
                                        item_id: item_delta,
                                        delta,
                                    },
                                )
                                .await;
                        }
                    },
                )
                .await?;
            streamed.push_str(&result.content);

            // Codex-like: tool rounds show ToolCall items; keep prose for the agent bubble.
            let show_text = !result.content.is_empty()
                && (result.tool_calls.is_empty() || result.content.chars().count() > 8);
            if show_text {
                final_text.push_str(&streamed);
            }

            if result.tool_calls.is_empty() {
                // Flash often stops after list_projects; nudge once to finish the real op.
                let only_listed = !last_tool_names.is_empty()
                    && last_tool_names.iter().all(|n| n == "list_projects");
                if user_needs_chapter_op && only_listed && !nudged_continue {
                    nudged_continue = true;
                    let proj = bound_project.clone().unwrap_or_else(|| "（会话项目）".into());
                    let nudge = format!(
                        "不要结束。用户原请求尚未执行。请立即对项目「{proj}」调用 revise_chapter \
                         （或 continue_writing），chapter 与 instructions 从用户原话提取；\
                         禁止再次 list_projects。"
                    );
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.messages.push(ChatMessage {
                            role: "assistant".into(),
                            content: result.content.clone(),
                            tool_call_id: None,
                            tool_calls: None,
                    ..Default::default()
                });
                        t.messages.push(ChatMessage {
                            role: "user".into(),
                            content: nudge,
                            tool_call_id: None,
                            tool_calls: None,
                    ..Default::default()
                });
                    }
                    tracing::warn!("studio stopped after list_projects; nudging to continue chapter op");
                    continue;
                }
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: result.content,
                        tool_call_id: None,
                        tool_calls: None,
                    ..Default::default()
                });
                }
                break;
            }

            last_tool_names = result.tool_calls.iter().map(|tc| tc.name.clone()).collect();
            let mut pending_tool_calls = result.tool_calls.clone();

            // Break query_lore / list_projects spin loops (model retries same read tool).
            if let Some(first) = pending_tool_calls.first() {
                let spammy = matches!(
                    first.name.as_str(),
                    "query_lore" | "query_memory" | "list_projects" | "list_entities"
                );
                if spammy && spam_tool_name == first.name {
                    spam_tool_streak += 1;
                } else if spammy {
                    spam_tool_name = first.name.clone();
                    spam_tool_streak = 1;
                } else {
                    spam_tool_name.clear();
                    spam_tool_streak = 0;
                }
                if spam_tool_streak >= 3 {
                    let nudge = if first.name == "query_lore" {
                        "停止重复 query_lore。它只返回设定上下文，不含正文。\
                         若要核对时间线/伤势/能力位置，请立即调用 read_chapter；\
                         若要修订请调用 revise_chapter / steer_run。不要再 query_lore。"
                            .to_string()
                    } else {
                        format!(
                            "停止重复调用 {}。请改用能推进任务的工具（read_chapter / revise_chapter / continue_writing / get_project_status），或直接用中文回复用户。",
                            first.name
                        )
                    };
                    let skipped_calls = serde_json::to_value(
                        pending_tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments,
                                    }
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap_or_default();
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.messages.push(ChatMessage {
                            role: "assistant".into(),
                            content: result.content.clone(),
                            tool_call_id: None,
                            tool_calls: Some(skipped_calls),
                            reasoning_content: result.reasoning_content.clone(),
                        });
                        for tc in &pending_tool_calls {
                            t.messages.push(ChatMessage {
                                role: "tool".into(),
                                content: format!(
                                    "（已跳过：连续 {} 次重复 {}，避免空转）",
                                    spam_tool_streak, first.name
                                ),
                                tool_call_id: Some(tc.id.clone()),
                                tool_calls: None,
                                reasoning_content: None,
                            });
                        }
                        t.messages.push(ChatMessage {
                            role: "user".into(),
                            content: nudge,
                            tool_call_id: None,
                            tool_calls: None,
                            reasoning_content: None,
                        });
                    }
                    tracing::warn!(
                        tool = %first.name,
                        streak = spam_tool_streak,
                        "studio repeated read-tool loop; nudging and skipping round"
                    );
                    spam_tool_streak = 0;
                    spam_tool_name.clear();
                    continue;
                }
            }

            // Chapter number: only trust the USER. Models invent「第7章」then we used to
            // wipe history and jump chapters on a bare「继续」.
            for tc in &mut pending_tool_calls {
                if tc.name != "continue_writing" {
                    continue;
                }
                let mut args_val: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or_default();
                if parse_chapter_number(&text).is_none() {
                    if let Some(obj) = args_val.as_object_mut() {
                        if obj.remove("chapter").is_some() {
                            tracing::info!(
                                "stripped model-invented continue_writing.chapter (user did not name a chapter)"
                            );
                        }
                    }
                } else if fill_continue_chapter_arg(&mut args_val, &[&text]) {
                    tracing::info!(
                        args = %args_val,
                        "filled continue_writing.chapter from user text"
                    );
                }
                if let Ok(s) = serde_json::to_string(&args_val) {
                    tc.arguments = s;
                }
            }

            // Clear prior chat only when user clearly starts a new chapter write.
            if !started_new_chapter && self.features.clear_history_on_new_chapter() {
                if let Some(tc) = pending_tool_calls.iter().find(|tc| tc.name == "continue_writing")
                {
                    let args_val: serde_json::Value =
                        serde_json::from_str(&tc.arguments).unwrap_or_default();
                    let project = args_val
                        .get("project")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let chapter = self.resolve_continue_chapter(&project, &args_val);
                    if !project.is_empty()
                        && chapter > 0
                        && self.continue_writing_should_clear_history(&project, &args_val, &text)
                    {
                        self.begin_new_chapter_history(
                            &thread_id,
                            &turn_id,
                            &project,
                            chapter,
                            &text,
                        )
                        .await;
                        started_new_chapter = true;
                    }
                }
            }

            // Persist assistant tool_calls message
            let tool_calls_json = serde_json::to_value(
                pending_tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {"name": tc.name, "arguments": tc.arguments}
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default();

            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: result.content.clone(),
                    tool_call_id: None,
                    tool_calls: Some(tool_calls_json),
                    // DeepSeek thinking + tools: must echo reasoning_content back.
                    reasoning_content: result.reasoning_content.clone(),
                });
            }

            let mut answered_ids: Vec<String> = Vec::new();
            for tc in &pending_tool_calls {
                let item_id = new_id("item");
                let args_val: serde_json::Value =
                    serde_json::from_str(&tc.arguments).unwrap_or_default();
                let started = TurnItem::ToolCall {
                    id: item_id.clone(),
                    name: tc.name.clone(),
                    arguments: args_val.clone(),
                    output: None,
                    status: ItemStatus::InProgress,
                    duration_ms: None,
                };
                self.emit_to_thread(&thread_id, EventMsg::ItemStarted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: started,
                    }).await;

                let start = std::time::Instant::now();
                let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<String>();
                let mut tool_ctx = self.tool_ctx(&thread_id);
                tool_ctx.progress = Some(prog_tx);
                let core_prog = self.clone();
                let thread_prog = thread_id.clone();
                let turn_prog = turn_id.clone();
                let item_prog = item_id.clone();
                let mut forward = tokio::spawn(async move {
                    coalesce_tool_progress(
                        &mut prog_rx,
                        &core_prog,
                        &thread_prog,
                        &turn_prog,
                        &item_prog,
                        None,
                    )
                    .await;
                });

                let tool_result =
                    dispatch(&self.tools, &tool_ctx, &tc.name, &tc.arguments).await;
                drop(tool_ctx);
                // Progress flusher can stall on a full WS sink; never block ItemCompleted.
                if tokio::time::timeout(std::time::Duration::from_secs(2), &mut forward)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        tool = %tc.name,
                        "tool progress flusher timed out; aborting flush to unblock turn"
                    );
                    forward.abort();
                }
                let duration_ms = start.elapsed().as_millis() as u64;
                let (output, status, data) = match tool_result {
                    Ok(r) => (r.output, ItemStatus::Completed, r.data),
                    Err(e) => (e.to_string(), ItemStatus::Failed, json!({})),
                };
                // Chat/WS: compact bulk context tools; model messages keep full `output`.
                let ui_output = tool_output_for_ui(&tc.name, &args_val, &output, &data)
                    .unwrap_or_else(|| output.clone());

                if !ui_output.is_empty() {
                    self.emit_to_thread(&thread_id, EventMsg::ToolCallOutputDelta {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            item_id: item_id.clone(),
                            delta: ui_output.clone(),
                        }).await;
                }

                let completed = TurnItem::ToolCall {
                    id: item_id,
                    name: tc.name.clone(),
                    arguments: args_val.clone(),
                    output: Some(ui_output),
                    status,
                    duration_ms: Some(duration_ms),
                };
                self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: completed,
                    }).await;

                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.messages.push(ChatMessage {
                        role: "tool".into(),
                        content: output,
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: None,
                    ..Default::default()
                });
                }
                answered_ids.push(tc.id.clone());

                // Setup confirm / volume end / volume audit / chapter audit → stop entire turn.
                if self
                    .maybe_offer_setup_confirm(
                        &thread_id,
                        &turn_id,
                        &tc.name,
                        &args_val,
                        &data,
                    )
                    .await?
                    || self
                        .maybe_offer_volume_sync(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
                        .maybe_offer_volume_audit(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
                        .maybe_offer_audit_fix(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                {
                    pause_for_human = true;
                    break;
                }
                // One chapter per user turn — offer continue/revise, do not auto-chain.
                if tc.name == "continue_writing" || tc.name == "revise_chapter" {
                    let _ = self
                        .maybe_offer_chapter_next(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?;
                    pause_for_human = true;
                    break;
                }
            }
            // Human gate / early stop: stub remaining tool_call_ids so next turn's history is valid.
            if pause_for_human {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    for tc in &pending_tool_calls {
                        if answered_ids.iter().any(|id| id == &tc.id) {
                            continue;
                        }
                        t.messages.push(ChatMessage {
                            role: "tool".into(),
                            content: format!(
                                "（未执行：上一工具需用户确认后结束本轮，跳过 {}）",
                                tc.name
                            ),
                            tool_call_id: Some(tc.id.clone()),
                            tool_calls: None,
                    ..Default::default()
                });
                    }
                    // Persist a repaired history for the next user turn.
                    t.messages = sanitize_chat_messages(std::mem::take(&mut t.messages));
                }
                break;
            }
        }

        self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id,
                    text: final_text,
                    status: ItemStatus::Completed,
                },
            }).await;

        // Server-authoritative: clear「调用模型中」spinners even if the client
        // missed ItemCompleted / TurnComplete over a congested WebSocket.
        if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
            t.ui_turns = mark_ui_turn_complete(std::mem::take(&mut t.ui_turns), &turn_id);
        }
        let _ = self.persist_thread(&thread_id).await;

        self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            }).await;
        Ok(())
    }

    async fn persist_thread(&self, thread_id: &str) -> Result<()> {
        let guard = self.threads.read().await;
        let Some(state) = guard.get(thread_id) else {
            return Ok(());
        };
        let persisted = thread_store::from_state(thread_id, state);
        drop(guard);
        thread_store::save_project_thread(&self.roots.projects_root, &persisted)?;
        Ok(())
    }

    /// Start or resume a project thread. `fresh=true` clears history and opens a new task.
    pub async fn spawn_thread(
        self: &Arc<Self>,
        project: Option<String>,
        fresh: bool,
    ) -> Result<(ThreadId, Option<String>)> {
        self.spawn_thread_with_source(project, fresh, SessionSource::Root)
            .await
    }

    pub async fn spawn_thread_with_source(
        self: &Arc<Self>,
        project: Option<String>,
        fresh: bool,
        source: SessionSource,
    ) -> Result<(ThreadId, Option<String>)> {
        if fresh {
            if let Some(proj) = project.as_deref() {
                let _ = thread_store::clear_project_thread(&self.roots.projects_root, proj);
                let mut guard = self.threads.write().await;
                guard.retain(|_, t| t.summary.project.as_deref() != Some(proj));
            }
        } else if source.is_root() {
            if let Some(proj) = project.as_deref() {
                if let Some(persisted) =
                    thread_store::load_project_thread(&self.roots.projects_root, proj)
                {
                    let (id, state) = thread_store::into_state(persisted);
                    self.threads.write().await.insert(id.clone(), state);
                    self.start_runtime(&id)?;
                    return Ok((id, project));
                }
            }
        }

        let id = new_id("thr");
        let summary = ThreadSummary {
            id: id.clone(),
            project: project.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            session_source: Some(source.clone()),
            agent_path: Some(source.agent_path()),
        };
        self.threads.write().await.insert(
            id.clone(),
            ThreadState {
                summary: summary.clone(),
                messages: vec![],
                abort: false,
                ui_turns: empty_json_array(),
                pending_audit: None,
                pending_volume_sync: None,
                pending_volume_audit: None,
                pending_setup: None,
                pending_chapter_next: None,
                session_source: source,
                lifecycle: AgentLifecycle::Running,
                subagent_job: None,
            },
        );
        self.start_runtime(&id)?;
        let _ = self.persist_thread(&id).await;
        Ok((id, project))
    }

    async fn run_one_tool(
        &self,
        thread_id: &str,
        turn_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<(String, Value)> {
        self.run_one_tool_mirrored(thread_id, turn_id, name, arguments, None)
            .await
    }

    /// Like [`Self::run_one_tool`], but optionally mirrors pipeline status heartbeats into an
    /// agent bubble so gate/steer turns show mid-process without waiting for the final summary.
    async fn run_one_tool_mirrored(
        &self,
        thread_id: &str,
        turn_id: &str,
        name: &str,
        arguments: &str,
        mirror_agent_item: Option<&str>,
    ) -> Result<(String, Value)> {
        let item_id = new_id("item");
        let args_val: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemStarted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::ToolCall {
                    id: item_id.clone(),
                    name: name.to_string(),
                    arguments: args_val.clone(),
                    output: None,
                    status: ItemStatus::InProgress,
                    duration_ms: None,
                },
            },
        )
        .await;

        let start = std::time::Instant::now();
        let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<String>();
        let mut tool_ctx = self.tool_ctx(thread_id);
        tool_ctx.progress = Some(prog_tx);
        let core_prog = self.clone();
        let thread_prog = thread_id.to_string();
        let turn_prog = turn_id.to_string();
        let item_prog = item_id.clone();
        let mirror_prog = mirror_agent_item.map(|s| s.to_string());
        let mut forward = tokio::spawn(async move {
            coalesce_tool_progress(
                &mut prog_rx,
                &core_prog,
                &thread_prog,
                &turn_prog,
                &item_prog,
                mirror_prog.as_deref(),
            )
            .await;
        });

        let tool_result = dispatch(&self.tools, &tool_ctx, name, arguments).await;
        drop(tool_ctx);
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut forward)
            .await
            .is_err()
        {
            tracing::warn!(%name, "direct tool progress flusher timed out; aborting flush");
            forward.abort();
        }
        let duration_ms = start.elapsed().as_millis() as u64;
        let (output, status, data) = match tool_result {
            Ok(r) => (r.output, ItemStatus::Completed, r.data),
            Err(e) => (e.to_string(), ItemStatus::Failed, json!({})),
        };
        let ui_output =
            tool_output_for_ui(name, &args_val, &output, &data).unwrap_or_else(|| output.clone());

        if !ui_output.is_empty() {
            self.emit_to_thread(
                thread_id,
                EventMsg::ToolCallOutputDelta {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: item_id.clone(),
                    delta: ui_output.clone(),
                },
            )
            .await;
        }

        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::ToolCall {
                    id: item_id,
                    name: name.to_string(),
                    arguments: args_val,
                    output: Some(ui_output),
                    status,
                    duration_ms: Some(duration_ms),
                },
            },
        )
        .await;

        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: Some(json!([{
                    "id": format!("direct_{name}"),
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                }])),
                    ..Default::default()
                });
            t.messages.push(ChatMessage {
                role: "tool".into(),
                content: output.clone(),
                tool_call_id: Some(format!("direct_{name}")),
                tool_calls: None,
                    ..Default::default()
                });
        }
        Ok((output, data))
    }

    fn resolve_continue_chapter(&self, project: &str, args: &Value) -> u32 {
        if let Some(c) = args.get("chapter").and_then(|v| v.as_u64()) {
            if c > 0 {
                return c as u32;
            }
        }
        if project.is_empty() {
            return 0;
        }
        let dir = novelx_pipeline::project_dir(&self.roots.projects_root, project);
        novelx_pipeline::load_project_state(&dir)
            .map(|s| s.next_chapter.max(1))
            .unwrap_or(1)
    }

    /// Whether continue_writing is expected to pass the plot gate (so clearing chat is safe).
    fn continue_writing_may_clear_history(&self, project: &str) -> bool {
        if project.is_empty() {
            return false;
        }
        let dir = project_dir(&self.roots.projects_root, project);
        let enforce = PhaseEnforceFlags {
            setup: self.features.enforce_setup_gate(),
            volume: self.features.enforce_volume_phase(),
        };
        matches!(
            check_plot_write_gate_with(&dir, enforce),
            PlotWriteGate::Allow { .. }
        )
    }

    /// Safe to wipe prior Studio chat before continue_writing.
    /// Must stay in sync with ContinueWriting's draft_exists_without_chapter guard —
    /// otherwise we clear history (and pending gates) then immediately block.
    fn continue_writing_should_clear_history(
        &self,
        project: &str,
        args: &Value,
        user_text: &str,
    ) -> bool {
        if !self.continue_writing_may_clear_history(project) {
            return false;
        }
        // Never wipe on ambiguous bare「继续」— model may invent chapter=N and flash the UI.
        let chapter_named = parse_chapter_number(user_text).is_some();
        if !self
            .policies
            .user_intends_new_chapter_write(user_text, chapter_named)
        {
            return false;
        }
        let explicit = args.get("chapter").and_then(|v| v.as_u64()).map(|c| c as u32);
        let chapter = self.resolve_continue_chapter(project, args);
        if chapter == 0 {
            return false;
        }
        if explicit.is_none() {
            let dir = project_dir(&self.roots.projects_root, project);
            let draft = novelx_pipeline::read_chapter_draft(&dir, chapter).unwrap_or_default();
            if draft.chars().count() >= self.policies.draft_min_chars() {
                return false;
            }
        }
        true
    }

    /// When user only says「继续」and next_chapter already has a draft, return a clarify prompt.
    fn clarify_bare_continue(&self, project: &str, user_text: &str) -> Option<String> {
        if !self.policies.is_bare_continue(user_text) {
            return None;
        }
        let dir = project_dir(&self.roots.projects_root, project);
        let state = novelx_pipeline::load_project_state(&dir).ok()?;
        let chapter = state.next_chapter.max(1);
        let draft = novelx_pipeline::read_chapter_draft(&dir, chapter).unwrap_or_default();
        let chars = draft.chars().count();
        if chars < self.policies.draft_min_chars() {
            return None;
        }
        let suggest = chapter.saturating_add(1);
        Some(format!(
            "第{chapter}章已有正文（约 {chars} 字），尚未发布（published_count={}，next_chapter={chapter}）。\n\n\
             「继续」有歧义，请直接说明下一步：\n\
             - 写第{suggest}章\n\
             - 修订第{chapter}章\n\
             - 审校第{chapter}章",
            state.published_count
        ))
    }

    /// At the start of a new-chapter write: drop prior chat noise, keep the current turn.
    /// Project files are untouched. Does not wipe the in-flight write UI.
    async fn begin_new_chapter_history(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        chapter: u32,
        user_text: &str,
    ) {
        let summary = format!(
            "已清理先前对话，开始撰写第{chapter}章（《{project}》）。设定与正文以项目文件为准。"
        );
        {
            let mut guard = self.threads.write().await;
            let Some(t) = guard.get_mut(thread_id) else {
                return;
            };
            let mut msgs = Vec::new();
            let user = user_text.trim();
            if !user.is_empty() {
                msgs.push(ChatMessage {
                    role: "user".into(),
                    content: user.to_string(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
            }
            t.messages = msgs;
            t.ui_turns = keep_only_ui_turn(std::mem::take(&mut t.ui_turns), turn_id);
            t.pending_audit = None;
            t.pending_volume_audit = None;
            t.pending_chapter_next = None;
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::ChatHistoryReset {
                thread_id: thread_id.to_string(),
                summary,
                keep_turn_id: Some(turn_id.to_string()),
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        tracing::info!(%project, chapter, "cleared prior chat history for new chapter write");
    }

    /// Attach the open human gate onto `ui_turns` (HTTP restore) and optionally re-emit WS.
    async fn sync_pending_gate_into_ui(&self, thread_id: &str, emit_ws: bool) {
        let (setup, volume, vol_audit, audit, chapter_next, turn_id) = {
            let guard = self.threads.read().await;
            let Some(t) = guard.get(thread_id) else {
                return;
            };
            let turn_id = t
                .ui_turns
                .as_array()
                .and_then(|arr| arr.last())
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("restored-gate")
                .to_string();
            (
                t.pending_setup.clone(),
                t.pending_volume_sync.clone(),
                t.pending_volume_audit.clone(),
                t.pending_audit.clone(),
                t.pending_chapter_next.clone(),
                turn_id,
            )
        };
        let (prompt, options) = if let Some(s) = setup {
            (
                format!(
                    "《{}》总纲与卷纲已就绪，是否确认定稿并进入写章？",
                    s.project
                ),
                self.setup_confirm_options(),
            )
        } else if let Some(v) = volume {
            let label = if v.name.is_empty() {
                format!("第{}卷", v.volume)
            } else {
                format!("第{}卷「{}」", v.volume, v.name)
            };
            (
                format!("{label}已结束，是否同步设定库？"),
                self.volume_sync_options(),
            )
        } else if let Some(va) = vol_audit {
            let list = va
                .chapters
                .iter()
                .map(|c| format!("第{c}章"))
                .collect::<Vec<_>>()
                .join("、");
            (
                format!(
                    "第{}卷复盘完成。建议深审：{list}。是否建立逐章正文审阅队列？",
                    va.volume
                ),
                self.volume_audit_options(),
            )
        } else if let Some(a) = audit {
            let queue_active =
                load_audit_queue(&self.roots.projects_root, &a.project).is_some();
            let prompt = if queue_active {
                format!(
                    "第{}章审校未通过（审阅队列 · 仅处理当前章），请选择：",
                    a.chapter
                )
            } else {
                format!("第{}章审校未通过，请选择如何处理：", a.chapter)
            };
            let options = if queue_active {
                self.audit_queue_options()
            } else {
                self.audit_options()
            };
            (prompt, options)
        } else if let Some(c) = chapter_next {
            let options = self.chapter_next_options(c.published);
            let prompt = if c.published {
                format!(
                    "第{}章已发布。请选择：继续创作；也可点「其他」说明要求。",
                    c.chapter
                )
            } else {
                format!(
                    "第{}章因硬规则未发布。请选择：修正本章；也可点「其他」说明要求。",
                    c.chapter
                )
            };
            (prompt, options)
        } else {
            return;
        };

        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                &turn_id,
                &prompt,
                &options,
            );
        }

        if emit_ws {
            self.emit_to_thread(
                thread_id,
                EventMsg::RequestUserInput {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.clone(),
                    prompt: prompt.clone(),
                    options: options.clone(),
                },
            )
            .await;
        }
    }

    /// After volume L1 audit, offer deep-audit of suggested chapters.
    async fn maybe_offer_volume_audit(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if tool_name != "audit_volume" {
            return Ok(false);
        }
        if data.get("needs_user_choice").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(false);
        }
        let chapters: Vec<u32> = data
            .get("suggested_chapters")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_u64().map(|n| n as u32))
                    .filter(|&n| n >= 1)
                    .collect()
            })
            .unwrap_or_default();
        if chapters.is_empty() {
            return Ok(false);
        }
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(false);
        }
        let volume = data
            .get("volume_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let list = chapters
            .iter()
            .map(|c| format!("第{c}章"))
            .collect::<Vec<_>>()
            .join("、");
        let prompt = format!(
            "第{volume}卷复盘完成。建议深审：{list}。是否建立逐章正文审阅队列？"
        );
        let options = self.volume_audit_options();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_volume_audit = Some(PendingVolumeAudit {
                project: project.clone(),
                volume,
                chapters: chapters.clone(),
            });
            t.pending_audit = None;
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &options,
            );
        }
        self.clear_queued_inputs(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt,
                options,
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    /// After master+arc outlines are ready, ask user to confirm setup.
    async fn maybe_offer_setup_confirm(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if !self.features.enforce_setup_gate() {
            return Ok(false);
        }
        if tool_name != "design_arc_outline" && tool_name != "design_master_outline" {
            return Ok(false);
        }
        let offer = data
            .get("offer_setup_confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || data.get("setup_phase").and_then(|v| v.as_str()) == Some("awaiting_confirm");
        if !offer {
            return Ok(false);
        }
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(false);
        }
        let prompt = format!("《{project}》总纲与卷纲已就绪，是否确认定稿并进入写章？");
        let options = self.setup_confirm_options();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_setup = Some(PendingSetup {
                project: project.clone(),
            });
            t.pending_audit = None;
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &options,
            );
        }
        self.clear_queued_inputs(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt,
                options,
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    /// After a volume ends (publish advanced to end chapter), ask sync/skip.
    async fn maybe_offer_volume_sync(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if tool_name != "continue_writing" && tool_name != "revise_chapter" {
            return Ok(false);
        }
        let Some(volume) = data.get("volume_ended").and_then(|v| v.as_u64()) else {
            return Ok(false);
        };
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(false);
        }
        let name = data
            .get("volume_ended_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let start = data
            .get("volume_ended_start")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let end = data
            .get("volume_ended_end")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let label = if name.is_empty() {
            format!("第{volume}卷")
        } else {
            format!("第{volume}卷「{name}」")
        };
        let range = if start > 0 && end > 0 {
            format!("（第{start}–{end}章）")
        } else {
            String::new()
        };
        let prompt = format!("{label}{range}已结束，是否同步设定库？");
        let options = self.volume_sync_options();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_volume_sync = Some(PendingVolumeSync {
                project: project.clone(),
                volume: volume as u32,
                name: name.clone(),
            });
            t.pending_audit = None;
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &options,
            );
        }
        self.clear_queued_inputs(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt,
                options,
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    /// If disk queue is stuck on a failed chapter but `pending_audit` is gone,
    /// restore the gate so options / steer still work (e.g. after silent revise stop).
    async fn maybe_reoffer_queue_gate(&self, thread_id: &str, turn_id: &str) -> Result<bool> {
        let project = {
            let guard = self.threads.read().await;
            let Some(t) = guard.get(thread_id) else {
                return Ok(false);
            };
            if t.pending_audit.is_some() {
                return Ok(false);
            }
            t.summary.project.clone().unwrap_or_default()
        };
        if project.is_empty() {
            return Ok(false);
        }
        let Some(q) = load_audit_queue(&self.roots.projects_root, &project) else {
            return Ok(false);
        };
        if q.is_finished() {
            return Ok(false);
        }
        let Some(chapter) = q.current_chapter() else {
            return Ok(false);
        };
        let failed = q
            .results
            .get(q.index)
            .map(|r| r.status == AuditQueueStatus::Failed)
            .unwrap_or(false);
        if !failed {
            return Ok(false);
        }
        let data = json!({
            "needs_user_choice": true,
            "consistency_passed": false,
            "project": project,
            "chapter": chapter,
            "queue_active": true,
            "audit_queue": q.to_json(),
            "todos": q.to_codex_todos(),
        });
        self.maybe_offer_audit_fix(
            thread_id,
            turn_id,
            "audit_chapters",
            &json!({ "project": project, "action": "continue" }),
            &data,
        )
        .await
    }

    /// Restore `pending_audit` from an unfinished failed queue entry (refresh / orphan).
    async fn reconcile_orphan_audit_queue(&self, thread_id: &str) {
        let mut guard = self.threads.write().await;
        let Some(t) = guard.get_mut(thread_id) else {
            return;
        };
        if t.pending_audit.is_some() {
            return;
        }
        let Some(project) = t.summary.project.as_deref() else {
            return;
        };
        let Some(q) = load_audit_queue(&self.roots.projects_root, project) else {
            return;
        };
        if q.is_finished() {
            return;
        }
        let Some(chapter) = q.current_chapter() else {
            return;
        };
        let failed = q
            .results
            .get(q.index)
            .map(|r| r.status == AuditQueueStatus::Failed)
            .unwrap_or(false);
        if !failed {
            return;
        }
        tracing::info!(
            %project,
            chapter,
            "reconcile orphan audit queue → restore pending_audit"
        );
        t.pending_audit = Some(PendingAudit {
            project: project.to_string(),
            chapter,
        });
    }

    /// Restore setup / volume-sync gates from durable project phases when thread pending was lost.
    async fn reconcile_phase_gates(&self, thread_id: &str) {
        let project = {
            let guard = self.threads.read().await;
            let Some(t) = guard.get(thread_id) else {
                return;
            };
            t.summary.project.clone()
        };
        let Some(project) = project.filter(|p| !p.is_empty()) else {
            return;
        };
        let dir = project_dir(&self.roots.projects_root, &project);

        // Setup confirm from meta.setup_phase.
        if self.features.enforce_setup_gate() {
            let needs_setup = {
                let guard = self.threads.read().await;
                guard
                    .get(thread_id)
                    .map(|t| t.pending_setup.is_none())
                    .unwrap_or(false)
            };
            if needs_setup && resolve_setup_phase(&dir) == SetupPhase::AwaitingConfirm {
                tracing::info!(%project, "reconcile setup_phase=awaiting_confirm → pending_setup");
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.pending_setup = Some(PendingSetup {
                        project: project.clone(),
                    });
                }
            }
        }

        // Volume sync from state.meta.volume_phase.
        if self.features.enforce_volume_phase() {
            let needs_vol = {
                let guard = self.threads.read().await;
                guard
                    .get(thread_id)
                    .map(|t| t.pending_volume_sync.is_none())
                    .unwrap_or(false)
            };
            if needs_vol && resolve_volume_phase(&dir) == VolumePhase::AwaitingSync {
                let bounds = load_volume_bounds(&dir);
                let ended = bounds.iter().rev().find(|b| b.completed).cloned();
                let (volume, name) = ended
                    .map(|b| (b.volume_index, b.name))
                    .unwrap_or((1, String::new()));
                tracing::info!(
                    %project,
                    volume,
                    "reconcile volume_phase=awaiting_sync → pending_volume_sync"
                );
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.pending_volume_sync = Some(PendingVolumeSync {
                        project: project.clone(),
                        volume,
                        name,
                    });
                }
            }
        }

        // Align chapter_next.published with disk (legacy threads omit the field).
        if let Ok(st) = novelx_pipeline::load_project_state(&dir) {
            if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                if let Some(c) = t.pending_chapter_next.as_mut() {
                    let on_disk = st.published_count >= c.chapter;
                    if c.published != on_disk {
                        tracing::info!(
                            %project,
                            chapter = c.chapter,
                            published = on_disk,
                            "reconcile chapter_next.published from disk"
                        );
                        c.published = on_disk;
                    }
                }
            }
        }
    }

    /// Clear pending audit gate + approval cards (Codex-style empty RequestUserInput).
    async fn dismiss_audit_gate(&self, thread_id: &str, turn_id: &str, prompt: &str) {
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_audit = None;
            t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt: prompt.into(),
                options: vec![],
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
    }

    /// After a failed audit, persist pending chapter and show fix options.
    async fn maybe_offer_audit_fix(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if tool_name != "audit_chapter"
            && tool_name != "audit_chapters"
            && tool_name != "continue_writing"
        {
            return Ok(false);
        }
        // Volume-end prompt takes precedence over audit options.
        if data.get("volume_ended").and_then(|v| v.as_u64()).is_some() {
            return Ok(false);
        }
        if data.get("queue_finished").and_then(|v| v.as_bool()) == Some(true)
            || data.get("queue_cancelled").and_then(|v| v.as_bool()) == Some(true)
        {
            self.dismiss_audit_gate(thread_id, turn_id, "审阅队列已结束。")
                .await;
            self.emit_todos_from_data(thread_id, data).await;
            return Ok(false);
        }
        // Passed audit wins over stale needs_user_choice / leftover approval cards.
        let passed = data.get("consistency_passed").and_then(|v| v.as_bool()) == Some(true);
        if passed {
            self.dismiss_audit_gate(thread_id, turn_id, "复审通过，本轮已正常结束。")
                .await;
            return Ok(false);
        }
        let needs = data
            .get("needs_user_choice")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || data.get("status").and_then(|v| v.as_str()) == Some("awaiting_human")
            || data.get("consistency_passed").and_then(|v| v.as_bool()) == Some(false);
        if !needs {
            return Ok(false);
        }
        // Prefer tool result data (queue may have advanced past args.chapter).
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let chapter = data
            .get("chapter")
            .and_then(|v| v.as_u64())
            .or_else(|| args.get("chapter").and_then(|v| v.as_u64()))
            .unwrap_or(1) as u32;
        if project.is_empty() {
            return Ok(false);
        }
        let queue_active = data.get("queue_active").and_then(|v| v.as_bool()).unwrap_or(false)
            || load_audit_queue(&self.roots.projects_root, &project).is_some();
        let options = if queue_active {
            self.audit_queue_options()
        } else {
            self.audit_options()
        };
        let prompt = if queue_active {
            format!("第{chapter}章审校未通过（审阅队列 · 仅处理当前章），请选择：")
        } else {
            format!("第{chapter}章审校未通过，请选择如何处理：")
        };
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_audit = Some(PendingAudit {
                project: project.clone(),
                chapter,
            });
            // Drop volume-audit gate so option id "1" cannot restart deep-audit queue.
            t.pending_volume_audit = None;
            // Persist gate on the latest UI turn so refresh / stale clients still see options.
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &options,
            );
        }
        // Only push todos when the multi-chapter queue is active — empty TodoUpdated
        // used to wipe approval cards on the client.
        if queue_active {
            self.emit_todos_from_data(thread_id, data).await;
        }
        // Drop duplicate option clicks queued while the audit tool was still running.
        self.clear_queued_inputs(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt,
                options,
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    async fn emit_todos_from_data(&self, thread_id: &str, data: &Value) {
        let todos = parse_todo_items(data.get("todos")).unwrap_or_else(|| {
            data.get("audit_queue")
                .and_then(|q| q.get("chapters"))
                .and_then(|_| {
                    // Fallback: rebuild from persisted queue file.
                    let project = data.get("project").and_then(|v| v.as_str())?;
                    let q = load_audit_queue(&self.roots.projects_root, project)?;
                    parse_todo_items(Some(&json!(q.to_codex_todos())))
                })
                .unwrap_or_default()
        });
        self.emit_to_thread(
            thread_id,
            EventMsg::TodoUpdated {
                thread_id: thread_id.to_string(),
                todos,
            },
        )
        .await;
    }

    async fn parse_volume_audit_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let (pending, chapter_gate_open) = {
            let guard = self.threads.read().await;
            let th = guard.get(thread_id)?;
            // Chapter audit options take precedence — never let volume_audit steal "1"/"2".
            (
                th.pending_volume_audit.clone(),
                th.pending_audit.is_some(),
            )
        };
        if chapter_gate_open {
            return None;
        }
        let pending = pending?;
        // Resolve first so dismiss still works while a queue is active.
        let resolved = self
            .gates
            .resolve_volume_audit(t, &pending.project, &pending.chapters)?;
        // Duplicate「按建议深审」while a queue already runs → drop stale volume gate only.
        if matches!(resolved, GateResolve::Tool { .. })
            && load_audit_queue(&self.roots.projects_root, &pending.project).is_some()
        {
            tracing::warn!(
                project = %pending.project,
                "ignore duplicate volume deep-audit; audit queue already active"
            );
            if let Some(th) = self.threads.write().await.get_mut(thread_id) {
                th.pending_volume_audit = None;
            }
            return None;
        }
        match resolved {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume => Some(("__dismiss_volume_audit".into(), json!({}))),
            GateResolve::SteerInstructions { .. } => None,
        }
    }

    async fn parse_setup_op(
        &self,
        thread_id: &str,
        text: &str,
        bound_project: Option<&str>,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_setup.clone())?;
        let _ = bound_project;
        match self.gates.resolve_setup(t, &pending.project)? {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume | GateResolve::SteerInstructions { .. } => None,
        }
    }

    async fn parse_volume_sync_op(
        &self,
        thread_id: &str,
        text: &str,
        bound_project: Option<&str>,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_volume_sync.clone());

        if let Some(pending) = pending {
            return match self.gates.resolve_volume(t, &pending.project, pending.volume)? {
                GateResolve::Tool { name, args } => Some((name, args)),
                GateResolve::SkipVolume => Some(("__skip_volume_sync".into(), json!({}))),
                GateResolve::SteerInstructions { .. } => None,
            };
        }

        // Explicit manual sync without an open gate — phrases from config/policies.yaml.
        if !self.policies.is_manual_volume_sync(t) {
            return None;
        }
        let project = bound_project?.to_string();
        let vol = extract_volume_number(t).unwrap_or(1);
        Some((
            "sync_volume".into(),
            json!({ "project": project, "volume": vol }),
        ))
    }

    async fn parse_audit_steer_op(
        &self,
        thread_id: &str,
        text: &str,
        bound_project: Option<&str>,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        // Gate choices require an open pending_audit — never fall back to audit.json scans.
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_audit.clone())?;
        let _ = bound_project; // project comes from pending gate
        let queue_active =
            load_audit_queue(&self.roots.projects_root, &pending.project).is_some();
        match self
            .gates
            .resolve_audit(t, &pending.project, pending.chapter, queue_active)?
        {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume => None,
            GateResolve::SteerInstructions { instructions } => Some((
                "steer_run".into(),
                json!({
                    "project": pending.project,
                    "chapter": pending.chapter,
                    "choice": "revise",
                    "instructions": instructions,
                }),
            )),
        }
    }

    /// Snapshot for HTTP restore (messages + UI turns).
    pub async fn thread_snapshot(&self, thread_id: &str) -> Option<serde_json::Value> {
        self.reconcile_orphan_audit_queue(thread_id).await;
        self.reconcile_phase_gates(thread_id).await;
        // Ensure ui_turns carry the open gate so HTTP restore shows ApprovalOptions.
        if self.thread_awaiting_human(thread_id).await {
            self.sync_pending_gate_into_ui(thread_id, false).await;
        }
        let turn_active = self.active_turn_id(thread_id).await.is_some();
        // No live turn + no human gate → finish any stuck「调用模型中」cards.
        if !turn_active {
            let awaiting = self.thread_awaiting_human(thread_id).await;
            if !awaiting {
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.ui_turns = finish_stale_ui_turns(std::mem::take(&mut t.ui_turns));
                }
                let _ = self.persist_thread(thread_id).await;
            }
        }
        let guard = self.threads.read().await;
        let t = guard.get(thread_id)?;
        let project = t.summary.project.clone();
        let queue = project
            .as_deref()
            .and_then(|p| load_audit_queue(&self.roots.projects_root, p));
        let open_gate = self.build_open_gate_dto(t, queue.is_some());
        Some(serde_json::json!({
            "threadId": thread_id,
            "project": t.summary.project,
            "messages": t.messages,
            "turns": t.ui_turns,
            "pending_audit": t.pending_audit,
            "pending_volume_sync": t.pending_volume_sync,
            "pending_setup": t.pending_setup,
            "pending_chapter_next": t.pending_chapter_next,
            "pending_audit_queue": queue,
            "turn_active": turn_active,
            // Prefer this over client-side hardcoded buttons (gates.yaml is source of truth).
            "open_gate": open_gate,
        }))
    }

    /// Build restore gate {prompt, options} from GateCatalog — no frontend phrase tables.
    fn build_open_gate_dto(&self, t: &ThreadState, queue_active: bool) -> Option<Value> {
        if let Some(a) = &t.pending_audit {
            let options = if queue_active {
                self.audit_queue_options()
            } else {
                self.audit_options()
            };
            let prompt = if queue_active {
                format!(
                    "第{}章审校未通过（审阅队列进行中），请选择：",
                    a.chapter
                )
            } else {
                format!("第{}章审校未通过，请选择如何处理：", a.chapter)
            };
            return Some(json!({
                "kind": "audit",
                "prompt": prompt,
                "options": options,
            }));
        }
        if let Some(v) = &t.pending_volume_sync {
            let options = self.gates.options("volume_sync");
            let label = if v.name.trim().is_empty() {
                format!("第{}卷", v.volume)
            } else {
                format!("第{}卷「{}」", v.volume, v.name)
            };
            return Some(json!({
                "kind": "volume_sync",
                "prompt": format!("{label}已结束，是否同步设定库？"),
                "options": options,
            }));
        }
        if let Some(s) = &t.pending_setup {
            let options = self.gates.options("setup_confirm");
            return Some(json!({
                "kind": "setup_confirm",
                "prompt": format!(
                    "《{}》总纲与卷纲已就绪，是否确认定稿并进入写章？",
                    s.project
                ),
                "options": options,
            }));
        }
        if let Some(c) = &t.pending_chapter_next {
            let options = self.gates.chapter_next_options(c.published);
            let prompt = if c.published {
                format!(
                    "第{}章已发布。请选择：继续创作；也可点「其他」说明要求。",
                    c.chapter
                )
            } else {
                format!(
                    "第{}章因硬规则未发布。请选择：修正本章；也可点「其他」说明要求。",
                    c.chapter
                )
            };
            return Some(json!({
                "kind": "chapter_next",
                "prompt": prompt,
                "options": options,
            }));
        }
        None
    }

    pub async fn save_ui_turns(&self, thread_id: &str, turns: serde_json::Value) -> Result<()> {
        {
            let mut guard = self.threads.write().await;
            if let Some(t) = guard.get_mut(thread_id) {
                let gate_open = t.pending_audit.is_some()
                    || t.pending_volume_sync.is_some()
                    || t.pending_volume_audit.is_some()
            || t.pending_setup.is_some()
            || t.pending_chapter_next.is_some()
            || load_audit_queue(
                        &self.roots.projects_root,
                        t.summary.project.as_deref().unwrap_or(""),
                    )
                    .is_some();
                // Never let a stale client timeline revive Running audit cards after a gate.
                // When any human gate is open, keep client approvals; otherwise strip.
                let mut sanitized = if t.pending_audit.is_some() {
                    sanitize_ui_turns_finish_audits(turns)
                } else if gate_open {
                    turns
                } else {
                    strip_ui_approvals(turns)
                };
                if !gate_open {
                    sanitized = strip_ui_approvals(sanitized);
                }
                // Reject weaker client snapshots (empty running / old restored-*) that
                // wipe a finished new-chapter turn after ChatHistoryReset.
                let reject = self.features.reject_weak_ui_turns()
                    && ui_turns_weaker_than(&sanitized, &t.ui_turns);
                if !reject {
                    t.ui_turns = sanitized;
                    t.summary.updated_at = Utc::now();
                }
            }
        }
        self.persist_thread(thread_id).await
    }

    /// Helper for approval-style options (Codex RequestUserInput).
    /// UI already appends a free-text「其他」button.
    pub fn audit_options(&self) -> Vec<UserInputOption> {
        self.gates.options("audit")
    }

    pub fn audit_queue_options(&self) -> Vec<UserInputOption> {
        self.gates.options("audit_queue")
    }

    pub fn volume_sync_options(&self) -> Vec<UserInputOption> {
        self.gates.options("volume_sync")
    }

    pub fn volume_audit_options(&self) -> Vec<UserInputOption> {
        self.gates.options("volume_audit")
    }

    pub fn setup_confirm_options(&self) -> Vec<UserInputOption> {
        self.gates.options("setup_confirm")
    }

    pub fn chapter_next_options(&self, published: bool) -> Vec<UserInputOption> {
        self.gates.chapter_next_options(published)
    }

    /// After chapter write/revise: published →「继续创作」; content-rule block →「修正本章」.
    async fn maybe_offer_chapter_next(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if tool_name != "continue_writing" && tool_name != "revise_chapter" {
            return Ok(false);
        }
        if data.get("blocked").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(false);
        }
        // Higher-priority gates already open — do not stack.
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_audit.is_some()
                    || t.pending_volume_sync.is_some()
                    || t.pending_volume_audit.is_some()
                    || t.pending_setup.is_some()
                {
                    return Ok(false);
                }
            }
        }
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let chapter = data
            .get("chapter")
            .and_then(|v| v.as_u64())
            .or_else(|| args.get("chapter").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;
        if project.is_empty() || chapter == 0 {
            return Ok(false);
        }
        let published = data.get("published").and_then(|v| v.as_bool()) == Some(true);
        let content_blocked =
            data.get("content_rule_blocked").and_then(|v| v.as_bool()) == Some(true);
        // Clean publish → continue next chapter. Hard-rule block → revise this chapter.
        // Other unpublished cases (consistency / P0) use the audit gate instead.
        if !published && !content_blocked {
            return Ok(false);
        }
        let options = self.chapter_next_options(published);
        if options.is_empty() {
            return Ok(false);
        }
        let prompt = if published {
            format!("第{chapter}章已发布。请选择：继续创作；也可点「其他」说明要求。")
        } else {
            format!(
                "第{chapter}章因硬规则未发布（如正文出现「第N章」元叙述或禁名）。请选择：修正本章；也可点「其他」说明要求。"
            )
        };
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_chapter_next = Some(PendingChapterNext {
                project: project.clone(),
                chapter,
                published,
            });
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &options,
            );
        }
        self.clear_queued_inputs(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt,
                options,
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    async fn parse_chapter_next_op(
        &self,
        thread_id: &str,
        text: &str,
        bound_project: Option<&str>,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_chapter_next.clone())?;
        let _ = bound_project;
        match self
            .gates
            .resolve_chapter_next(t, &pending.project, pending.chapter)?
        {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume | GateResolve::SteerInstructions { .. } => None,
        }
    }
}

fn extract_volume_number(text: &str) -> Option<u32> {
    // 「第1卷」/ 「第12卷」
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == '第' {
            let mut j = i + 1;
            let mut n: u32 = 0;
            let mut digits = false;
            while j < chars.len() && chars[j].is_ascii_digit() {
                digits = true;
                n = n
                    .saturating_mul(10)
                    .saturating_add(chars[j].to_digit(10).unwrap_or(0));
                j += 1;
            }
            if digits && j < chars.len() && (chars[j] == '卷' || chars[j] == '幕') {
                return Some(n.max(1));
            }
        }
    }
    None
}

fn parse_todo_items(v: Option<&Value>) -> Option<Vec<TodoItem>> {
    let arr = v?.as_array()?;
    let mut out = Vec::new();
    for item in arr {
        let content = item.get("content")?.as_str()?.to_string();
        let status = match item.get("status").and_then(|s| s.as_str()).unwrap_or("pending") {
            "in_progress" | "inProgress" => TodoStatus::InProgress,
            "completed" | "Completed" => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        };
        out.push(TodoItem { content, status });
    }
    Some(out)
}


/// If `continue_writing` omitted `chapter`, recover it from「第N章 / 第七章」in nearby text.
/// Returns true when args were mutated.
fn fill_continue_chapter_arg(args: &mut Value, hints: &[&str]) -> bool {
    if args
        .get("chapter")
        .and_then(|v| v.as_u64())
        .filter(|c| *c > 0)
        .is_some()
    {
        return false;
    }
    for hint in hints {
        if hint.trim().is_empty() {
            continue;
        }
        if let Some(n) = parse_chapter_number(hint) {
            args["chapter"] = json!(n);
            return true;
        }
    }
    false
}

/// Merge tool progress micro-deltas before pushing to EventTx / WebSocket.
/// Without this, audit streams fill the bounded event channel and stall the tool.
///
/// When `mirror_agent_item` is set, status heartbeats (▶ / ✓ / （调用模型…）) are also
/// appended to that agent bubble so gate turns show mid-process in the main message.
async fn coalesce_tool_progress(
    prog_rx: &mut mpsc::UnboundedReceiver<String>,
    core: &NovelxCore,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    mirror_agent_item: Option<&str>,
) {
    let mut buf = String::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(40));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    async fn flush(
        buf: &mut String,
        core: &NovelxCore,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        mirror_agent_item: Option<&str>,
    ) {
        if buf.is_empty() {
            return;
        }
        let delta = std::mem::take(buf);
        if let Some(agent_id) = mirror_agent_item {
            if let Some(mirror) = mirror_status_for_agent_bubble(&delta) {
                core.emit_to_thread(
                    thread_id,
                    EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.to_string(),
                        turn_id: turn_id.to_string(),
                        item_id: agent_id.to_string(),
                        delta: mirror,
                    },
                )
                .await;
            }
        }
        core.emit_to_thread(
            thread_id,
            EventMsg::ToolCallOutputDelta {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item_id: item_id.to_string(),
                delta,
            },
        )
        .await;
    }

    loop {
        tokio::select! {
            biased;
            msg = prog_rx.recv() => {
                match msg {
                    Some(delta) => {
                        buf.push_str(&delta);
                        if buf.len() >= 800 {
                            flush(
                                &mut buf,
                                core,
                                thread_id,
                                turn_id,
                                item_id,
                                mirror_agent_item,
                            )
                            .await;
                        }
                    }
                    None => {
                        flush(
                            &mut buf,
                            core,
                            thread_id,
                            turn_id,
                            item_id,
                            mirror_agent_item,
                        )
                        .await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                flush(
                    &mut buf,
                    core,
                    thread_id,
                    turn_id,
                    item_id,
                    mirror_agent_item,
                )
                .await;
            }
        }
    }
}

/// Compact status lines suitable for the agent bubble (skip report dumps / separators).
fn mirror_status_for_agent_bubble(delta: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in delta.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("——") || t.starts_with("## ") {
            continue;
        }
        let keep = t.starts_with('▶')
            || t.starts_with('✓')
            || t.starts_with('⏸')
            || t.starts_with('⚙')
            || t.starts_with('✕')
            || t.contains("调用模型")
            || t.contains("等待首包")
            || t.contains("正文已写入")
            || (t.starts_with('…') && t.contains("生成中"))
            || t.contains("模型返回空");
        if keep {
            lines.push(t.to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!("\n{}", lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_continue_chapter_from_user_only() {
        let mut args = json!({"project": "demo"});
        // Bare「继续」must not pick chapter from assistant prose.
        assert!(!fill_continue_chapter_arg(&mut args, &["继续"]));
        assert!(args.get("chapter").is_none());
        assert!(fill_continue_chapter_arg(&mut args, &["写第7章"]));
        assert_eq!(args["chapter"], 7);
        let p = StudioPolicies::defaults();
        assert!(!p.user_intends_new_chapter_write("继续", false));
        assert!(p.user_intends_new_chapter_write("写第7章", true));
        assert!(p.user_intends_new_chapter_write("继续创作", false));
        assert!(p.is_bare_continue("继续"));
    }

    #[tokio::test]
    async fn submit_starts_submission_runtime() {
        let root = std::env::temp_dir().join(format!("novelx_test_{}", uuid::Uuid::new_v4()));
        let projects = root.join("projects");
        let config = root.join("config");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(config.join("skills/agents")).unwrap();
        std::fs::write(config.join("llm.yaml"), "providers: {}\n").unwrap();
        // Minimal intents so NovelxCore::new loads cleanly in temp config.
        std::fs::write(
            config.join("intents.yaml"),
            include_str!("../../../config/intents.yaml"),
        )
        .unwrap();
        std::fs::write(
            config.join("gates.yaml"),
            include_str!("../../../config/gates.yaml"),
        )
        .unwrap();
        std::fs::write(
            config.join("features.yaml"),
            include_str!("../../../config/features.yaml"),
        )
        .unwrap();
        let llm = Arc::new(LlmClient::new(Default::default()));
        let core = NovelxCore::new(projects, config, llm);
        let (tid, _) = core.spawn_thread(None, true).await.unwrap();
        let sub = core
            .submit(
                &tid,
                Op::InterruptTurn {
                    thread_id: tid.clone(),
                    turn_id: None,
                },
            )
            .await
            .unwrap();
        assert!(sub.starts_with("sub_"));
        assert!(core.runtimes.read().unwrap().contains_key(&tid));
        let _ = std::fs::remove_dir_all(&root);
    }
}

fn _assert_send_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<NovelxCore>();
    assert_sync::<NovelxCore>();
    assert_send::<crate::agent::AgentHub>();
    assert_sync::<crate::agent::AgentHub>();
}
