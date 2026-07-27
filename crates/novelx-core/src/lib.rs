//! NovelX core — Codex-style Submission / Session / SubAgent runtime.

mod agent;
pub mod audit_decisions;
mod features;
mod gates;
mod intent;
mod session;
pub(crate) mod studio_next;
mod studio_next_gates;
mod tasks;
mod thread_store;
mod ui_sync;

use audit_decisions::{
    audit_fail_prompt, build_fallback_audit_decisions, decisions_to_ui_options,
    format_audit_issue_checklist, resolve_decision_pick, validate_offered_decisions,
    AuditDecisionOption,
};
use features::FeatureFlags;
use gates::{GateCatalog, GateResolve};
use intent::{parse_chapter_number, ClearHistory, IntentMatch, IntentRouter};
use novelx_harness::{
    check_draft_with, rewrite_meta_chapter_refs_with, with_issue_ids, ContentRulesConfig,
    NamingRules, StudioPolicies,
};
use ui_sync::{
    append_completion_ui_turn, attach_ui_approval, attach_ui_mutation_preview,
    finish_stale_ui_turns, keep_only_ui_turn, mark_ui_turn_complete,
    sanitize_ui_turns_finish_audits, strip_ui_approvals, ui_turns_weaker_than,
    update_ui_turn_summary, upsert_ui_agent_message, upsert_ui_tool_call,
};

use agent::{json_step_result, result_mail, AgentHub, CoreAgentRuntime, SubagentJob};
use anyhow::Result;
use chrono::Utc;
use novelx_llm::{load_llm_config, sanitize_chat_messages, ChatMessage, LlmClient};
use novelx_pipeline::{
    check_plot_write_gate_with, execute_single_agent_step, format_volume_memory_preview,
    load_volume_bounds, mark_volume_sync_skipped, project_dir, read_chapter_draft,
    reopen_volume_act, resolve_arc_outline_volume, recover_false_volume_end,
    resolve_setup_next_step, resolve_setup_phase, resolve_volume_phase, scan_impact,
    set_volume_phase, volume_has_open_plot_work, write_chapter_draft, ImpactHit, ImpactSource,
    ImpactTargetKind, PhaseEnforceFlags, PlotWriteGate, RevisionOptions, RunMode, SetupNextStep,
    SetupPhase, VolumePhase,
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
    all_tools, clear_audit_queue, dispatch, load_audit_queue, tool_output_for_ui, tool_specs,
    AuditQueueStatus,
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
    /// Hot-reloadable studio policies (`config/policies.yaml`).
    policies: Arc<std::sync::RwLock<StudioPolicies>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditGateKind {
    /// Real consistency issues → revise / accept.
    #[default]
    Content,
    /// Empty / unparseable auditor → retry only (no local-patch loop).
    Infra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingAudit {
    pub project: String,
    pub chapter: u32,
    #[serde(default)]
    pub kind: AuditGateKind,
    /// Structured issues with stable `id` (from last failed audit).
    #[serde(default)]
    pub issues: Vec<Value>,
    /// Dynamic decision card (Studio `offer_decisions` or P0 fallback).
    #[serde(default)]
    pub decision_options: Vec<AuditDecisionOption>,
    /// True after fail prepared, before decision card opened (Studio may offer first).
    #[serde(default)]
    pub awaiting_offer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingVolumeSync {
    pub project: String,
    pub volume: u32,
    #[serde(default)]
    pub name: String,
    /// Setting BLOCKER deferred while volume sync card is open.
    #[serde(default)]
    pub deferred_setting_blocker: Option<PendingSettingBlocker>,
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
    /// `need_brief` | `need_master` | `need_arc` | `need_bible` | `confirm`
    #[serde(default = "default_setup_next")]
    pub next: String,
}

fn default_setup_next() -> String {
    "confirm".into()
}

/// Volume handoff next step: design_arc_outline or design_plot(+activate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingVolumeHandoff {
    pub project: String,
    /// `awaiting_next_arc` | `awaiting_next_plot`
    pub phase: String,
    pub volume: u32,
    /// Default plot title when phase is awaiting_next_plot.
    #[serde(default)]
    pub title: String,
    /// Last failed attempt summary (injected into design_plot brief on retry).
    #[serde(default)]
    pub last_error: String,
}

/// After a chapter write finishes: continue (if published) or revise (if blocked).
/// Also used when「继续创作」hits an unpublished draft at `next_chapter` (`suggest_next`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingChapterNext {
    pub project: String,
    pub chapter: u32,
    /// Published → only「继续创作」; content-rule block → only「修正本章」.
    #[serde(default)]
    pub published: bool,
    /// Published but plot_acceptor failed → show continue + revise.
    #[serde(default)]
    pub plot_accept_open: bool,
    /// When set, this is a `draft_exists` clarify gate; value = suggested next chapter to write.
    #[serde(default)]
    pub suggest_next: Option<u32>,
    /// Concrete revise instructions for hard-rule / plot-exit gaps (overrides gate YAML default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revise_instructions: Option<String>,
}

/// Skip-ahead write blocked → offer writing `next_chapter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingChapterOrder {
    pub project: String,
    pub next_chapter: u32,
}

/// continue_writing blocked on plot lifecycle → activate planned / design next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingPlotWrite {
    pub project: String,
    /// `planned_inactive` | `need_design_plot`
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub volume: u32,
}

/// Preprocess expected-event gate: review first, or approve/skip a candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingExpectedEvent {
    pub project: String,
    /// `need_review` | `decide`
    pub kind: String,
    #[serde(default)]
    pub chapter: u32,
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub event_text: String,
}

/// Setting auditor BLOCKER — human chooses repair / accept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingSettingBlocker {
    pub project: String,
    #[serde(default)]
    pub chapter: u32,
    #[serde(default)]
    pub detail: String,
    /// After dismiss/tool: re-offer「继续创作 / 修正本章」.
    #[serde(default)]
    pub resume_chapter_next: bool,
    #[serde(default)]
    pub published: bool,
    #[serde(default)]
    pub plot_accept_open: bool,
    #[serde(default)]
    pub content_blocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revise_instructions: Option<String>,
    /// After dismiss/tool: offer volume handoff (volume-end path).
    #[serde(default)]
    pub offer_volume_handoff_after: bool,
}

/// Generic mutation confirm: apply cached tool args after user approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingMutation {
    pub mutation_id: String,
    pub mutation_kind: String,
    pub summary: String,
    pub apply_tool: String,
    pub apply_args: Value,
    #[serde(default)]
    pub preview: Value,
    /// After apply (e.g. audit steer local patch), run audit_chapter automatically.
    #[serde(default)]
    pub reaudit_after: bool,
}

/// Legacy field kept for thread JSON compatibility (superseded by pending_studio_next).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingMutationFollowup {
    pub project: String,
    #[serde(default)]
    pub apply_tool: String,
}

/// After a mutation apply: dependent artifacts that may need cascade revise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingImpact {
    pub project: String,
    pub source: Value,
    pub hits: Vec<Value>,
    #[serde(default)]
    pub summary_markdown: String,
    #[serde(default)]
    pub entity_gaps_count: usize,
    /// Tool that triggered the impact scan — used to restore setup/handoff after sync/skip.
    #[serde(default)]
    pub resume_tool: String,
    #[serde(default)]
    pub resume_args: Value,
    #[serde(default)]
    pub resume_data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ThreadState {
    pub(crate) summary: ThreadSummary,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) abort: bool,
    /// UI timeline turns (JSON) for restore after refresh.
    #[serde(default = "empty_json_array")]
    pub(crate) ui_turns: serde_json::Value,
    /// Last failed audit awaiting per-issue decision (offer_decisions / P0 fallback).
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
    /// Volume handoff: design next arc / design+activate next plot.
    #[serde(default)]
    pub(crate) pending_volume_handoff: Option<PendingVolumeHandoff>,
    /// Post-chapter choice: continue writing / revise / other.
    #[serde(default)]
    pub(crate) pending_chapter_next: Option<PendingChapterNext>,
    /// Chapter order skip blocked → write next_chapter.
    #[serde(default)]
    pub(crate) pending_chapter_order: Option<PendingChapterOrder>,
    /// Plot write gate: activate planned card / design next plot.
    #[serde(default)]
    pub(crate) pending_plot_write: Option<PendingPlotWrite>,
    /// Expected-event gate: review / approve / skip / later.
    #[serde(default)]
    pub(crate) pending_expected_event: Option<PendingExpectedEvent>,
    /// Setting auditor BLOCKER after plot/chapter pass.
    #[serde(default)]
    pub(crate) pending_setting_blocker: Option<PendingSettingBlocker>,
    /// Event ids the user chose「本次跳过」for this session.
    #[serde(default)]
    pub(crate) skipped_expected_ids: Vec<String>,
    /// Disk mutation awaiting confirm (apply / discard).
    #[serde(default)]
    pub(crate) pending_mutation: Option<PendingMutation>,
    /// Legacy — prefer pending_studio_next / awaiting_studio_next.
    #[serde(default)]
    pub(crate) pending_mutation_followup: Option<PendingMutationFollowup>,
    /// Model-offered situational next-step card (`offer_decisions kind=studio_next`).
    #[serde(default)]
    pub(crate) pending_studio_next: Option<studio_next::PendingStudioNext>,
    /// Nudge in flight asking Studio to offer studio_next (not a human gate yet).
    #[serde(default)]
    pub(crate) awaiting_studio_next: Option<studio_next::AwaitingStudioNext>,
    /// Mid outline rewrite (总纲/卷纲) — bare「继续」must not jump to continue_writing.
    #[serde(default)]
    pub(crate) outline_rewrite_active: bool,
    /// Post-apply impact cascade awaiting sync / skip.
    #[serde(default)]
    pub(crate) pending_impact: Option<PendingImpact>,
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
        let policies = Arc::new(std::sync::RwLock::new(StudioPolicies::load(&config_root)));
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
                    skipped_expected_ids: Vec::new(),
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
        // Best-effort sync fill of skipped expected ids for continue_writing soft-block.
        if let Ok(guard) = self.threads.try_read() {
            if let Some(t) = guard.get(thread_id) {
                ctx.skipped_expected_ids = t.skipped_expected_ids.clone();
            }
        }
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
        // Deprecated situational pendings (plot_write / expected / setting_blocker /
        // volume_audit / mutation_followup) are cleared on load and must not block.
        t.pending_audit.is_some()
            || t.pending_volume_sync.is_some()
            || t.pending_setup.is_some()
            || t.pending_volume_handoff.is_some()
            || t.pending_chapter_next.is_some()
            || t.pending_chapter_order.is_some()
            || t.pending_mutation.is_some()
            || t.pending_studio_next.is_some()
            || t.pending_impact.is_some()
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

    /// Queue a synthetic user message for RegularTask to drain after this turn ends.
    async fn enqueue_pending_user_text(&self, thread_id: &str, text: &str) {
        let queue = {
            let Ok(guard) = self.runtimes.read() else {
                return;
            };
            guard.get(thread_id).map(|r| r.input_queue.clone())
        };
        if let Some(q) = queue {
            q.extend_pending_input(vec![crate::session::TurnInput::UserInput {
                content: vec![UserInput::text(text.to_string())],
            }])
            .await;
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

    fn with_policies<R>(&self, f: impl FnOnce(&StudioPolicies) -> R) -> R {
        let guard = self.policies.read().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// Reload `config/policies.yaml` into the in-memory cache (after Web PUT).
    pub fn reload_policies(&self) {
        let loaded = StudioPolicies::load(&self.roots.config_root);
        *self.policies.write().unwrap_or_else(|e| e.into_inner()) = loaded;
    }

    /// Reload `config/llm.yaml` (+ process env API key) into the shared `LlmClient`.
    pub fn reload_llm(&self) -> Result<(), String> {
        let path = self.roots.config_root.join("llm.yaml");
        let cfg = load_llm_config(&path).map_err(|e| format!("加载 llm.yaml 失败：{e}"))?;
        self.roots.llm.reload(cfg);
        Ok(())
    }

    pub fn config_root(&self) -> &std::path::Path {
        &self.roots.config_root
    }

    /// Repository root (parent of `config/`), where `.env` lives.
    pub fn repo_root(&self) -> PathBuf {
        self.roots
            .config_root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn llm(&self) -> &Arc<LlmClient> {
        &self.roots.llm
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
                            pending_volume_handoff: None,
                            pending_chapter_next: None,
                            pending_chapter_order: None,
                            pending_plot_write: None,
                            pending_expected_event: None,
                            pending_setting_blocker: None,
                            skipped_expected_ids: Vec::new(),
                            pending_mutation: None,
                            pending_mutation_followup: None,
                            pending_studio_next: None,
                            awaiting_studio_next: None,
                            outline_rewrite_active: false,
                            pending_impact: None,
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
        // Cap ~30s — a stuck RegularTask must not block HTTP/local sinks forever.
        for _ in 0..600 {
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

        // Deterministic: mutation confirm (apply / discard) — before other gates.
        if let Some((tool_name, args)) = self.parse_mutation_confirm_op(&thread_id, &text).await {
            tracing::info!(%tool_name, args = %args, "studio direct mutation confirm");
            let pending_snap = {
                let guard = self.threads.read().await;
                guard
                    .get(&thread_id)
                    .and_then(|th| th.pending_mutation.clone())
            };
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

            let mut clear_pending = false;
            let mut reoffer_pending: Option<PendingMutation> = None;
            let mut reoffer_setup_after_discard = false;
            let summary = if tool_name == "__discard_mutation" {
                clear_pending = true;
                reoffer_setup_after_discard = pending_snap
                    .as_ref()
                    .map(|p| {
                        matches!(
                            p.apply_tool.as_str(),
                            "design_master_outline"
                                | "design_arc_outline"
                                | "upsert_setting"
                                | "supplement_setting"
                        )
                    })
                    .unwrap_or(false);
                "已放弃此次修改，磁盘未变更。".to_string()
            } else if let Some(pending) = pending_snap.clone() {
                let mid = args
                    .get("mutation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if mid != pending.mutation_id {
                    // Keep gate so the user can retry; do not strip approvals.
                    reoffer_pending = Some(pending.clone());
                    format!(
                        "确认失败：mutation_id 不匹配（期望 {}）。请重新点「应用修改」。",
                        pending.mutation_id
                    )
                } else {
                    clear_pending = true;
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.pending_mutation = None;
                        t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
                    }
                    let intro = format!("已确认，正在应用：{}", pending.summary);
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
                    let (output, data) = self
                        .run_one_tool(
                            &thread_id,
                            &turn_id,
                            &pending.apply_tool,
                            &pending.apply_args.to_string(),
                        )
                        .await?;
                    let nested = data.get("needs_confirm").and_then(|v| v.as_bool())
                        == Some(true);
                    if nested {
                        let _ = self
                            .maybe_offer_mutation_confirm(
                                &thread_id,
                                &turn_id,
                                &pending.apply_tool,
                                &pending.apply_args,
                                &data,
                            )
                            .await?;
                        // Nested preview still needs the tool text in the bubble.
                        format!("{intro}\n\n{output}")
                    } else if pending.reaudit_after
                        && self.features.auto_reaudit_after_steer()
                        && (pending.apply_tool == "revise_chapter"
                            || pending.apply_tool == "steer_run")
                    {
                        let project = pending
                            .apply_args
                            .get("project")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let chapter = pending
                            .apply_args
                            .get("chapter")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        // Tool card already shows write result; bubble keeps ack + reaudit status.
                        let mut body = mutation_apply_agent_summary(&intro, &output, &data);
                        if !project.is_empty() && chapter > 0 {
                            self.emit_to_thread(
                                &thread_id,
                                EventMsg::AgentMessageContentDelta {
                                    thread_id: thread_id.clone(),
                                    turn_id: turn_id.clone(),
                                    item_id: agent_item_id.clone(),
                                    delta: format!("\n\n修订已落盘，正在复审第{chapter}章…"),
                                },
                            )
                            .await;
                            let audit_args = json!({
                                "project": project,
                                "chapter": chapter,
                                "verify_previous": true,
                            });
                            let (_audit_out, audit_data) = self
                                .run_one_tool_mirrored(
                                    &thread_id,
                                    &turn_id,
                                    "audit_chapter",
                                    &audit_args.to_string(),
                                    Some(&agent_item_id),
                                )
                                .await?;
                            let asked = self
                                .maybe_offer_audit_fix(
                                    &thread_id,
                                    &turn_id,
                                    "audit_chapter",
                                    &audit_args,
                                    &audit_data,
                                )
                                .await?;
                            if asked {
                                body.push_str("\n\n复审未通过，请按下方选项继续。");
                            } else if audit_data
                                .get("consistency_passed")
                                .and_then(|v| v.as_bool())
                                == Some(true)
                            {
                                // Pass must open the next human gate (继续创作 / 卷同步…),
                                // not dead-end with「本轮已正常结束」.
                                let offered = self
                                    .maybe_offer_after_audit_pass(
                                        &thread_id,
                                        &turn_id,
                                        "audit_chapter",
                                        &audit_args,
                                        &audit_data,
                                    )
                                    .await?;
                                if offered {
                                    body.push_str("\n\n复审通过，请选择下一步。");
                                } else {
                                    body.push_str(
                                        "\n\n复审通过。可说「继续创作」写下一章。",
                                    );
                                }
                            }
                        }
                        body
                    } else {
                        let mut body = mutation_apply_agent_summary(&intro, &output, &data);
                        // Impact cascade owns the next human gate when hits exist.
                        let offered_impact = self
                            .maybe_offer_impact_cascade(
                                &thread_id,
                                &turn_id,
                                &pending.apply_tool,
                                &pending.apply_args,
                                &data,
                            )
                            .await?;
                        if offered_impact {
                            body.push_str("\n\n已扫描依赖面，请选择是否自动同步修正受影响内容。");
                        } else {
                            // After applying design_* from handoff / setup, advance next gate.
                            let asked_handoff = self
                                .maybe_offer_volume_handoff(
                                    &thread_id,
                                    &turn_id,
                                    &pending.apply_tool,
                                    &pending.apply_args,
                                    &data,
                                )
                                .await?;
                            let asked_setup = if asked_handoff {
                                false
                            } else {
                                self.maybe_offer_setup_confirm(
                                    &thread_id,
                                    &turn_id,
                                    &pending.apply_tool,
                                    &pending.apply_args,
                                    &data,
                                )
                                .await?
                            };
                            let asked_next = if asked_handoff || asked_setup {
                                false
                            } else {
                                self.maybe_offer_chapter_next(
                                    &thread_id,
                                    &turn_id,
                                    &pending.apply_tool,
                                    &pending.apply_args,
                                    &data,
                                )
                                .await?
                            };
                            // Ready projects won't get setup gates; keep multi-step plans alive.
                            let asked_followup = if asked_handoff || asked_setup || asked_next {
                                false
                            } else {
                                self.maybe_offer_mutation_followup(
                                    &thread_id,
                                    &turn_id,
                                    &pending.apply_tool,
                                    &pending.apply_args,
                                    &data,
                                )
                                .await?
                            };
                            if asked_followup {
                                body.push_str(
                                    "\n\n正在请 Studio 给出下一步审批卡…",
                                );
                            }
                            if matches!(
                                pending.apply_tool.as_str(),
                                "design_master_outline" | "design_arc_outline"
                            ) && data.get("blocked").and_then(|v| v.as_bool()) != Some(true)
                            {
                                if let Some(t) =
                                    self.threads.write().await.get_mut(&thread_id)
                                {
                                    t.outline_rewrite_active = true;
                                }
                            }
                        }
                        body
                    }
                }
            } else {
                "没有待确认的修改。".into()
            };

            let chapter_gate_open = {
                let guard = self.threads.read().await;
                guard
                    .get(&thread_id)
                    .map(|th| th.pending_chapter_next.is_some())
                    .unwrap_or(false)
            };
            // Clear applied mutation before probing open gates (pending_mutation would false-positive).
            if clear_pending {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.pending_mutation = None;
                }
            }
            let awaiting_gate = reoffer_pending.is_some()
                || self.thread_awaiting_human(&thread_id).await;
            if clear_pending && !awaiting_gate {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    // Keep chapter_next / setup / other gates that maybe_offer_* just attached.
                    t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
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
                t.ui_turns = append_completion_ui_turn(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &summary,
                    awaiting_gate,
                );
            }
            // Re-attach chapter_next approval after append_completion (it clears approval).
            if chapter_gate_open {
                if let Some(pending) = {
                    let guard = self.threads.read().await;
                    guard
                        .get(&thread_id)
                        .and_then(|t| t.pending_chapter_next.clone())
                } {
                    let options = self
                        .chapter_next_options(pending.published, pending.plot_accept_open);
                    let prompt = if pending.published {
                        chapter_next_published_prompt(
                            pending.chapter,
                            pending.plot_accept_open,
                        )
                    } else {
                        chapter_next_hard_rule_prompt(pending.chapter, &summary)
                    };
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.ui_turns = attach_ui_approval(
                            std::mem::take(&mut t.ui_turns),
                            &turn_id,
                            &prompt,
                            &options,
                        );
                    }
                    self.emit_to_thread(
                        &thread_id,
                        EventMsg::RequestUserInput {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            prompt,
                            options,
                        },
                    )
                    .await;
                }
            }
            self.emit_to_thread(
                &thread_id,
                EventMsg::ItemCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item: TurnItem::AgentMessage {
                        id: agent_item_id,
                        text: summary.clone(),
                        status: ItemStatus::Completed,
                    },
                },
            )
            .await;
            if let Some(pending) = reoffer_pending {
                let options = self.gates.mutation_confirm_options();
                let prompt = format!("待确认：{}", pending.summary);
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.pending_mutation = Some(pending.clone());
                    t.ui_turns = attach_ui_approval(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &prompt,
                        &options,
                    );
                    t.ui_turns = attach_ui_mutation_preview(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &pending.preview,
                        pending
                            .preview
                            .get("diffs")
                            .cloned()
                            .unwrap_or(json!([])),
                    );
                }
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::RequestUserInput {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        prompt,
                        options,
                    },
                )
                .await;
            } else if reoffer_setup_after_discard {
                if let Some(project) = pending_snap
                    .as_ref()
                    .and_then(|p| p.apply_args.get("project"))
                    .and_then(|v| v.as_str())
                {
                    let dir = project_dir(&self.roots.projects_root, project);
                    if let Some(next) = resolve_setup_next_step(&dir) {
                        let _ = self
                            .offer_setup_gate(&thread_id, &turn_id, project, next)
                            .await?;
                    }
                }
            }
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

        // Situational next-step card from offer_decisions(kind=studio_next) or soft fallback.
        if let Some(pick) = self.parse_studio_next_op(&thread_id, &text).await {
            tracing::info!(option = %pick.opt.id, "studio_next gate");
            self.run_studio_next_pick(&thread_id, &turn_id, pick).await?;
            return Ok(());
        }

        // Deterministic: impact cascade (sync / skip) after mutation apply.
        if let Some(op) = self.parse_impact_confirm_op(&thread_id, &text).await {
            tracing::info!(?op, "studio direct impact confirm");
            let pending_snap = {
                let mut guard = self.threads.write().await;
                let snap = guard
                    .get(&thread_id)
                    .and_then(|th| th.pending_impact.clone());
                if let Some(th) = guard.get_mut(&thread_id) {
                    th.pending_impact = None;
                    th.ui_turns = strip_ui_approvals(std::mem::take(&mut th.ui_turns));
                }
                snap
            };
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
            let (summary, resume) = match (op.as_str(), pending_snap) {
                ("sync_impact", Some(pending)) => {
                    let intro = format!(
                        "已确认，正在同步修正 {} 处依赖位点…",
                        pending.hits.len()
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
                    let cascade_out = self
                        .run_impact_cascade(&thread_id, &turn_id, &pending)
                        .await?;
                    (
                        format!("{intro}\n\n{cascade_out}"),
                        Some(pending),
                    )
                }
                ("skip_impact", Some(pending)) => (
                    "已跳过依赖同步。源对象已落盘；可稍后手动修订受影响正文/章纲。".into(),
                    Some(pending),
                ),
                _ => ("没有待处理的影响同步。".into(), None),
            };
            let mut gate_open = false;
            if let Some(pending) = resume.as_ref() {
                if !pending.resume_tool.is_empty() {
                    let asked_handoff = self
                        .maybe_offer_volume_handoff(
                            &thread_id,
                            &turn_id,
                            &pending.resume_tool,
                            &pending.resume_args,
                            &pending.resume_data,
                        )
                        .await?;
                    let asked_setup = if asked_handoff {
                        false
                    } else {
                        self.maybe_offer_setup_confirm(
                            &thread_id,
                            &turn_id,
                            &pending.resume_tool,
                            &pending.resume_args,
                            &pending.resume_data,
                        )
                        .await?
                    };
                    let asked_next = if asked_handoff || asked_setup {
                        false
                    } else {
                        self.maybe_offer_chapter_next(
                            &thread_id,
                            &turn_id,
                            &pending.resume_tool,
                            &pending.resume_args,
                            &pending.resume_data,
                        )
                        .await?
                    };
                    let asked_followup = if asked_handoff || asked_setup || asked_next {
                        false
                    } else {
                        self.maybe_offer_mutation_followup(
                            &thread_id,
                            &turn_id,
                            &pending.resume_tool,
                            &pending.resume_args,
                            &pending.resume_data,
                        )
                        .await?
                    };
                    gate_open = asked_handoff || asked_setup || asked_next || asked_followup;
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
                t.ui_turns = append_completion_ui_turn(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &summary,
                    gate_open,
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

        // Deterministic: setting auditor BLOCKER gate.
        if let Some((tool_name, args, pending_sb)) =
            self.parse_setting_blocker_op(&thread_id, &text).await
        {
            if tool_name == "__dismiss_setting_blocker" {
                let summary =
                    "已接受设定 BLOCKER。可稍后 list_entities / audit_setting / design_entity。"
                        .to_string();
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
                        false,
                    );
                }
                let _ = self
                    .finish_setting_blocker_followups(&thread_id, &turn_id, &pending_sb)
                    .await?;
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
            tracing::info!(%tool_name, args = %args, "studio direct setting_blocker gate");
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
            let (output, _data) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            let summary = output;
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
                        text: summary,
                        status: ItemStatus::Completed,
                    },
                },
            )
            .await;
            let _ = self
                .finish_setting_blocker_followups(&thread_id, &turn_id, &pending_sb)
                .await?;
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

        // Deterministic: expected-event review / incorporate gate.
        if let Some(op) = self.parse_expected_event_op(&thread_id, &text).await {
            match op {
                ExpectedEventGateOp::Dismiss => {
                    let summary = "已关闭预期决策卡。可稍后 list_expected_events / review_expected_events。"
                        .to_string();
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
                            false,
                        );
                    }
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
                ExpectedEventGateOp::Tool { tool_name, args } => {
                    tracing::info!(%tool_name, args = %args, "studio direct expected_event gate");
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.pending_expected_event = None;
                        t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
                    }
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
                    let (output, data) = self
                        .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                        .await?;
                    self.apply_expected_event_tool_side_effects(&thread_id, &tool_name, &data)
                        .await;
                    let _ = self
                        .maybe_offer_expected_event(&thread_id, &turn_id, &tool_name, &args, &data)
                        .await?;
                    let summary = output;
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
            }
        }

        // Deterministic: planned/need plot write gate.
        if let Some(action) = self.parse_plot_write_op(&thread_id, &text).await {
            self.run_plot_write_gate_action(&thread_id, &turn_id, action)
                .await?;
            return Ok(());
        }

        // Deterministic: chapter order (write next_chapter).
        if let Some((tool_name, args)) = self.parse_chapter_order_op(&thread_id, &text).await {
            tracing::info!(%tool_name, args = %args, "studio direct chapter_order");
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_chapter_order = None;
                t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
            }
            // Gate already confirmed intent — skip a second mutation card for continue/batch.
            let args = if tool_name == "continue_writing"
                || tool_name == "continue_writing_batch"
            {
                novelx_tools::with_confirm_skip(args)
            } else {
                args
            };
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
                "按顺序创作第{}章…",
                args.get("chapter").and_then(|v| v.as_u64()).unwrap_or(0)
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
            let (output, data) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            let mut summary = format!("{intro}\n\n{output}");
            let asked_mut = self
                .maybe_offer_mutation_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            let asked_impact = if asked_mut {
                false
            } else {
                self.maybe_offer_impact_cascade(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            if !asked_mut && !asked_impact {
                let _ = self
                    .maybe_offer_chapter_order(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?;
            }
            if asked_mut {
                summary = format!("{summary}\n\n请选择：应用修改 / 放弃。");
            } else if asked_impact {
                summary = format!("{summary}\n\n已扫描依赖面，请选择是否自动同步修正。");
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
            let summary = format!("{intro}\n\n{output}");
            let asked_audit = self
                .maybe_offer_audit_fix(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
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
            // 「继续创作」但 next_chapter 已有未发布草稿 → 弹选项，勿空跑 continue_writing。
            if tool_name == "continue_writing"
                && args.get("chapter").and_then(|v| v.as_u64()).is_none()
            {
                let project = args
                    .get("project")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| bound_project.as_deref().unwrap_or(""))
                    .to_string();
                if !project.is_empty()
                    && self.next_chapter_draft_chars(&project) >= self.with_policies(|p| p.draft_min_chars())
                {
                    let agent_item_id = new_id("item");
                    let (chapter, chars, suggest) = self.draft_exists_meta(&project);
                    let summary = format!(
                        "第{chapter}章已有正文（约 {chars} 字），但尚未发布（next_chapter={chapter}）。\n\
                         「继续创作」有歧义，请选择下一步。"
                    );
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
                            delta: summary.clone(),
                        },
                    )
                    .await;
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.messages.push(ChatMessage {
                            role: "assistant".into(),
                            content: summary.clone(),
                            tool_call_id: None,
                            tool_calls: None,
                            ..Default::default()
                        });
                        t.ui_turns = upsert_ui_agent_message(
                            std::mem::take(&mut t.ui_turns),
                            &turn_id,
                            &agent_item_id,
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
                    let _ = self
                        .offer_draft_exists_gate(&thread_id, &turn_id, &project, chapter, suggest)
                        .await?;
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
            // Meta「第N章」硬规则：先确定性改稿，再 audit 复审发布，避免空泛 revise 反复失败。
            let (tool_name, args, autofix_note) = if tool_name == "revise_chapter" {
                let project = args
                    .get("project")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| bound_project.as_deref().unwrap_or(""))
                    .to_string();
                let chapter = args.get("chapter").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                if let Some(note) = try_autofix_meta_chapter_refs(
                    &self.roots.projects_root,
                    &self.roots.config_root,
                    &project,
                    chapter,
                ) {
                    (
                        "audit_chapter".to_string(),
                        json!({ "project": project, "chapter": chapter }),
                        Some(note),
                    )
                } else {
                    (tool_name, args, None)
                }
            } else {
                (tool_name, args, None)
            };
            // Gate intent for continue / batch is already confirmed; revise still needs diff confirm.
            let args = if tool_name == "continue_writing"
                || tool_name == "continue_writing_batch"
            {
                novelx_tools::with_confirm_skip(args)
            } else {
                args
            };
            let intro = if let Some(note) = &autofix_note {
                format!(
                    "{note}\n\n已识别指令，正在对《{}》执行 {tool_name}…",
                    bound_project.as_deref().unwrap_or("?")
                )
            } else {
                format!(
                    "已识别指令，正在对《{}》执行 {tool_name}…",
                    bound_project.as_deref().unwrap_or("?")
                )
            };
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
            let asked_mutation = self
                .maybe_offer_mutation_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            let asked_impact = if asked_mutation {
                false
            } else {
                self.maybe_offer_impact_cascade(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_order = if asked_mutation || asked_impact {
                false
            } else {
                self.maybe_offer_chapter_order(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_plot = if asked_mutation || asked_impact || asked_order {
                false
            } else {
                self.maybe_offer_plot_write(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_expected =
                if asked_mutation || asked_impact || asked_order || asked_plot {
                    false
                } else {
                    self.maybe_offer_expected_event(
                        &thread_id,
                        &turn_id,
                        &tool_name,
                        &args,
                        &data,
                    )
                    .await?
                };
            let asked_setup =
                if asked_mutation || asked_impact || asked_order || asked_plot || asked_expected {
                    false
                } else {
                    self.maybe_offer_setup_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                        .await?
                };
            let asked_draft = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
            {
                false
            } else {
                self.maybe_offer_draft_exists(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_audit = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
            {
                false
            } else {
                self.maybe_offer_audit_fix(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_volume = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_audit
            {
                false
            } else {
                self.maybe_offer_volume_sync(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_handoff = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_audit
                || asked_volume
            {
                false
            } else {
                self.maybe_offer_volume_handoff(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_setting = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_audit
                || asked_volume
                || asked_handoff
            {
                false
            } else {
                self.maybe_offer_setting_blocker(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_next = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_audit
                || asked_volume
                || asked_handoff
                || asked_setting
            {
                false
            } else {
                self.maybe_offer_chapter_next(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_followup = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_audit
                || asked_volume
                || asked_handoff
                || asked_setting
                || asked_next
            {
                false
            } else {
                self.maybe_offer_mutation_followup(
                    &thread_id,
                    &turn_id,
                    &tool_name,
                    &args,
                    &data,
                )
                .await?
            };
            // Options live on ApprovalOptions — avoid duplicating the menu in the bubble.
            if asked_mutation {
                summary = format!(
                    "{}\n\n请选择：应用修改 / 放弃。",
                    data.get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("待确认修改")
                );
            } else if asked_impact {
                summary = format!("{summary}\n\n已扫描依赖面，请选择是否自动同步修正。");
            } else if asked_followup {
                summary = format!(
                    "{summary}\n\n正在请 Studio 给出下一步审批卡…"
                );
            } else if asked_order {
                let next = data
                    .get("next_chapter")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                summary = format!("不能跳章。请先写第{next}章。");
            } else if asked_expected {
                summary = data
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        "预处理预期待决策：请在审批卡选择纳入 / 跳过 / 稍后。".into()
                    });
            } else if asked_plot {
                let title = data
                    .get("plot_title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("剧情卡");
                summary = if data.get("reason").and_then(|v| v.as_str())
                    == Some("need_design_plot")
                {
                    "⛔ 写章已拦截：需要先设计并激活剧情卡。请在下方选择下一步。".into()
                } else {
                    format!("⛔ 写章已拦截：剧情卡「{title}」尚未激活。请在下方选择下一步。")
                };
            } else if asked_setup {
                let hint = self
                    .threads
                    .read()
                    .await
                    .get(&thread_id)
                    .and_then(|t| t.pending_setup.as_ref())
                    .and_then(|p| SetupNextStep::parse(&p.next))
                    .map(|n| n.summary_hint())
                    .unwrap_or("请选择下一步。");
                summary = format!("{summary}\n\n{hint}");
            } else if asked_draft {
                summary = format!(
                    "第{}章已有未发布正文，请选择：审校本章 / 修订本章 / 写下一章。",
                    data.get("chapter").and_then(|v| v.as_u64()).unwrap_or(0)
                );
            } else if asked_volume {
                summary.push_str("\n\n本卷已结束 — 请选择：同步设定库 / 跳过。");
            } else if asked_handoff {
                let phase = data
                    .get("volume_phase")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                summary = match phase {
                    "awaiting_sync" => {
                        format!("{summary}\n\n请选择：同步设定库 / 跳过。")
                    }
                    "awaiting_next_plot" => {
                        format!("{summary}\n\n请选择：设计并激活剧情卡。")
                    }
                    _ => format!("{summary}\n\n请选择：设计下卷卷纲。"),
                };
            } else if asked_setting {
                summary.push_str(
                    "\n\n设定审计 BLOCKER — 请选择：补全/修订设定卡 / 再跑设定审计 / 接受并继续。",
                );
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
        // Bare digits that failed parse_chapter_next (out of range) must NOT clear the gate —
        // that used to drop「修正本章」and send「1」into stale-audit / LLM writing paths.
        if let Some(pending_cn) = self
            .threads
            .read()
            .await
            .get(&thread_id)
            .and_then(|t| t.pending_chapter_next.clone())
        {
            let bare_index = {
                let t = text.trim();
                !t.is_empty()
                    && t.len() <= 2
                    && t.chars().all(|c| c.is_ascii_digit())
            };
            if bare_index {
                let hint = if pending_cn.suggest_next.is_some() {
                    "请点击「审校本章 / 修订本章 / 写下一章」，或输入该按钮原文；序号请与上方选项一致。"
                } else if pending_cn.published {
                    "请点击「继续创作」，或输入该按钮原文；序号请与上方选项一致。"
                } else {
                    "请点击「修正本章」，或输入该按钮原文；序号请与上方选项一致。"
                };
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
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: hint.into(),
                        tool_call_id: None,
                        tool_calls: None,
                        ..Default::default()
                    });
                    t.ui_turns = append_completion_ui_turn(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        hint,
                        true,
                    );
                }
                self.sync_pending_gate_into_ui(&thread_id, true).await;
                self.emit_to_thread(
                    &thread_id,
                    EventMsg::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: TurnItem::AgentMessage {
                            id: agent_item_id,
                            text: hint.into(),
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
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_chapter_next = None;
                t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
            }
            let _ = self.persist_thread(&thread_id).await;
        }

        // Deterministic: volume handoff next step (design arc / design+activate plot).
        if let Some((tool_name, mut args)) = self
            .parse_volume_handoff_op(&thread_id, &text)
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct volume handoff (skip LLM routing)");
            // Snapshot + clear before work so double-clicks cannot start two handoffs.
            let pending_snap = {
                let mut guard = self.threads.write().await;
                let t = guard.get_mut(&thread_id);
                let snap = t.as_ref().and_then(|th| th.pending_volume_handoff.clone());
                if let Some(th) = t {
                    th.pending_volume_handoff = None;
                    th.ui_turns = strip_ui_approvals(std::mem::take(&mut th.ui_turns));
                }
                snap
            };
            self.clear_queued_inputs(&thread_id).await;
            // Retry after setting-audit BLOCKER: steer the next card away from the conflict.
            if tool_name == "design_plot" {
                if let Some(err) = pending_snap
                    .as_ref()
                    .map(|p| p.last_error.trim())
                    .filter(|s| !s.is_empty())
                {
                    let hint = format!(
                        "上次设定审计未通过，请避开冲突后重写开局卡。摘要：{}",
                        err.chars().take(400).collect::<String>()
                    );
                    if let Some(obj) = args.as_object_mut() {
                        let prev = obj
                            .get("brief")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let brief = if prev.is_empty() {
                            hint
                        } else {
                            format!("{prev}\n{hint}")
                        };
                        obj.insert("brief".into(), json!(brief));
                    }
                }
            }
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
            let intro = if tool_name == "design_arc_outline" {
                "已收到选择，正在设计下卷卷纲…"
            } else {
                "已收到选择，正在设计并激活开局剧情卡…"
            };
            self.emit_to_thread(
                &thread_id,
                EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.into(),
                },
            )
            .await;
            let (output, data) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            // Content preview awaiting confirm is success for this step — not a handoff failure.
            let needs_confirm =
                data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true);
            if needs_confirm {
                let summary = format!("{intro}\n\n{output}\n\n请选择：应用修改 / 放弃。");
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    // Keep handoff pending until mutation is applied.
                    if let Some(pending) = pending_snap.clone() {
                        t.pending_volume_handoff = Some(pending);
                    }
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
                        true,
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
                let _ = self
                    .maybe_offer_mutation_confirm(
                        &thread_id,
                        &turn_id,
                        &tool_name,
                        &args,
                        &data,
                    )
                    .await?;
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
            let blocked = data.get("blocked").and_then(|v| v.as_bool()) == Some(true);
            let activated = data.get("activated").and_then(|v| v.as_bool()) == Some(true);
            let handoff_ok = !blocked
                && (tool_name != "design_plot" || activated || data.get("path").is_some());
            let mut summary = format!("{intro}\n\n{output}");
            if tool_name == "design_plot" && activated {
                summary.push_str("\n\n剧情卡已激活，可 continue_writing 续写。");
            }
            if !handoff_ok {
                // Failure used to clear the gate while disk stayed in handoff phase —
                // refresh / reconcile then re-offered the same button (felt like multi-trigger).
                let err = data
                    .get("audit_summary")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("schema_error").and_then(|v| v.as_str()))
                    .unwrap_or(output.as_str());
                let err_short = err.chars().take(200).collect::<String>();
                summary = format!(
                    "{intro}\n\n未成功：{err_short}\n\n卷间交接未完成，请按下方选项重试。"
                );
                if let Some(mut pending) = pending_snap.clone() {
                    pending.last_error = err_short.clone();
                    let phase = VolumePhase::parse(&pending.phase)
                        .unwrap_or(VolumePhase::AwaitingNextPlot);
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.pending_volume_handoff = Some(pending.clone());
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
                    let _ = self
                        .offer_volume_handoff_gate(
                            &thread_id,
                            &turn_id,
                            &pending.project,
                            phase,
                            Some(&err_short),
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
                        text: summary.clone(),
                        status: ItemStatus::Completed,
                    },
                },
            )
            .await;
            let asked = self
                .maybe_offer_volume_handoff(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            if asked {
                let phase = data
                    .get("volume_phase")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                summary.push_str(match phase {
                    "awaiting_next_plot" => "\n\n请选择：设计并激活剧情卡。",
                    "awaiting_next_arc" => "\n\n请选择：设计下卷卷纲。",
                    _ => "\n\n请选择下一步。",
                });
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.ui_turns = update_ui_turn_summary(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        &summary,
                    );
                }
            }
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

        // Deterministic: setup progress / confirm (before volume sync).
        if let Some((tool_name, args)) = self
            .parse_setup_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct setup gate (skip LLM routing)");
            let pending_snap = {
                let mut guard = self.threads.write().await;
                let t = guard.get_mut(&thread_id);
                let snap = t.as_ref().and_then(|th| th.pending_setup.clone());
                if let Some(th) = t {
                    th.pending_setup = None;
                    th.ui_turns = strip_ui_approvals(std::mem::take(&mut th.ui_turns));
                }
                snap
            };
            if tool_name == "__dismiss_setup" {
                let summary = "已取消定稿推进。需要时再说「继续创作」或点对应步骤。".to_string();
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
            let intro = match tool_name.as_str() {
                "design_master_outline" => "已收到选择，正在生成总纲…",
                "design_arc_outline" => "已收到选择，正在生成卷纲…",
                "upsert_setting" | "supplement_setting" => "已收到选择，正在生成世界观 Bible…",
                "lock_brief" => "已收到灵感，正在锁定 brief…",
                "confirm_setup" => "已收到选择，正在处理定稿…",
                "get_project_status" => "正在查看项目状态…",
                _ => "已收到选择，正在推进定稿…",
            };
            self.emit_to_thread(
                &thread_id,
                EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    item_id: agent_item_id.clone(),
                    delta: intro.into(),
                },
            )
            .await;
            let (output, data) = self
                .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                .await?;
            let needs_confirm =
                data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true);
            if needs_confirm {
                // Mutation owns the turn — do NOT restore pending_setup here.
                // Restoring it used to make maybe_offer_mutation_confirm bail out
                // (it refused to stack over pending_setup), so the UI had no apply card
                // while the bubble still said「应用修改」.
                let asked_mut = self
                    .maybe_offer_mutation_confirm(
                        &thread_id,
                        &turn_id,
                        &tool_name,
                        &args,
                        &data,
                    )
                    .await?;
                let summary = if asked_mut {
                    format!("{intro}\n\n{output}\n\n请选择：应用修改 / 放弃。")
                } else {
                    format!("{intro}\n\n{output}\n\n预览未挂上确认卡；请再说一次「生成总纲」或「继续」。")
                };
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    // Keep setup snap only if mutation card failed, so user can retry step.
                    if !asked_mut {
                        if let Some(pending) = pending_snap.clone() {
                            t.pending_setup = Some(pending);
                        }
                    }
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
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let mut summary = format!("{intro}\n\n{output}");
            if tool_name == "confirm_setup" && action == "approve" {
                summary.push_str(
                    "\n\n下一步：design_plot → update_plot(in_progress) → continue_writing。",
                );
            }
            // Direct apply (confirm off / confirm_skip): impact before setup resume.
            let asked_impact = self
                .maybe_offer_impact_cascade(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            let asked = if asked_impact {
                false
            } else {
                self.maybe_offer_setup_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_followup = if asked_impact || asked {
                false
            } else {
                self.maybe_offer_mutation_followup(
                    &thread_id,
                    &turn_id,
                    &tool_name,
                    &args,
                    &data,
                )
                .await?
            };
            if asked_impact {
                summary = format!("{summary}\n\n已扫描依赖面，请选择是否自动同步修正。");
            } else if asked {
                let hint = self
                    .threads
                    .read()
                    .await
                    .get(&thread_id)
                    .and_then(|t| t.pending_setup.as_ref())
                    .and_then(|p| SetupNextStep::parse(&p.next))
                    .map(|n| n.summary_hint())
                    .unwrap_or("请选择下一步。");
                summary = format!("{summary}\n\n{hint}");
            } else if asked_followup {
                summary = format!(
                    "{summary}\n\n正在请 Studio 给出下一步审批卡…"
                );
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

        // Deterministic: volume-end sync / skip (before audit steer).
        if let Some((tool_name, args)) = self
            .parse_volume_sync_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct volume sync (skip LLM routing)");
            // Snapshot + clear gate before work so double-clicks cannot start two syncs.
            let pending_snap = {
                let mut guard = self.threads.write().await;
                let t = guard.get_mut(&thread_id);
                let snap = t.as_ref().and_then(|th| th.pending_volume_sync.clone());
                if let Some(th) = t {
                    th.pending_volume_sync = None;
                    th.ui_turns = strip_ui_approvals(std::mem::take(&mut th.ui_turns));
                }
                snap
            };
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
                    "已收到选择，正在同步《{}》第{}卷设定库并确认卷记忆…",
                    args.get("project").and_then(|v| v.as_str()).unwrap_or("?"),
                    args.get("volume").and_then(|v| v.as_u64()).unwrap_or(0)
                )
            } else if tool_name == "confirm_volume_memory" {
                format!(
                    "已收到选择，正在确认《{}》第{}卷卷记忆…",
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
2. `design_plot` — 创建下卷第一段主线剧情卡（scope=local，对应卷纲阶梯前段；禁止一张卡复述整卷；无卷纲会失败）\n\
3. `update_plot(..., status=in_progress, set_active_main=true)` — 激活该卡\n\
4. 再 `continue_writing`\n\
说明：`sync_volume` 不写剧情卡；实体直接同步 status/holdings 终态（写入「当前状态」）。新建缺卡可再 `design_entity` 或 Web「设定缺口」补全。无进行中剧情卡时续写会被拦截。";
            let summary = if tool_name == "__skip_volume_sync" {
                // Persist handoff phase even when sync is skipped — unless plot work
                // remains (false volume-end). Then return to drafting this volume.
                let project = pending_snap
                    .as_ref()
                    .map(|p| p.project.clone())
                    .or_else(|| bound_project.clone());
                let volume_idx = pending_snap
                    .as_ref()
                    .map(|p| p.volume)
                    .unwrap_or(0);
                if let Some(project) = project {
                    let dir = project_dir(&self.roots.projects_root, &project);
                    let open_plots =
                        volume_idx > 0 && volume_has_open_plot_work(&dir, volume_idx);
                    if open_plots {
                        let _ = mark_volume_sync_skipped(&dir, false);
                        let _ = set_volume_phase(&dir, VolumePhase::DraftingVolume);
                        // Keep story_outline act open so the next publish can re-evaluate.
                        let _ = reopen_volume_act(&dir, volume_idx);
                        format!(
                            "{intro}\n\n检测到本卷剧情卡尚未走完（仍有进行中卡或未设计的 next_plot）。\
                             已取消错误卷末交接，恢复本卷写作。请 `design_plot` 开下一张本卷剧情卡后继续续写。"
                        )
                    } else {
                        let _ = mark_volume_sync_skipped(&dir, true);
                        let _ = set_volume_phase(&dir, VolumePhase::AwaitingNextArc);
                        format!("{intro}{handoff}")
                    }
                } else {
                    format!("{intro}{handoff}")
                }
            } else if tool_name == "confirm_volume_memory" {
                let (output, data) = self
                    .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                    .await?;
                let ok = data.get("ok").and_then(|v| v.as_bool()) != Some(false)
                    && data.get("error").is_none();
                let body = if ok {
                    format!("{intro}\n\n{output}\n\n卷记忆已写入。若设定库尚未同步，请继续选择「同步设定并确认卷记忆」或「跳过」。")
                } else {
                    format!("{intro}\n\n确认失败：{output}")
                };
                if let Some(pending) = pending_snap.clone() {
                    let options = self.volume_sync_options();
                    let prompt = {
                        let label = if pending.name.trim().is_empty() {
                            format!("第{}卷", pending.volume)
                        } else {
                            format!("第{}卷「{}」", pending.volume, pending.name)
                        };
                        format!("{label}卷记忆已处理，是否仍同步设定库？")
                    };
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.pending_volume_sync = Some(pending);
                        t.ui_turns = append_completion_ui_turn(
                            std::mem::take(&mut t.ui_turns),
                            &turn_id,
                            &body,
                            true,
                        );
                        t.ui_turns = attach_ui_approval(
                            std::mem::take(&mut t.ui_turns),
                            &turn_id,
                            &prompt,
                            &options,
                        );
                    }
                    self.emit_to_thread(
                        &thread_id,
                        EventMsg::AgentMessageContentDelta {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            item_id: agent_item_id.clone(),
                            delta: body,
                        },
                    )
                    .await;
                    self.emit_to_thread(
                        &thread_id,
                        EventMsg::RequestUserInput {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            prompt,
                            options,
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
                format!("{body}")
            } else {
                let (output, data) = self
                    .run_one_tool(&thread_id, &turn_id, &tool_name, &args.to_string())
                    .await?;
                let sync_ok = data.get("ok").and_then(|v| v.as_bool()) != Some(false)
                    && data.get("error").is_none()
                    && (data
                        .get("volume_phase")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| s.contains("awaiting_next"))
                        || data.get("volume_index").is_some());
                if !sync_ok {
                    // Failure used to clear the gate + print handoff while disk stayed
                    // awaiting_sync — refresh then re-offered「同步设定库」again.
                    let err = data
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or(output.as_str());
                    let fail_msg = format!(
                        "{intro}\n\n同步失败：{err}\n\n卷末同步未完成，请重试「同步设定库」或选「跳过」。"
                    );
                    if let Some(pending) = pending_snap.clone() {
                        let options = self.volume_sync_options();
                        let prompt = {
                            let label = if pending.name.trim().is_empty() {
                                format!("第{}卷", pending.volume)
                            } else {
                                format!("第{}卷「{}」", pending.volume, pending.name)
                            };
                            format!("{label}同步失败，是否重试同步设定库？")
                        };
                        if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                            t.pending_volume_sync = Some(pending);
                            t.ui_turns = append_completion_ui_turn(
                                std::mem::take(&mut t.ui_turns),
                                &turn_id,
                                &fail_msg,
                                true,
                            );
                            t.ui_turns = attach_ui_approval(
                                std::mem::take(&mut t.ui_turns),
                                &turn_id,
                                &prompt,
                                &options,
                            );
                            t.messages.push(ChatMessage {
                                role: "assistant".into(),
                                content: fail_msg.clone(),
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
                                    text: fail_msg,
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
                                prompt,
                                options,
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
                    format!("{intro}\n\n同步失败：{err}")
                } else {
                    format!("{intro}\n\n{output}{handoff}")
                }
            };
            let deferred_sb = pending_snap
                .as_ref()
                .and_then(|p| p.deferred_setting_blocker.clone());
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
            // Setting BLOCKER deferred at volume-end → ask before handoff.
            let offered_sb = if let Some(sb) = deferred_sb {
                self.offer_setting_blocker_gate(&thread_id, &turn_id, sb)
                    .await?
            } else {
                false
            };
            // After sync/skip → immediately offer「设计下卷卷纲」instead of only text.
            if !offered_sb {
                if let Some(project) = pending_snap
                    .as_ref()
                    .map(|p| p.project.clone())
                    .or_else(|| bound_project.clone())
                {
                    let _ = self
                        .offer_volume_handoff_gate(
                            &thread_id,
                            &turn_id,
                            &project,
                            VolumePhase::AwaitingNextArc,
                            None,
                        )
                        .await;
                }
            }
            let _ = self.persist_thread(&thread_id).await;
            self.emit_to_thread(&thread_id, EventMsg::TurnComplete {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                }).await;
            return Ok(());
        }

        // Soft revise after a *passed* audit: agent/user may say「局部修订」without an open gate.
        // Must run before the stale-queue dismiss (which used to swallow this as「队列已结束」).
        let no_blocking_gate = {
            let guard = self.threads.read().await;
            match guard.get(&thread_id) {
                Some(t) => {
                    t.pending_audit.is_none()
                        && t.pending_volume_audit.is_none()
                        && t.pending_chapter_next.is_none()
                        && t.pending_setup.is_none()
                        && t.pending_volume_handoff.is_none()
                        && t.pending_mutation.is_none()
                        && t.pending_chapter_order.is_none()
                }
                None => true,
            }
        };
        if no_blocking_gate && self.gates.is_audit_revise_choice(&text) {
            if let Some(project) = bound_project.clone() {
                if let Some(chapter) = self.soft_revise_target_chapter(&project) {
                    let instructions = self.soft_revise_instructions(&project, chapter);
                    tracing::info!(
                        %project,
                        chapter,
                        "soft audit revise (no pending_audit gate) → revise_chapter"
                    );
                    let args = json!({
                        "project": project,
                        "chapter": chapter,
                        "instructions": instructions,
                    });
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
                        "按上一轮审校改进项，准备局部修订第{chapter}章（《{project}》）…"
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
                    let (output, data) = self
                        .run_one_tool_mirrored(
                            &thread_id,
                            &turn_id,
                            "revise_chapter",
                            &args.to_string(),
                            Some(&agent_item_id),
                        )
                        .await?;
                    let summary = format!("{intro}\n\n{output}");
                    let asked_mutation = self
                        .maybe_offer_mutation_confirm(
                            &thread_id,
                            &turn_id,
                            "revise_chapter",
                            &args,
                            &data,
                        )
                        .await?;
                    if !asked_mutation {
                        let _ = self
                            .maybe_offer_audit_fix(
                                &thread_id,
                                &turn_id,
                                "revise_chapter",
                                &args,
                                &data,
                            )
                            .await?;
                    }
                    if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                        t.messages.push(ChatMessage {
                            role: "assistant".into(),
                            content: summary.clone(),
                            tool_call_id: None,
                            tool_calls: None,
                            ..Default::default()
                        });
                        let awaiting = t.pending_mutation.is_some() || t.pending_impact.is_some() || t.pending_audit.is_some();
                        t.ui_turns = append_completion_ui_turn(
                            std::mem::take(&mut t.ui_turns),
                            &turn_id,
                            &summary,
                            awaiting,
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
            }
        }
        if no_blocking_gate && self.gates.is_audit_accept_choice(&text) {
            let msg = "好的，保留原文。若要写下一章可以说「继续创作」；若要再审请发送「审阅第N章」。";
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
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: msg.into(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
                t.ui_turns = append_completion_ui_turn(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    msg,
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
                        text: msg.into(),
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

        // Stale *queue* approval click after queue finished — do not steer / LLM.
        // Must NOT treat「局部修订」/ chapter_next / setup / bare「继续」as expired queue choices.
        if self.gates.is_stale_audit_choice_token(&text)
            && no_blocking_gate
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
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
                    t.messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: msg.into(),
                        tool_call_id: None,
                        tool_calls: None,
                        ..Default::default()
                    });
                    t.ui_turns = append_completion_ui_turn(
                        std::mem::take(&mut t.ui_turns),
                        &turn_id,
                        msg,
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
                            text: msg.into(),
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

        // Deterministic: audit fix / queue choices (before chapter rewrite heuristics).
        if let Some((tool_name, args)) = self
            .parse_audit_steer_op(&thread_id, &text, bound_project.as_deref())
            .await
        {
            tracing::info!(%tool_name, args = %args, "studio direct audit steer (skip LLM routing)");
            // Close the gate *before* revise/reaudit work. Leaving pending_audit open until
            // tools finish lets HTTP snapshot / open_gate revive ApprovalOptions mid-turn,
            // unlock the composer, and queue a second steer click.
            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                t.pending_audit = None;
                t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
            }
            self.clear_queued_inputs(&thread_id).await;
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
            // After *revise* only: queue → re-audit current; single chapter → audit_chapter.
            // 「接受问题」必须结束门控，不得再自动复审（否则失败就死循环）。
            // Local-patch preview (needs_confirm) has NOT written disk — pause for apply first.
            let steer_choice = args
                .get("choice")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let steer_accepted = tool_name == "steer_run"
                && (steer_choice == "accept"
                    || steer_choice.contains("接受")
                    || data.get("accepted").and_then(|v| v.as_bool()) == Some(true));
            let steer_revise = tool_name == "steer_run" && !steer_accepted;
            let needs_confirm =
                data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true);
            let mut gate_tool = tool_name.as_str();
            let mut asked_mutation = false;
            if steer_revise && needs_confirm {
                if let Some(obj) = data.as_object_mut() {
                    obj.insert("reaudit_after".into(), json!(true));
                }
                asked_mutation = self
                    .maybe_offer_mutation_confirm(
                        &thread_id,
                        &turn_id,
                        &tool_name,
                        &args,
                        &data,
                    )
                    .await?;
            } else if steer_revise {
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
                            "verify_previous": true,
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
            let mut asked = if asked_mutation {
                true
            } else if steer_accepted {
                // Keep gate closed; strip leftover approval cards.
                self.dismiss_audit_gate(
                    &thread_id,
                    &turn_id,
                    "已接受问题，审校门控已关闭。",
                )
                .await;
                false
            } else {
                self.maybe_offer_audit_fix(
                    &thread_id,
                    &turn_id,
                    gate_tool,
                    &args,
                    &data,
                )
                .await?
            };
            // Safety net: revise cleared pending_audit; if queue still needs a decision,
            // rebuild the gate so the pipeline cannot silently end.
            // Never re-offer when this round's audit already passed, or user accepted.
            let reaudit_passed = data
                .get("consistency_passed")
                .and_then(|v| v.as_bool())
                == Some(true);
            if !asked && steer_revise && !needs_confirm && !reaudit_passed {
                asked = self
                    .maybe_reoffer_queue_gate(&thread_id, &turn_id)
                    .await?;
            }
            let mut offered_after_pass = false;
            if !asked
                && !asked_mutation
                && !steer_accepted
                && reaudit_passed
                && matches!(gate_tool, "audit_chapter" | "audit_chapters")
            {
                offered_after_pass = self
                    .maybe_offer_after_audit_pass(
                        &thread_id,
                        &turn_id,
                        gate_tool,
                        &args,
                        &data,
                    )
                    .await?;
            }
            let closing = if asked_mutation {
                "\n\n局部修订补丁已生成，请在审批卡选择「应用修改」或「放弃」。应用后再复审。"
            } else if asked {
                // Report is a separate bubble from emit_audit_report_before_gate —
                // keep this intro short so update_ui_turn_summary cannot clobber it.
                ""
            } else if steer_accepted {
                "\n\n已接受问题，审校门控已关闭。可说「继续」写下一章，或手动 audit_chapter 复审。"
            } else if offered_after_pass {
                "\n\n复审通过，请选择下一步。"
            } else if (gate_tool == "audit_chapter" || gate_tool == "audit_chapters")
                && reaudit_passed
            {
                "\n\n复审通过。可说「继续创作」写下一章。"
            } else if steer_revise {
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
                // Persist intro separately from the audit-report bubble.
                t.messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: summary.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
                // First agent bubble only — must not clobber the audit report item.
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

        // Bare「继续」during outline rewrite → keep rebuilding大纲, never jump to write章.
        if let Some(project) = bound_project.as_deref() {
            if self.with_policies(|p| p.is_continue_write_intent(&text))
                && self.outline_rewrite_blocks_bare_continue(&thread_id).await
            {
                tracing::info!(%project, "bare continue blocked — outline rewrite active");
                let summary = "当前正在重建大纲（总纲/卷纲），「继续」不会写下一章。请选择下一步：".to_string();
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
                        true,
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
                let _ = self
                    .maybe_offer_mutation_followup(
                        &thread_id,
                        &turn_id,
                        "design_master_outline",
                        &json!({ "project": project }),
                        &json!({ "project": project, "outline_rewrite": true }),
                    )
                    .await?;
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

        // Bare「继续」while next_chapter already has a draft: ask before tools / LLM.
        if let Some(project) = bound_project.as_deref() {
            if let Some(clarify) = self.clarify_bare_continue(project, &text) {
                tracing::info!(%project, "bare continue with existing draft — clarify without tools");
                let agent_item_id = new_id("item");
                let (chapter, _chars, suggest) = self.draft_exists_meta(project);
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
                let _ = self
                    .offer_draft_exists_gate(&thread_id, &turn_id, project, chapter, suggest)
                    .await?;
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

        // Deterministic intents from config/intents.yaml (Codex-style data-driven routing).
        // Bare「继续」+ next_chapter 无实质草稿 → 直接续写（勿落入 LLM 只查状态就结束）。
        let block_bare_for_outline = self.outline_rewrite_blocks_bare_continue(&thread_id).await;
        if self.features.deterministic_intents() {
        if let Some(intent) = self
            .intents
            .match_text(&text, bound_project.as_deref())
            .or_else(|| {
                if block_bare_for_outline {
                    None
                } else {
                    self.bare_continue_write_intent(bound_project.as_deref(), &text)
                }
            })
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
            } else if tool_name == "list_plots" {
                format!(
                    "正在核对《{}》的剧情进度…",
                    bound_project.as_deref().unwrap_or("?")
                )
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
            // Escaping into write/revise must drop stale audit gates (zombie pending_audit).
            if tool_name == "continue_writing" || tool_name == "revise_chapter" {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.pending_audit = None;
                    t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
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
            // list_plots already returns a Chinese progress report — don't bury it under intro spam.
            let mut summary = if tool_name == "list_plots" {
                output.clone()
            } else {
                format!("{intro}\n\n{output}")
            };
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
            let asked_mutation = self
                .maybe_offer_mutation_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                .await?;
            let asked_impact = if asked_mutation {
                false
            } else {
                self.maybe_offer_impact_cascade(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_order = if asked_mutation || asked_impact {
                false
            } else {
                self.maybe_offer_chapter_order(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_plot = if asked_mutation || asked_impact || asked_order {
                false
            } else {
                self.maybe_offer_plot_write(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_expected =
                if asked_mutation || asked_impact || asked_order || asked_plot {
                    false
                } else {
                    self.maybe_offer_expected_event(
                        &thread_id, &turn_id, &tool_name, &args, &data,
                    )
                    .await?
                };
            let asked_setup = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
            {
                false
            } else {
                self.maybe_offer_setup_confirm(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_draft = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
            {
                false
            } else {
                self.maybe_offer_draft_exists(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_volume = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
            {
                false
            } else {
                self.maybe_offer_volume_sync(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_handoff = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_volume
            {
                false
            } else {
                self.maybe_offer_volume_handoff(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_vol_audit = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_volume
                || asked_handoff
            {
                false
            } else {
                self.maybe_offer_volume_audit(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_audit = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_volume
                || asked_handoff
                || asked_vol_audit
            {
                false
            } else {
                self.maybe_offer_audit_fix(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_setting = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_volume
                || asked_handoff
                || asked_vol_audit
                || asked_audit
            {
                false
            } else {
                self.maybe_offer_setting_blocker(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_next = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_volume
                || asked_handoff
                || asked_vol_audit
                || asked_audit
                || asked_setting
            {
                false
            } else {
                self.maybe_offer_chapter_next(&thread_id, &turn_id, &tool_name, &args, &data)
                    .await?
            };
            let asked_followup = if asked_mutation
                || asked_impact
                || asked_order
                || asked_plot
                || asked_expected
                || asked_setup
                || asked_draft
                || asked_volume
                || asked_handoff
                || asked_vol_audit
                || asked_audit
                || asked_setting
                || asked_next
            {
                false
            } else {
                self.maybe_offer_mutation_followup(
                    &thread_id,
                    &turn_id,
                    &tool_name,
                    &args,
                    &data,
                )
                .await?
            };
            if asked_mutation {
                summary = format!(
                    "{}\n\n请选择：应用修改 / 放弃。",
                    data.get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("待确认修改")
                );
            } else if asked_impact {
                summary = format!("{summary}\n\n已扫描依赖面，请选择是否自动同步修正。");
            } else if asked_followup {
                summary = format!(
                    "{summary}\n\n正在请 Studio 给出下一步审批卡…"
                );
            } else if asked_order {
                let next = data
                    .get("next_chapter")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                summary = format!("不能跳章。请先写第{next}章。");
            } else if asked_plot {
                let title = data
                    .get("plot_title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("剧情卡");
                summary = if data.get("reason").and_then(|v| v.as_str())
                    == Some("need_design_plot")
                {
                    "⛔ 写章已拦截：需要先设计并激活剧情卡。请在下方选择下一步。".into()
                } else {
                    format!("⛔ 写章已拦截：剧情卡「{title}」尚未激活。请在下方选择下一步。")
                };
            } else if asked_setup {
                let hint = self
                    .threads
                    .read()
                    .await
                    .get(&thread_id)
                    .and_then(|t| t.pending_setup.as_ref())
                    .and_then(|p| SetupNextStep::parse(&p.next))
                    .map(|n| n.summary_hint())
                    .unwrap_or("请选择下一步。");
                summary = format!("{summary}\n\n{hint}");
            } else if asked_draft {
                summary = format!(
                    "第{}章已有未发布正文，请选择：审校本章 / 修订本章 / 写下一章。",
                    data.get("chapter").and_then(|v| v.as_u64()).unwrap_or(0)
                );
            } else if asked_volume {
                summary.push_str("\n\n本卷已结束 — 请选择：同步设定库 / 跳过。");
            } else if asked_handoff {
                let phase = data
                    .get("volume_phase")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                summary.push_str(match phase {
                    "awaiting_sync" => "\n\n请选择：同步设定库 / 跳过。",
                    "awaiting_next_plot" => "\n\n请选择：设计并激活剧情卡。",
                    _ => "\n\n请选择：设计下卷卷纲。",
                });
            } else if asked_vol_audit {
                summary.push_str("\n\n请选择：按建议深审 / 结束复盘。");
            } else if asked_setting {
                summary.push_str(
                    "\n\n设定审计 BLOCKER — 请选择：补全/修订设定卡 / 再跑设定审计 / 接受并继续。",
                );
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
                // Persist prose after tools so restore / WS sync doesn't leave only tool cards.
                t.ui_turns = upsert_ui_agent_message(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &agent_item_id,
                    &summary,
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
        } // features.deterministic_intents

        let injections = build_skill_injections(&skills, &activate);
        let skills_catalog = build_available_skills(&skills, Some(4000));
        let studio_body = std::fs::read_to_string(
            self.roots.config_root.join("skills/studio.md"),
        )
        .unwrap_or_default();
        let novel_draft_body = std::fs::read_to_string(
            self.roots.config_root.join("skills/novel-draft.md"),
        )
        .unwrap_or_default();
        let qa_hint_block = bound_project.as_deref().and_then(|p| {
            let dir = self.roots.projects_root.join(p);
            let hints = novelx_pipeline::studio_activation_hints_for_project(
                &self.roots.config_root,
                &dir,
                p,
            );
            novelx_pipeline::format_studio_activation_hints_block(&hints).map(|b| {
                format!(
                    "\n【当前项目长程 QA】\n{b}\n\
                     若用户未否定，应在合适时机主动调用上述工具；汇报进度时须转述此建议。\n"
                )
            })
        });
        let project_bind = if let Some(p) = bound_project.as_deref() {
            let mut s = format!(
                "当前会话已绑定项目《{p}》。用户指令默认针对此书。\n\
                 - 禁止无谓调用 list_projects（除非用户明确要「列出所有项目」）。\n\
             - 「修正/扩写/重写/加长第N章」→ 立即 revise_chapter(project=\"{p}\", chapter=N, instructions=用户原话或「扩写到5000-6000字」)。\n\
             - 用户只说「继续」：若下一章尚无正文 → 必须 continue_writing；若下一章已有草稿 → 先问清写下一章/修订/审校（勿只查状态就结束）。\n\
             - 问剧情卡/主线/写到哪了/对照进度 → list_plots + get_project_status 后中文汇报；禁止 continue_writing。\n\
             - 不要只列项目或只口头答应就结束；同一轮必须把可执行工具跑完。\n"
            );
            if let Some(qa) = qa_hint_block {
                s.push_str(&qa);
            }
            s
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
             单章审校用 audit_chapter。通过（含仅有 P1/P2）→ 勿称未通过、勿伪造审批卡；用户要改则 revise_chapter。\
             未通过 → 立即 offer_decisions（按 issue_id 给出修某条/修全部阻断/接受等），不要只给「按审校局部修订」。\
             禁止在正文里自拟编号审批卡；决策卡只经 offer_decisions / 服务端 open_gate。\n\n{skills_catalog}"
        );
        if !studio_body.trim().is_empty() {
            system.push_str("\n\n# Skill: studio\n\n");
            system.push_str(&studio_body);
        }
        // Field contract for setup collecting — not an LLM extractor agent.
        if !novel_draft_body.trim().is_empty() {
            system.push_str("\n\n# Skill: novel-draft\n\n");
            system.push_str(&novel_draft_body);
        }
        for inj in &injections {
            if inj.name == "studio" || inj.name == "novel-draft" {
                continue; // already injected as default
            }
            system.push_str(&format!(
                "\n\n# Skill: {}\n\n{}",
                inj.name, inj.body
            ));
        }

        let specs = tool_specs(&self.tools);
        let mut agent_item_id = new_id("item");
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
        let mut opened_post_tool_bubble = false;
        let mut nudged_continue = false;
        let user_needs_chapter_op = self
            .intents
            .match_text(&text, Some("__intent_probe__"))
            .is_some();
        let mut pause_for_human = false;
        let mut started_new_chapter = false;
        let mut spam_tool_name = String::new();
        let mut spam_tool_streak: usize = 0;
        // After audit fail: wait one Studio round for `offer_decisions` before fallback gate.
        let mut awaiting_studio_audit_offer = false;

        for _round in 0..MAX_TOOL_ROUNDS {
            // Set when continue_writing audit-fails: stub sibling tools, then continue
            // the outer loop so Studio can call offer_decisions (do not pause yet).
            let mut end_batch_for_studio_offer = false;
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

            // After tools ran, open a fresh bubble so the final answer lands *below*
            // tool cards. Streaming into the pre-tool bubble left an empty/invisible
            // NovelX message (TurnTimeline hides blank agent_message).
            if !last_tool_names.is_empty() && !opened_post_tool_bubble {
                opened_post_tool_bubble = true;
                agent_item_id = new_id("item");
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
            }

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
                    .await
                });

                let tool_result =
                    dispatch(&self.tools, &tool_ctx, &tc.name, &tc.arguments).await;
                drop(tool_ctx);
                // Progress flusher can stall on a full WS sink; never block ItemCompleted.
                let streamed = match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    &mut forward,
                )
                .await
                {
                    Ok(Ok(s)) => s,
                    Ok(Err(_)) => String::new(),
                    Err(_) => {
                        tracing::warn!(
                            tool = %tc.name,
                            "tool progress flusher timed out; aborting flush to unblock turn"
                        );
                        forward.abort();
                        String::new()
                    }
                };
                let duration_ms = start.elapsed().as_millis() as u64;
                let (output, status, data) = match tool_result {
                    Ok(r) => (r.output, ItemStatus::Completed, r.data),
                    Err(e) => (
                        e.to_string(),
                        ItemStatus::Failed,
                        json!({"ok": false, "error": e.to_string()}),
                    ),
                };
                // Chat/WS: compact bulk context tools; model messages keep full `output`.
                let final_out = tool_output_for_ui(&tc.name, &args_val, &output, &data)
                    .unwrap_or_else(|| output.clone());
                // Keep ▶/✓ process lines; don't replace the stream with a short coda only.
                let ui_output = merge_tool_ui_output(&streamed, &final_out);

                // Only emit trailing delta when the coda wasn't already streamed / restated.
                if !final_out.is_empty()
                    && !streamed.contains(final_out.trim())
                    && !tool_ui_looks_like_restated_report(&streamed, &final_out)
                {
                    self.emit_to_thread(&thread_id, EventMsg::ToolCallOutputDelta {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            item_id: item_id.clone(),
                            delta: if streamed.is_empty() {
                                final_out.clone()
                            } else {
                                format!("\n{final_out}")
                            },
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

                let mut tool_content = output;
                // Studio decision tool: open per-issue gate from offered options.
                if tc.name == "offer_decisions" {
                    match self
                        .apply_offer_decisions(&thread_id, &turn_id, &args_val, &data)
                        .await?
                    {
                        OfferApplyResult::OpenedGate => {
                            awaiting_studio_audit_offer = false;
                            if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                                t.messages.push(ChatMessage {
                                    role: "tool".into(),
                                    content: tool_content,
                                    tool_call_id: Some(tc.id.clone()),
                                    tool_calls: None,
                                    ..Default::default()
                                });
                            }
                            answered_ids.push(tc.id.clone());
                            pause_for_human = true;
                            break;
                        }
                        OfferApplyResult::Rejected(err) => {
                            tool_content = format!(
                                "offer_decisions 被拒绝：{err}。请修正 options 后重试（勿在正文伪造编号卡）。"
                            );
                            // Fall through: push rejected tool result so the model can retry.
                        }
                        OfferApplyResult::Ignored => {}
                    }
                }
                // Content audit fail → prepare checklist, let Studio offer decisions next round.
                let audit_fail = matches!(
                    tc.name.as_str(),
                    "audit_chapter" | "audit_chapters" | "continue_writing"
                ) && data.get("consistency_passed").and_then(|v| v.as_bool())
                    == Some(false)
                    && !audit_failure_is_meta_only(&data)
                    && data.get("volume_ended").and_then(|v| v.as_u64()).is_none();
                if audit_fail {
                    let project = data
                        .get("project")
                        .and_then(|v| v.as_str())
                        .or_else(|| args_val.get("project").and_then(|v| v.as_str()))
                        .unwrap_or("");
                    let chapter = data
                        .get("chapter")
                        .and_then(|v| v.as_u64())
                        .or_else(|| args_val.get("chapter").and_then(|v| v.as_u64()))
                        .unwrap_or(1) as u32;
                    if !project.is_empty() {
                        self.prepare_audit_fail_for_studio(
                            &thread_id,
                            &turn_id,
                            project,
                            chapter,
                            &data,
                        )
                        .await?;
                        tool_content.push_str(
                            "\n\n【决策】请立即调用 offer_decisions：按问题给出可执行选项\
                             （修某条 issue_id / 修全部阻断 / 接受）。禁止在正文里伪造编号审批卡。",
                        );
                        awaiting_studio_audit_offer = true;
                    }
                }
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    t.messages.push(ChatMessage {
                        role: "tool".into(),
                        content: tool_content,
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: None,
                    ..Default::default()
                });
                }
                answered_ids.push(tc.id.clone());

                // Mutation confirm / chapter order / setup / volume / audit → stop.
                // Content audit fail already prepared above — skip static gate so Studio can offer.
                let audit_gate = if audit_fail {
                    false
                } else {
                    self.maybe_offer_audit_fix(
                        &thread_id,
                        &turn_id,
                        &tc.name,
                        &args_val,
                        &data,
                    )
                    .await?
                };
                if self
                    .maybe_offer_mutation_confirm(
                        &thread_id,
                        &turn_id,
                        &tc.name,
                        &args_val,
                        &data,
                    )
                    .await?
                    || self
                        .maybe_offer_impact_cascade(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
                        .maybe_offer_chapter_order(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
                        .maybe_offer_plot_write(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
                        .maybe_offer_expected_event(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
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
                        .maybe_offer_volume_handoff(
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
                        .maybe_offer_setting_blocker(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || self
                        .maybe_offer_mutation_followup(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?
                    || audit_gate
                {
                    pause_for_human = true;
                    break;
                }
                // Draft-exists clarify / one chapter per turn — do not auto-chain.
                if tc.name == "continue_writing" || tc.name == "revise_chapter" {
                    if self
                        .maybe_offer_draft_exists(
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
                    let _ = self
                        .maybe_offer_chapter_next(
                            &thread_id,
                            &turn_id,
                            &tc.name,
                            &args_val,
                            &data,
                        )
                        .await?;
                    // Content audit fail: keep the tool loop alive for one Studio
                    // `offer_decisions` round. Pausing here left pending_audit in
                    // awaiting_offer with no RequestUserInput (zombie gate).
                    if defer_pause_for_studio_audit_offer(
                        audit_fail,
                        awaiting_studio_audit_offer,
                    ) {
                        end_batch_for_studio_offer = true;
                        break;
                    }
                    pause_for_human = true;
                    break;
                }
                // Audit-only hard-rule block: open「修正本章」(not consistency decision card).
                if matches!(tc.name.as_str(), "audit_chapter" | "audit_chapters")
                    && self
                        .maybe_offer_chapter_next(
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
                // Other mutate tools that returned needs_confirm already paused above.
            }
            // Human gate / early stop: stub remaining tool_call_ids so next turn's history is valid.
            // Also stub when ending a batch for Studio offer_decisions (no pause yet).
            if pause_for_human || end_batch_for_studio_offer {
                if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
                    for tc in &pending_tool_calls {
                        if answered_ids.iter().any(|id| id == &tc.id) {
                            continue;
                        }
                        let reason = if pause_for_human {
                            format!(
                                "（未执行：上一工具需用户确认后结束本轮，跳过 {}）",
                                tc.name
                            )
                        } else {
                            format!(
                                "（未执行：上一工具触发审校决策，等待 offer_decisions，跳过 {}）",
                                tc.name
                            )
                        };
                        t.messages.push(ChatMessage {
                            role: "tool".into(),
                            content: reason,
                            tool_call_id: Some(tc.id.clone()),
                            tool_calls: None,
                    ..Default::default()
                });
                    }
                    // Persist a repaired history for the next user turn.
                    t.messages = sanitize_chat_messages(std::mem::take(&mut t.messages));
                }
                if pause_for_human {
                    break;
                }
                // end_batch_for_studio_offer: continue outer loop for Studio offer.
                continue;
            }
        }

        // Studio skipped offer_decisions → open deterministic per-P0 decision card.
        // Independent of pause_for_human: continue_writing used to pause before Studio
        // could offer, leaving awaiting_offer with no UI card.
        let pending_awaiting_offer = {
            self.threads
                .read()
                .await
                .get(&thread_id)
                .and_then(|t| t.pending_audit.as_ref())
                .is_some_and(|p| p.awaiting_offer && p.kind == AuditGateKind::Content)
        };
        if should_open_audit_offer_fallback(awaiting_studio_audit_offer, pending_awaiting_offer) {
            let pending = {
                self.threads
                    .read()
                    .await
                    .get(&thread_id)
                    .and_then(|t| t.pending_audit.clone())
            };
            if let Some(p) = pending.filter(|p| p.awaiting_offer && p.kind == AuditGateKind::Content)
            {
                let queue_active =
                    load_audit_queue(&self.roots.projects_root, &p.project).is_some();
                let data = json!({
                    "project": p.project,
                    "chapter": p.chapter,
                    "issues": p.issues,
                    "consistency_passed": false,
                });
                self.open_content_audit_gate(
                    &thread_id,
                    &turn_id,
                    &p.project,
                    p.chapter,
                    &data,
                    queue_active,
                    None,
                    false,
                )
                .await?;
                pause_for_human = true;
            }
        }

        // Studio skipped studio_next offer after situational nudge → soft fallback.
        if !pause_for_human {
            let _ = self
                .maybe_offer_studio_next_fallback(&thread_id, &turn_id)
                .await?;
        }

        // If the model returned prose only via message history (client missed deltas),
        // still surface it in the post-tool bubble.
        let mut closing = final_text.clone();
        if closing.trim().is_empty() {
            if let Some(t) = self.threads.read().await.get(&thread_id) {
                if let Some(last) = t.messages.iter().rev().find(|m| {
                    m.role == "assistant"
                        && !m.content.trim().is_empty()
                        // Checklist is a dedicated UI bubble — never reuse as turn closing.
                        && !is_audit_checklist_text(&m.content)
                }) {
                    closing = last.content.clone();
                }
            }
        }
        // If Studio restated the checklist in final_text, drop it when a checklist
        // bubble already exists (avoids one NovelX message with the list twice).
        if is_audit_checklist_text(&closing) {
            let has_dedicated = {
                let guard = self.threads.read().await;
                guard.get(&thread_id).and_then(|t| t.ui_turns.as_array()).is_some_and(|turns| {
                    turns.iter().any(|tr| {
                        tr.get("id").and_then(|v| v.as_str()) == Some(turn_id.as_str())
                            && tr.get("items").and_then(|v| v.as_array()).is_some_and(|items| {
                                items.iter().any(|it| {
                                    it.get("type").and_then(|v| v.as_str()) == Some("agent_message")
                                        && it.get("id").and_then(|v| v.as_str())
                                            != Some(agent_item_id.as_str())
                                        && is_audit_checklist_text(
                                            it.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                                        )
                                })
                            })
                    })
                })
            };
            if has_dedicated {
                closing.clear();
            }
        }

        self.emit_to_thread(&thread_id, EventMsg::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id.clone(),
                    text: closing.clone(),
                    status: ItemStatus::Completed,
                },
            }).await;

        // Server-authoritative: clear「调用模型中」spinners even if the client
        // missed ItemCompleted / TurnComplete over a congested WebSocket.
        // Also persist NovelX prose into ui_turns (HTTP restore used to show only tool cards).
        if let Some(t) = self.threads.write().await.get_mut(&thread_id) {
            t.ui_turns = mark_ui_turn_complete(std::mem::take(&mut t.ui_turns), &turn_id);
            if !closing.trim().is_empty() {
                t.ui_turns = upsert_ui_agent_message(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    &agent_item_id,
                    &closing,
                );
            }
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
        // If「新开任务」already replaced this project's root thread, do not let a
        // late persist from the old id rewrite studio_thread.json.
        if let Some(proj) = persisted.project.as_deref() {
            let superseded = guard.values().any(|t| {
                t.summary.id != thread_id
                    && t.summary.project.as_deref() == Some(proj)
                    && t.session_source.is_root()
            });
            if superseded {
                return Ok(());
            }
        }
        drop(guard);
        // Re-check membership after drop: clear may have removed us mid-flight.
        if !self.threads.read().await.contains_key(thread_id) {
            return Ok(());
        }
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
                // Audit queue is project-durable and would be re-attached by
                // reconcile_orphan_audit_queue on the next refresh — clear it too.
                let _ = clear_audit_queue(&self.roots.projects_root, proj);
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
                pending_volume_handoff: None,
                pending_chapter_next: None,
                pending_chapter_order: None,
                pending_plot_write: None,
                pending_expected_event: None,
                pending_setting_blocker: None,
                skipped_expected_ids: Vec::new(),
                pending_mutation: None,
                pending_mutation_followup: None,
                pending_studio_next: None,
                awaiting_studio_next: None,
                outline_rewrite_active: false,
                pending_impact: None,
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
        // Persist tool card into ui_turns (WS clients already see ItemStarted; HTTP restore needs this).
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.ui_turns = upsert_ui_tool_call(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &item_id,
                name,
                &args_val,
                None,
                "in_progress",
                None,
            );
        }

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
            .await
        });

        let tool_result = dispatch(&self.tools, &tool_ctx, name, arguments).await;
        drop(tool_ctx);
        let streamed = match tokio::time::timeout(std::time::Duration::from_secs(2), &mut forward)
            .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(_)) => String::new(),
            Err(_) => {
                tracing::warn!(%name, "direct tool progress flusher timed out; aborting flush");
                forward.abort();
                String::new()
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;
        let (output, status, data) = match tool_result {
            Ok(r) => (r.output, ItemStatus::Completed, r.data),
            Err(e) => (
                e.to_string(),
                ItemStatus::Failed,
                // Callers (e.g. volume sync) must detect failure — empty data looked like success.
                json!({"ok": false, "error": e.to_string()}),
            ),
        };
        let final_out =
            tool_output_for_ui(name, &args_val, &output, &data).unwrap_or_else(|| output.clone());
        // Keep ▶/✓ process lines from the stream; final_out alone is often a short coda.
        let ui_output = merge_tool_ui_output(&streamed, &final_out);
        let status_str = match status {
            ItemStatus::Completed => "completed",
            ItemStatus::Failed => "failed",
            ItemStatus::Cancelled => "cancelled",
            ItemStatus::InProgress => "in_progress",
        };

        // Only emit a trailing delta when the final coda wasn't already streamed / restated.
        if !final_out.is_empty()
            && !streamed.contains(final_out.trim())
            && !tool_ui_looks_like_restated_report(&streamed, &final_out)
        {
            self.emit_to_thread(
                thread_id,
                EventMsg::ToolCallOutputDelta {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: item_id.clone(),
                    delta: if streamed.is_empty() {
                        final_out.clone()
                    } else {
                        format!("\n{final_out}")
                    },
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
                    id: item_id.clone(),
                    name: name.to_string(),
                    arguments: args_val.clone(),
                    output: Some(ui_output.clone()),
                    status,
                    duration_ms: Some(duration_ms),
                },
            },
        )
        .await;

        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.ui_turns = upsert_ui_tool_call(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &item_id,
                name,
                &args_val,
                Some(&ui_output),
                status_str,
                Some(duration_ms),
            );
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

    /// Chapter to polish when user says「局部修订」after a passed audit (no open gate).
    fn soft_revise_target_chapter(&self, project: &str) -> Option<u32> {
        if project.is_empty() {
            return None;
        }
        let dir = novelx_pipeline::project_dir(&self.roots.projects_root, project);
        let state = novelx_pipeline::load_project_state(&dir).ok()?;
        let hi = state.published_count.max(state.next_chapter).max(1);
        // Prefer newest chapter that already has an audit report.
        for ch in (1..=hi).rev() {
            let audit = dir
                .join("chapters")
                .join(format!("{ch:03}"))
                .join("audit.json");
            if audit.is_file() {
                return Some(ch);
            }
        }
        if state.published_count > 0 {
            return Some(state.published_count);
        }
        let draft = dir
            .join("chapters")
            .join(format!("{:03}", state.next_chapter.max(1)))
            .join("draft.md");
        if draft.is_file() {
            return Some(state.next_chapter.max(1));
        }
        None
    }

    fn soft_revise_instructions(&self, project: &str, chapter: u32) -> String {
        let path = novelx_pipeline::project_dir(&self.roots.projects_root, project)
            .join("chapters")
            .join(format!("{chapter:03}"))
            .join("audit.json");
        let fallback = format!(
            "按第{chapter}章上一轮审校的可改进项做局部修订，保持情节与设定一致，不要无故全文重写。"
        );
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return fallback;
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            return fallback;
        };
        let mut tips: Vec<String> = Vec::new();
        if let Some(issues) = v.get("issues").and_then(|x| x.as_array()) {
            for it in issues.iter().take(6) {
                let pri = it
                    .get("priority")
                    .or_else(|| it.get("severity"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let msg = it
                    .get("message")
                    .or_else(|| it.get("detail"))
                    .or_else(|| it.get("title"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim();
                if msg.is_empty() {
                    continue;
                }
                if pri.is_empty() {
                    tips.push(format!("- {msg}"));
                } else {
                    tips.push(format!("- [{pri}] {msg}"));
                }
            }
        }
        if tips.is_empty() {
            if let Some(report) = v.get("report").and_then(|x| x.as_str()) {
                let snippet: String = report.chars().take(400).collect();
                if !snippet.trim().is_empty() {
                    return format!(
                        "按第{chapter}章审校报告中的可改进项做局部修订：\n{snippet}"
                    );
                }
            }
            return fallback;
        }
        format!(
            "按第{chapter}章审校可改进项做局部修订：\n{}\n保持情节与设定一致，优先补丁、避免无故全文重写。",
            tips.join("\n")
        )
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
            chapter_order: self.features.enforce_chapter_order(),
            mutation_confirm: self.features.require_mutation_confirm(),
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
        // Preview-only continue_writing must not wipe chat / pending gates.
        // Gate-sourced `confirm_skip` means the write is real — allow clear.
        if self.features.require_mutation_confirm()
            && args.get("apply").and_then(|v| v.as_bool()) != Some(true)
            && !novelx_tools::confirm_skipped(args)
        {
            return false;
        }
        // Never wipe on ambiguous bare「继续」(next chapter already has a draft).
        // Bare「继续」with empty next chapter *is* a new-chapter write — allow clear.
        let chapter_named = parse_chapter_number(user_text).is_some();
        let intends = self
            .with_policies(|p| p.user_intends_new_chapter_write(user_text, chapter_named));
        let bare_ready = self.with_policies(|p| p.is_bare_continue(user_text))
            && self.next_chapter_draft_chars(project) < self.with_policies(|p| p.draft_min_chars());
        if !intends && !bare_ready {
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
            if draft.chars().count() >= self.with_policies(|p| p.draft_min_chars()) {
                return false;
            }
        }
        true
    }

    fn next_chapter_draft_chars(&self, project: &str) -> usize {
        let dir = project_dir(&self.roots.projects_root, project);
        let Ok(state) = novelx_pipeline::load_project_state(&dir) else {
            return 0;
        };
        let chapter = state.next_chapter.max(1);
        novelx_pipeline::read_chapter_draft(&dir, chapter)
            .unwrap_or_default()
            .chars()
            .count()
    }

    /// Bare「继续」/「继续创作」+ next_chapter 无实质草稿 → deterministic continue_writing.
    fn bare_continue_write_intent(
        &self,
        project: Option<&str>,
        user_text: &str,
    ) -> Option<IntentMatch> {
        let project = project.filter(|p| !p.is_empty() && *p != "_")?;
        if !self.with_policies(|p| p.is_continue_write_intent(user_text)) {
            return None;
        }
        // Ambiguous: next chapter already has a draft (handled by clarify_bare_continue).
        if self.next_chapter_draft_chars(project) >= self.with_policies(|p| p.draft_min_chars()) {
            return None;
        }
        let dir = project_dir(&self.roots.projects_root, project);
        let _ = novelx_pipeline::load_project_state(&dir).ok()?;
        tracing::info!(%project, "bare continue → continue_writing (next chapter empty)");
        Some(IntentMatch {
            id: "bare_continue_write".into(),
            tool: "continue_writing".into(),
            args: json!({ "project": project }),
            clear_history: ClearHistory::OnStart,
        })
    }

    async fn outline_rewrite_blocks_bare_continue(&self, thread_id: &str) -> bool {
        let guard = self.threads.read().await;
        let Some(t) = guard.get(thread_id) else {
            return false;
        };
        if t.outline_rewrite_active
            || t.pending_studio_next.is_some()
            || t.awaiting_studio_next.is_some()
            || t.pending_mutation_followup.is_some()
        {
            return true;
        }
        if let Some(m) = &t.pending_mutation {
            if Self::is_structure_mutation_tool(&m.apply_tool) {
                return true;
            }
        }
        // Recent failed/incomplete outline apply still on screen.
        for m in t.messages.iter().rev().take(10) {
            let c = m.content.as_str();
            let outlineish = c.contains("总纲") || c.contains("卷纲") || c.contains("master_outline");
            if outlineish
                && (c.contains("未进入可应用")
                    || c.contains("请选择：继续未完成步骤")
                    || c.contains("结构/设定审计仍有 BLOCKER")
                    || (c.contains("BLOCKER") && c.contains("结构审计")))
            {
                return true;
            }
        }
        false
    }

    /// When user says「继续」/「继续创作」and next_chapter already has a draft, clarify.
    fn clarify_bare_continue(&self, project: &str, user_text: &str) -> Option<String> {
        if !self.with_policies(|p| p.is_continue_write_intent(user_text)) {
            return None;
        }
        let (chapter, chars, suggest) = self.draft_exists_meta(project);
        if chars < self.with_policies(|p| p.draft_min_chars()) {
            return None;
        }
        Some(format!(
            "第{chapter}章已有正文（约 {chars} 字），尚未发布（next_chapter={chapter}）。\n\
             「继续」有歧义，请选择：审校本章 / 修订本章 / 写第{suggest}章。"
        ))
    }

    fn draft_exists_meta(&self, project: &str) -> (u32, usize, u32) {
        let dir = project_dir(&self.roots.projects_root, project);
        let chapter = novelx_pipeline::load_project_state(&dir)
            .map(|s| s.next_chapter.max(1))
            .unwrap_or(1);
        let chars = novelx_pipeline::read_chapter_draft(&dir, chapter)
            .unwrap_or_default()
            .chars()
            .count();
        (chapter, chars, chapter.saturating_add(1))
    }

    async fn offer_draft_exists_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        chapter: u32,
        suggest_chapter: u32,
    ) -> Result<bool> {
        let _ = turn_id;
        self.request_studio_next(
            thread_id,
            project,
            &format!(
                "第{chapter}章已有未发布正文；可审校/修订本章，或写第{suggest_chapter}章"
            ),
            studio_next::StudioNextContext::DraftExists {
                chapter,
                suggest_chapter,
            },
            false,
        )
        .await
    }

    async fn maybe_offer_draft_exists(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        _args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if tool_name != "continue_writing" {
            return Ok(false);
        }
        if data.get("blocked").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(false);
        }
        if data.get("reason").and_then(|v| v.as_str()) != Some("draft_exists_without_chapter") {
            return Ok(false);
        }
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| _args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let chapter = data
            .get("chapter")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let suggest = data
            .get("suggest_chapter")
            .and_then(|v| v.as_u64())
            .unwrap_or(chapter.saturating_add(1) as u64) as u32;
        if project.is_empty() || chapter == 0 {
            return Ok(false);
        }
        self.offer_draft_exists_gate(thread_id, turn_id, &project, chapter, suggest)
            .await
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
            "已清理先前对话，正在撰写第{chapter}章（《{project}》）。\
             进度见下方「继续创作」工具卡；设定与正文以项目文件为准。"
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
            t.pending_chapter_order = None;
            t.pending_plot_write = None;
            t.pending_expected_event = None;
            t.pending_setting_blocker = None;
            t.pending_mutation = None;
            t.pending_mutation_followup = None;
            t.pending_studio_next = None;
            t.awaiting_studio_next = None;
            t.outline_rewrite_active = false;
            t.pending_impact = None;
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
        let (mutation, impact, order, studio_next, setup, volume, handoff, audit, chapter_next, turn_id) = {
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
                t.pending_mutation.clone(),
                t.pending_impact.clone(),
                t.pending_chapter_order.clone(),
                t.pending_studio_next.clone(),
                t.pending_setup.clone(),
                t.pending_volume_sync.clone(),
                t.pending_volume_handoff.clone(),
                t.pending_audit.clone(),
                t.pending_chapter_next.clone(),
                turn_id,
            )
        };
        let mutation_preview = mutation.as_ref().map(|m| m.preview.clone());
        let (prompt, options) = if let Some(m) = mutation {
            (
                format!("待确认：{}", m.summary),
                self.gates.mutation_confirm_options(),
            )
        } else if let Some(imp) = impact {
            (
                format!(
                    "设定/结构已更新，发现 {} 处可能受影响的依赖位点。是否自动同步修正？\n\n{}",
                    imp.hits.len(),
                    imp.summary_markdown
                ),
                self.gates.impact_confirm_options(),
            )
        } else if let Some(o) = order {
            (
                format!("不能跳章。当前应写第{}章，请选择：", o.next_chapter),
                self.gates.chapter_order_options(o.next_chapter),
            )
        } else if let Some(sn) = studio_next {
            (
                sn.prompt.clone(),
                sn.options
                    .iter()
                    .map(studio_next::StudioNextOption::to_ui_option)
                    .collect(),
            )
        } else if let Some(s) = setup {
            let next = SetupNextStep::parse(&s.next).unwrap_or(SetupNextStep::Confirm);
            (next.prompt(&s.project), self.gates.options(next.gate_name()))
        } else if let Some(v) = volume {
            let label = if v.name.is_empty() {
                format!("第{}卷", v.volume)
            } else {
                format!("第{}卷「{}」", v.volume, v.name)
            };
            (
                format!("{label}已结束。请同步设定库，并确认卷记忆。"),
                self.volume_sync_options(),
            )
        } else if let Some(h) = handoff {
            let (gate, prompt) = match h.phase.as_str() {
                "awaiting_next_plot" => (
                    "volume_handoff_plot",
                    format!(
                        "卷间交接：第{}卷卷纲已就绪，请设计并激活开局剧情卡。",
                        h.volume
                    ),
                ),
                _ => (
                    "volume_handoff_arc",
                    format!(
                        "卷间交接：请先设计第{}卷卷纲，再开剧情卡写章。",
                        h.volume
                    ),
                ),
            };
            (prompt, self.gates.options(gate))
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
            if c.suggest_next.is_some() {
                // Legacy draft_exists pending — cleared; situational cards use studio_next.
                return;
            }
            let options = self.chapter_next_options(c.published, c.plot_accept_open);
            let prompt = if c.published {
                chapter_next_published_prompt(c.chapter, c.plot_accept_open)
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
            if let Some(preview) = &mutation_preview {
                t.ui_turns = attach_ui_mutation_preview(
                    std::mem::take(&mut t.ui_turns),
                    &turn_id,
                    preview,
                    preview.get("diffs").cloned().unwrap_or(json!([])),
                );
            }
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

    /// After volume L1 audit, Studio offers deep-audit / dismiss card.
    async fn maybe_offer_volume_audit(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        let _ = turn_id;
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
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_audit = None;
        }
        self.request_studio_next(
            thread_id,
            &project,
            &format!("第{volume}卷复盘完成。建议深审：{list}"),
            studio_next::StudioNextContext::VolumeAudit { volume, chapters },
            false,
        )
        .await
    }

    /// Guide setup progress: write blocked on setup, or after outline/bible tools.
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
        if data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(false);
        }
        let blocked = data.get("blocked").and_then(|v| v.as_bool()) == Some(true);
        let from_write_block = tool_name == "continue_writing"
            && blocked
            && data.get("reason").and_then(|v| v.as_str()) == Some("setup");
        let from_progress = matches!(
            tool_name,
            "design_master_outline"
                | "design_arc_outline"
                | "lock_brief"
                | "upsert_setting"
                | "supplement_setting"
                | "confirm_setup"
        ) && !blocked;
        let offer_flag = data
            .get("offer_setup_confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || data.get("setup_phase").and_then(|v| v.as_str()) == Some("awaiting_confirm");
        if !from_write_block && !from_progress && !offer_flag {
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
        let dir = project_dir(&self.roots.projects_root, &project);
        let Some(next) = resolve_setup_next_step(&dir) else {
            return Ok(false);
        };
        // Same open gate → do not stack another RequestUserInput.
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if let Some(p) = &t.pending_setup {
                    if p.project == project && p.next == next.as_str() {
                        return Ok(true);
                    }
                }
                if t.pending_mutation.is_some()
                    || t.pending_impact.is_some()
                    || t.pending_audit.is_some()
                    || t.pending_volume_sync.is_some()
                    || t.pending_volume_handoff.is_some()
                {
                    return Ok(false);
                }
            }
        }
        self.offer_setup_gate(thread_id, turn_id, &project, next).await
    }

    async fn offer_setup_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        next: SetupNextStep,
    ) -> Result<bool> {
        let gate = next.gate_name();
        let options = self.gates.options(gate);
        if options.is_empty() {
            return Ok(false);
        }
        let prompt = next.prompt(project);
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_setup = Some(PendingSetup {
                project: project.to_string(),
                next: next.as_str().to_string(),
            });
            t.pending_audit = None;
            t.pending_chapter_next = None;
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
        // Reaudit/publish via audit_chapter can also end a volume.
        if tool_name != "continue_writing"
            && tool_name != "revise_chapter"
            && tool_name != "audit_chapter"
        {
            return Ok(false);
        }
        let Some(volume) = data.get("volume_ended").and_then(|v| v.as_u64()) else {
            return Ok(false);
        };
        // Already waiting on this volume — do not stack duplicate RequestUserInput.
        let project_hint = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("");
        {
            let guard = self.threads.read().await;
            if let Some(p) = guard
                .get(thread_id)
                .and_then(|t| t.pending_volume_sync.as_ref())
            {
                if (project_hint.is_empty() || p.project == project_hint)
                    && p.volume as u64 == volume
                {
                    return Ok(true);
                }
            }
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
        let dir = project_dir(&self.roots.projects_root, &project);
        let mem_preview = if start > 0 && end > 0 {
            format_volume_memory_preview(&dir, volume as u32, &name, start as u32, end as u32)
        } else {
            String::new()
        };
        let setting_blocker = data.get("plot_setting_blocker").and_then(|v| v.as_bool())
            == Some(true);
        let sb_chapter = data
            .get("chapter")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .filter(|c| *c > 0)
            .unwrap_or(end as u32);
        let sb_detail = data
            .get("setting_blocker_detail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(240)
            .collect::<String>();
        let deferred_sb = if setting_blocker {
            Some(PendingSettingBlocker {
                project: project.clone(),
                chapter: sb_chapter,
                detail: sb_detail,
                resume_chapter_next: false,
                published: false,
                plot_accept_open: false,
                content_blocked: false,
                revise_instructions: None,
                offer_volume_handoff_after: true,
            })
        } else {
            None
        };
        let mut prompt = if mem_preview.is_empty() {
            format!("{label}{range}已结束。请同步设定库，并确认卷记忆（防长程漂移）。")
        } else {
            format!(
                "{label}{range}已结束。请同步设定库，并确认卷记忆（防长程漂移）。\n\n{mem_preview}"
            )
        };
        if setting_blocker {
            prompt.push_str(
                "\n\n⚠ 本卷剧情后设定审计出现 BLOCKER；完成卷末选择后将请你处理设定缺口。",
            );
        }
        let options = self.volume_sync_options();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_volume_sync = Some(PendingVolumeSync {
                project: project.clone(),
                volume: volume as u32,
                name: name.clone(),
                deferred_setting_blocker: deferred_sb,
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

    fn handoff_target_volume(&self, project: &str) -> u32 {
        let dir = project_dir(&self.roots.projects_root, project);
        let bounds = load_volume_bounds(&dir);
        if let Some(b) = bounds.iter().find(|b| !b.completed) {
            return b.volume_index.max(1);
        }
        let next = bounds
            .iter()
            .map(|b| b.volume_index)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        if next >= 1 {
            next
        } else {
            resolve_arc_outline_volume(&dir, None).max(1)
        }
    }

    fn handoff_plot_title(&self, project: &str, volume: u32) -> String {
        let dir = project_dir(&self.roots.projects_root, project);
        let name = load_volume_bounds(&dir)
            .into_iter()
            .find(|b| b.volume_index == volume)
            .map(|b| b.name)
            .unwrap_or_default();
        let name = name.trim().to_string();
        if name.is_empty() {
            format!("第{volume}卷开局")
        } else {
            format!("第{volume}卷·{name}·开局")
        }
    }

    /// Offer next volume-handoff action (arc outline / design+activate plot / sync).
    async fn offer_volume_handoff_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        phase: VolumePhase,
        failure_hint: Option<&str>,
    ) -> Result<bool> {
        if !self.features.enforce_volume_phase() || project.is_empty() {
            return Ok(false);
        }
        match phase {
            VolumePhase::AwaitingSync => {
                let dir = project_dir(&self.roots.projects_root, project);
                if let Some(vol) = recover_false_volume_end(&dir) {
                    tracing::info!(
                        %project,
                        volume = vol,
                        "skip volume-sync gate: false volume-end recovered"
                    );
                    if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                        t.pending_volume_sync = None;
                        t.pending_volume_handoff = None;
                    }
                    let _ = self.persist_thread(thread_id).await;
                    return Ok(false);
                }
                let bounds = load_volume_bounds(&dir);
                let ended = bounds.iter().rev().find(|b| b.completed).cloned();
                let (volume, name) = ended
                    .map(|b| (b.volume_index, b.name))
                    .unwrap_or((1, String::new()));
                let label = if name.is_empty() {
                    format!("第{volume}卷")
                } else {
                    format!("第{volume}卷「{name}」")
                };
                let prompt = format!("{label}已结束，是否同步设定库？");
                let options = self.volume_sync_options();
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.pending_volume_sync = Some(PendingVolumeSync {
                        project: project.to_string(),
                        volume,
                        name,
                        deferred_setting_blocker: None,
                    });
                    t.pending_volume_handoff = None;
                    t.pending_chapter_next = None;
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
                return Ok(true);
            }
            VolumePhase::AwaitingNextArc | VolumePhase::AwaitingNextPlot => {}
            VolumePhase::DraftingVolume => return Ok(false),
        }
        let volume = self.handoff_target_volume(project);
        let title = self.handoff_plot_title(project, volume);
        let phase_s = phase.as_str().to_string();
        let (gate, base_prompt) = match phase {
            VolumePhase::AwaitingNextArc => (
                "volume_handoff_arc",
                format!("卷间交接：请先设计第{volume}卷卷纲，再开剧情卡写章。"),
            ),
            VolumePhase::AwaitingNextPlot => (
                "volume_handoff_plot",
                format!("卷间交接：第{volume}卷卷纲已就绪，请设计并激活开局剧情卡。"),
            ),
            _ => return Ok(false),
        };
        let prompt = if let Some(hint) = failure_hint.filter(|s| !s.is_empty()) {
            format!("上次未成功：{hint}\n\n{base_prompt}")
        } else {
            base_prompt
        };
        let options = self.gates.options(gate);
        if options.is_empty() {
            return Ok(false);
        }
        // Same open gate without a new failure → do not stack another RequestUserInput.
        if failure_hint.is_none() {
            let guard = self.threads.read().await;
            if let Some(p) = guard
                .get(thread_id)
                .and_then(|t| t.pending_volume_handoff.as_ref())
            {
                if p.project == project && p.phase == phase_s && p.volume == volume {
                    return Ok(true);
                }
            }
        }
        let last_error = failure_hint.unwrap_or("").to_string();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_volume_handoff = Some(PendingVolumeHandoff {
                project: project.to_string(),
                phase: phase_s,
                volume,
                title,
                last_error,
            });
            t.pending_volume_sync = None;
            t.pending_chapter_next = None;
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

    async fn maybe_offer_volume_handoff(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if !self.features.enforce_volume_phase() {
            return Ok(false);
        }
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(false);
        }
        let dir = project_dir(&self.roots.projects_root, &project);
        let phase = data
            .get("volume_phase")
            .and_then(|v| v.as_str())
            .and_then(VolumePhase::parse)
            .unwrap_or_else(|| resolve_volume_phase(&dir));
        let blocked = data.get("blocked").and_then(|v| v.as_bool()) == Some(true);
        let offer_flag = data
            .get("offer_volume_handoff")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let should = match tool_name {
            "continue_writing" | "revise_chapter" | "design_plot" => {
                blocked
                    && matches!(
                        phase,
                        VolumePhase::AwaitingSync
                            | VolumePhase::AwaitingNextArc
                            | VolumePhase::AwaitingNextPlot
                    )
            }
            "sync_volume" => {
                !blocked
                    && data.get("ok").and_then(|v| v.as_bool()) != Some(false)
                    && matches!(phase, VolumePhase::AwaitingNextArc)
            }
            "design_arc_outline" => {
                (!blocked && matches!(phase, VolumePhase::AwaitingNextPlot)) || offer_flag
            }
            _ => offer_flag,
        };
        if !should {
            return Ok(false);
        }
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_audit.is_some()
                    || t.pending_volume_audit.is_some()
                    || t.pending_setup.is_some()
                {
                    return Ok(false);
                }
            }
        }
        self.offer_volume_handoff_gate(thread_id, turn_id, &project, phase, None)
            .await
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
        let issues = self.extract_audit_issues(&json!({}), project, chapter);
        let queue_active = true;
        let decisions =
            build_fallback_audit_decisions(project, chapter, &issues, queue_active);
        t.pending_audit = Some(PendingAudit {
            project: project.to_string(),
            chapter,
            kind: AuditGateKind::Content,
            issues,
            decision_options: decisions,
            awaiting_offer: false,
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
            if needs_setup {
                if let Some(next) = resolve_setup_next_step(&dir) {
                    if matches!(
                        next,
                        SetupNextStep::Confirm | SetupNextStep::NeedBible
                    ) && resolve_setup_phase(&dir) == SetupPhase::AwaitingConfirm
                    {
                        tracing::info!(
                            %project,
                            next = next.as_str(),
                            "reconcile setup → pending_setup"
                        );
                        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                            t.pending_setup = Some(PendingSetup {
                                project: project.clone(),
                                next: next.as_str().to_string(),
                            });
                        }
                    }
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
                if let Some(vol) = recover_false_volume_end(&dir) {
                    tracing::info!(
                        %project,
                        volume = vol,
                        "reconcile: false volume-end → drafting_volume"
                    );
                    if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                        t.pending_volume_sync = None;
                        t.pending_volume_handoff = None;
                    }
                } else {
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
                            deferred_setting_blocker: None,
                        });
                    }
                }
            }

            let needs_handoff = {
                let guard = self.threads.read().await;
                guard.get(thread_id).map(|t| {
                    t.pending_volume_handoff.is_none()
                        && t.pending_volume_sync.is_none()
                        && t.pending_setup.is_none()
                        && t.pending_audit.is_none()
                }).unwrap_or(false)
            };
            let phase = resolve_volume_phase(&dir);
            if needs_handoff
                && matches!(
                    phase,
                    VolumePhase::AwaitingNextArc | VolumePhase::AwaitingNextPlot
                )
            {
                let volume = self.handoff_target_volume(&project);
                let title = self.handoff_plot_title(&project, volume);
                tracing::info!(
                    %project,
                    phase = phase.as_str(),
                    volume,
                    "reconcile volume_phase handoff → pending_volume_handoff"
                );
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.pending_volume_handoff = Some(PendingVolumeHandoff {
                        project: project.clone(),
                        phase: phase.as_str().into(),
                        volume,
                        title,
                        last_error: String::new(),
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
        // Drop duplicate option clicks so RegularTask cannot auto-start another steer
        // after「复审通过」clears the human gate.
        self.clear_queued_inputs(thread_id).await;
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

    /// Put the compact issue checklist into the chat bubble *before* options.
    /// Skips if this turn already has a checklist (prepare + open must not double-paste).
    async fn emit_audit_report_before_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        chapter: u32,
        data: &Value,
    ) {
        let mut brief = format_audit_choice_brief(data, chapter);
        // Reaudit tool data sometimes omits issues; fall back to on-disk audit.json.
        if !brief.contains("### 问题清单") {
            if let Some(project) = data
                .get("project")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                if let Some(disk) =
                    load_chapter_audit_brief(&self.roots.projects_root, project, chapter)
                {
                    brief = disk;
                }
            }
        }
        if brief.trim().is_empty() {
            return;
        }
        // Deduplicate: same turn already showing「问题清单」→ update in place, no second bubble.
        let existing_id = {
            let guard = self.threads.read().await;
            guard.get(thread_id).and_then(|t| {
                t.ui_turns.as_array().and_then(|turns| {
                    turns.iter().find(|tr| {
                        tr.get("id").and_then(|v| v.as_str()) == Some(turn_id)
                    }).and_then(|tr| {
                        tr.get("items").and_then(|v| v.as_array()).and_then(|items| {
                            items.iter().rev().find_map(|it| {
                                if it.get("type").and_then(|v| v.as_str()) != Some("agent_message")
                                {
                                    return None;
                                }
                                let text = it.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                if text.contains("### 问题清单")
                                    || text.contains(&format!("第{chapter}章审校未通过"))
                                {
                                    it.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())
                                } else {
                                    None
                                }
                            })
                        })
                    })
                })
            })
        };
        let is_update = existing_id.is_some();
        let item_id = existing_id.unwrap_or_else(|| new_id("item"));
        if !is_update {
            self.emit_to_thread(
                thread_id,
                EventMsg::ItemStarted {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item: TurnItem::AgentMessage {
                        id: item_id.clone(),
                        text: String::new(),
                        status: ItemStatus::InProgress,
                    },
                },
            )
            .await;
            self.emit_to_thread(
                thread_id,
                EventMsg::AgentMessageContentDelta {
                    thread_id: thread_id.to_string(),
                    turn_id: turn_id.to_string(),
                    item_id: item_id.clone(),
                    delta: brief.clone(),
                },
            )
            .await;
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: item_id.clone(),
                    text: brief.clone(),
                    status: ItemStatus::Completed,
                },
            },
        )
        .await;
        // UI-only: do not push checklist into chat messages — that caused the model
        // to restate it and the turn-closing path to paste it onto the main bubble.
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.ui_turns = upsert_ui_agent_message(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &item_id,
                &brief,
            );
        }
    }

    fn extract_audit_issues(&self, data: &Value, project: &str, chapter: u32) -> Vec<Value> {
        let mut issues = data
            .get("issues")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if issues.is_empty() {
            let path = self
                .roots
                .projects_root
                .join(project)
                .join("chapters")
                .join(format!("{chapter:03}"))
                .join("audit.json");
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    issues = v
                        .get("issues")
                        .and_then(|x| x.as_array())
                        .cloned()
                        .unwrap_or_default();
                }
            }
        }
        with_issue_ids(issues)
    }

    /// Open content-audit decision card (per-issue options or Studio-offered).
    async fn open_content_audit_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        chapter: u32,
        data: &Value,
        queue_active: bool,
        offered: Option<Vec<AuditDecisionOption>>,
        emit_brief: bool,
    ) -> Result<()> {
        let issues = self.extract_audit_issues(data, project, chapter);
        let decisions = if let Some(o) = offered.filter(|v| !v.is_empty()) {
            o
        } else {
            build_fallback_audit_decisions(project, chapter, &issues, queue_active)
        };
        let options = decisions_to_ui_options(&decisions);
        let prompt = audit_fail_prompt(chapter, &issues, queue_active);
        if emit_brief {
            self.emit_audit_report_before_gate(thread_id, turn_id, chapter, data)
                .await;
        }
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_audit = Some(PendingAudit {
                project: project.to_string(),
                chapter,
                kind: AuditGateKind::Content,
                issues: issues.clone(),
                decision_options: decisions,
                awaiting_offer: false,
            });
            t.pending_volume_audit = None;
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
        Ok(())
    }

    /// Prepare fail state for Studio to call `offer_decisions` (no gate yet).
    async fn prepare_audit_fail_for_studio(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        chapter: u32,
        data: &Value,
    ) -> Result<()> {
        let issues = self.extract_audit_issues(data, project, chapter);
        self.emit_audit_report_before_gate(thread_id, turn_id, chapter, data)
            .await;
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_audit = Some(PendingAudit {
                project: project.to_string(),
                chapter,
                kind: AuditGateKind::Content,
                issues,
                decision_options: vec![],
                awaiting_offer: true,
            });
            t.pending_volume_audit = None;
            t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
        }
        let _ = self.persist_thread(thread_id).await;
        Ok(())
    }

    /// Apply Studio `offer_decisions` → validated open_gate.
    async fn apply_offer_decisions(
        &self,
        thread_id: &str,
        turn_id: &str,
        args: &Value,
        data: &Value,
    ) -> Result<OfferApplyResult> {
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("kind").and_then(|v| v.as_str()))
            .unwrap_or("audit");
        if kind == "studio_next" {
            return self
                .apply_studio_next_offer(thread_id, turn_id, args, data)
                .await;
        }
        let pending = {
            let guard = self.threads.read().await;
            guard.get(thread_id).and_then(|t| t.pending_audit.clone())
        };
        let Some(pending) = pending else {
            return Ok(OfferApplyResult::Ignored);
        };
        if pending.kind != AuditGateKind::Content {
            return Ok(OfferApplyResult::Ignored);
        }
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .unwrap_or(pending.project.as_str());
        let chapter = args
            .get("chapter")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(pending.chapter);
        let raw_options = data
            .get("options")
            .or_else(|| args.get("options"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let queue_active = load_audit_queue(&self.roots.projects_root, project).is_some();
        let decisions = match validate_offered_decisions(
            project,
            chapter,
            &pending.issues,
            &raw_options,
            queue_active,
        ) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "offer_decisions rejected; keeping fallback path");
                return Ok(OfferApplyResult::Rejected(e));
            }
        };
        let mut data = data.clone();
        if data.get("issues").is_none() {
            data["issues"] = json!(pending.issues);
        }
        data["project"] = json!(project);
        data["chapter"] = json!(chapter);
        self.open_content_audit_gate(
            thread_id,
            turn_id,
            project,
            chapter,
            &data,
            queue_active,
            Some(decisions),
            false,
        )
        .await?;
        Ok(OfferApplyResult::OpenedGate)
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
        // Infrastructure / parse failures: offer retry, never「局部修订」loops.
        if audit_failure_is_meta_only(data) {
            self.emit_audit_report_before_gate(thread_id, turn_id, chapter, data)
                .await;
            let options = self.gates.options("audit_infra");
            let prompt = format!(
                "第{chapter}章一致性审计基础设施失败（空响应或无法解析）。请重试，勿按正文问题局部修订："
            );
            if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                t.pending_audit = Some(PendingAudit {
                    project: project.clone(),
                    chapter,
                    kind: AuditGateKind::Infra,
                    issues: vec![],
                    decision_options: vec![],
                    awaiting_offer: false,
                });
                t.pending_volume_audit = None;
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
            return Ok(true);
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
        let queue_active = data.get("queue_active").and_then(|v| v.as_bool()).unwrap_or(false)
            || load_audit_queue(&self.roots.projects_root, &project).is_some();
        // Compact checklist + per-issue decision card (not the old binary revise/accept).
        self.open_content_audit_gate(
            thread_id,
            turn_id,
            &project,
            chapter,
            data,
            queue_active,
            None,
            true,
        )
        .await?;
        if queue_active {
            self.emit_todos_from_data(thread_id, data).await;
        }
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
        let _pending = pending?;
        let _ = t;
        // volume_audit situational cards now use pending_studio_next.
        None
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
        let next = SetupNextStep::parse(&pending.next).unwrap_or(SetupNextStep::Confirm);
        if let Some(resolved) = self
            .gates
            .resolve_setup_progress_visible(t, &pending.project, next.gate_name())
        {
            return match resolved {
                GateResolve::Tool { name, args } => Some((name, args)),
                GateResolve::DismissGate => Some(("__dismiss_setup".into(), json!({}))),
                GateResolve::SkipVolume
                | GateResolve::SteerInstructions { .. }
                | GateResolve::ApplyMutation
                | GateResolve::DiscardMutation
                | GateResolve::SyncImpact
                | GateResolve::SkipImpact
                | GateResolve::ActivatePlotWrite
                | GateResolve::ContinueStudio => None,
            };
        }
        // Free-text while needing brief → lock_brief.
        if next == SetupNextStep::NeedBrief
            && t.chars().count() >= 8
            && !self.gates.is_known_token(t)
        {
            return Some((
                "lock_brief".into(),
                json!({ "project": pending.project, "brief": t }),
            ));
        }
        None
    }

    async fn parse_volume_handoff_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_volume_handoff.clone())?;
        match self.gates.resolve_volume_handoff_visible(
            t,
            &pending.project,
            &pending.phase,
            pending.volume,
            &pending.title,
        )? {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume
            | GateResolve::SteerInstructions { .. }
            | GateResolve::ApplyMutation
            | GateResolve::DiscardMutation
            | GateResolve::SyncImpact
            | GateResolve::SkipImpact
            | GateResolve::ActivatePlotWrite
            | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
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
            return match self.gates.resolve_volume_visible(
                t,
                &pending.project,
                pending.volume,
            )? {
                GateResolve::Tool { name, args } => Some((name, args)),
                GateResolve::SkipVolume => Some(("__skip_volume_sync".into(), json!({}))),
                GateResolve::SteerInstructions { .. }
                | GateResolve::ApplyMutation
                | GateResolve::DiscardMutation
                | GateResolve::SyncImpact
                | GateResolve::SkipImpact
                | GateResolve::ActivatePlotWrite
                | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
            };
        }

        // Explicit manual sync without an open gate — phrases from config/policies.yaml.
        if !self.with_policies(|p| p.is_manual_volume_sync(t)) {
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
        let infra = pending.kind == AuditGateKind::Infra;
        // Prefer dynamic per-issue decisions when present.
        if !infra && !pending.decision_options.is_empty() {
            if let Some(pair) = resolve_decision_pick(t, &pending.decision_options) {
                return Some(pair);
            }
        }
        match self.gates.resolve_audit(
            t,
            &pending.project,
            pending.chapter,
            queue_active,
            infra,
        )? {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume
            | GateResolve::ApplyMutation
            | GateResolve::DiscardMutation
            | GateResolve::SyncImpact
            | GateResolve::SkipImpact
            | GateResolve::ActivatePlotWrite
            | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
            GateResolve::SteerInstructions { instructions } => {
                // Infra gate must not turn free-text into local-patch revise.
                if infra {
                    None
                } else {
                    Some((
                        "steer_run".into(),
                        json!({
                            "project": pending.project,
                            "chapter": pending.chapter,
                            "choice": "revise",
                            "instructions": instructions,
                        }),
                    ))
                }
            }
        }
    }

    /// Snapshot for HTTP restore (messages + UI turns).
    pub async fn thread_snapshot(&self, thread_id: &str) -> Option<serde_json::Value> {
        let turn_active = self.active_turn_id(thread_id).await.is_some();
        // While a turn is running (e.g. revise + reaudit), do not revive orphan gates from
        // disk — that re-opens ApprovalOptions and unlocks chat mid-flight.
        if !turn_active {
            self.reconcile_orphan_audit_queue(thread_id).await;
            self.reconcile_phase_gates(thread_id).await;
            // Ensure ui_turns carry the open gate so HTTP restore shows ApprovalOptions.
            if self.thread_awaiting_human(thread_id).await {
                self.sync_pending_gate_into_ui(thread_id, false).await;
            }
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
        // Hide open_gate while working so clients cannot re-attach a stale choice card.
        let open_gate = if turn_active {
            None
        } else {
            self.build_open_gate_dto(t, queue.is_some())
        };
        Some(serde_json::json!({
            "threadId": thread_id,
            "project": t.summary.project,
            "messages": t.messages,
            "turns": t.ui_turns,
            "pending_audit": if turn_active { Value::Null } else { json!(t.pending_audit) },
            "pending_volume_sync": t.pending_volume_sync,
            "pending_setup": t.pending_setup,
            "pending_volume_handoff": t.pending_volume_handoff,
            "pending_chapter_next": t.pending_chapter_next,
            "pending_chapter_order": t.pending_chapter_order,
            "pending_plot_write": t.pending_plot_write,
            "pending_setting_blocker": t.pending_setting_blocker,
            "pending_mutation": t.pending_mutation,
            "pending_mutation_followup": t.pending_mutation_followup,
            "pending_studio_next": t.pending_studio_next,
            "awaiting_studio_next": t.awaiting_studio_next,
            "pending_impact": t.pending_impact,
            "pending_audit_queue": queue,
            "turn_active": turn_active,
            // Prefer this over client-side hardcoded buttons (gates.yaml is source of truth).
            "open_gate": open_gate,
        }))
    }

    /// Build restore gate {prompt, options} from GateCatalog — no frontend phrase tables.
    fn build_open_gate_dto(&self, t: &ThreadState, queue_active: bool) -> Option<Value> {
        if let Some(m) = &t.pending_mutation {
            return Some(json!({
                "kind": "confirm_mutation",
                "prompt": format!("待确认：{}", m.summary),
                "options": self.gates.mutation_confirm_options(),
                "preview": m.preview,
                "mutation_kind": m.mutation_kind,
            }));
        }
        if let Some(imp) = &t.pending_impact {
            return Some(json!({
                "kind": "confirm_impact",
                "prompt": format!(
                    "设定/结构已更新，发现 {} 处可能受影响的依赖位点。是否自动同步修正？",
                    imp.hits.len()
                ),
                "options": self.gates.impact_confirm_options(),
                "summary_markdown": imp.summary_markdown,
                "hits": imp.hits,
                "entity_gaps_count": imp.entity_gaps_count,
            }));
        }
        if let Some(sn) = &t.pending_studio_next {
            return Some(json!({
                "kind": "studio_next",
                "prompt": sn.prompt,
                "options": sn.options.iter().map(studio_next::StudioNextOption::to_ui_option).collect::<Vec<_>>(),
            }));
        }
        if let Some(o) = &t.pending_chapter_order {
            return Some(json!({
                "kind": "chapter_order",
                "prompt": format!("不能跳章。当前应写第{}章，请选择：", o.next_chapter),
                "options": self.gates.chapter_order_options(o.next_chapter),
            }));
        }
        if let Some(a) = &t.pending_audit {
            if a.awaiting_offer {
                return None;
            }
            let (kind, options, prompt) = match a.kind {
                AuditGateKind::Infra => (
                    "audit_infra",
                    self.gates.options("audit_infra"),
                    format!(
                        "第{}章一致性审计基础设施失败（空响应或无法解析）。请重试，勿按正文问题局部修订：",
                        a.chapter
                    ),
                ),
                AuditGateKind::Content => {
                    let decisions = if a.decision_options.is_empty() {
                        build_fallback_audit_decisions(
                            &a.project,
                            a.chapter,
                            &a.issues,
                            queue_active,
                        )
                    } else {
                        a.decision_options.clone()
                    };
                    (
                        "audit",
                        decisions_to_ui_options(&decisions),
                        audit_fail_prompt(a.chapter, &a.issues, queue_active),
                    )
                }
            };
            return Some(json!({
                "kind": kind,
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
            let mut prompt = format!("{label}已结束。请同步设定库，并确认卷记忆。");
            if v.deferred_setting_blocker.is_some() {
                prompt.push_str("（完成后将处理设定 BLOCKER）");
            }
            return Some(json!({
                "kind": "volume_sync",
                "prompt": prompt,
                "options": options,
            }));
        }
        if let Some(h) = &t.pending_volume_handoff {
            let (kind, prompt) = match h.phase.as_str() {
                "awaiting_next_plot" => (
                    "volume_handoff_plot",
                    format!(
                        "卷间交接：第{}卷卷纲已就绪，请设计并激活开局剧情卡。",
                        h.volume
                    ),
                ),
                _ => (
                    "volume_handoff_arc",
                    format!(
                        "卷间交接：请先设计第{}卷卷纲，再开剧情卡写章。",
                        h.volume
                    ),
                ),
            };
            return Some(json!({
                "kind": kind,
                "prompt": prompt,
                "options": self.gates.options(kind),
            }));
        }
        if let Some(s) = &t.pending_setup {
            let next = SetupNextStep::parse(&s.next).unwrap_or(SetupNextStep::Confirm);
            let kind = next.gate_name();
            return Some(json!({
                "kind": kind,
                "prompt": next.prompt(&s.project),
                "options": self.gates.options(kind),
            }));
        }
        if let Some(c) = &t.pending_chapter_next {
            if c.suggest_next.is_some() {
                return None;
            }
            let options = self
                .gates
                .chapter_next_options(c.published, c.plot_accept_open);
            let prompt = if c.published {
                chapter_next_published_prompt(c.chapter, c.plot_accept_open)
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
                    || t.pending_volume_handoff.is_some()
                    || t.pending_chapter_next.is_some()
                    || t.pending_chapter_order.is_some()
                    || t.pending_mutation.is_some()
                    || t.pending_impact.is_some()
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
        // Situational volume_audit cards come from offer_decisions(studio_next).
        self.gates.studio_next_fallback_options()
    }

    pub fn chapter_next_options(
        &self,
        published: bool,
        plot_accept_open: bool,
    ) -> Vec<UserInputOption> {
        self.gates.chapter_next_options(published, plot_accept_open)
    }

    /// After a passed audit/reaudit: volume gates first, then「继续创作」/「修正本章」.
    async fn maybe_offer_after_audit_pass(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if self
            .maybe_offer_volume_sync(thread_id, turn_id, tool_name, args, data)
            .await?
        {
            return Ok(true);
        }
        if self
            .maybe_offer_volume_handoff(thread_id, turn_id, tool_name, args, data)
            .await?
        {
            return Ok(true);
        }
        if self
            .maybe_offer_volume_audit(thread_id, turn_id, tool_name, args, data)
            .await?
        {
            return Ok(true);
        }
        if self
            .maybe_offer_setting_blocker(thread_id, turn_id, tool_name, args, data)
            .await?
        {
            return Ok(true);
        }
        self.maybe_offer_chapter_next(thread_id, turn_id, tool_name, args, data)
            .await
    }

    /// After chapter write/revise/audit: published →「继续创作」; content-rule block →「修正本章」.
    async fn maybe_offer_chapter_next(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if !matches!(
            tool_name,
            "continue_writing" | "revise_chapter" | "audit_chapter" | "audit_chapters"
        ) {
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
                    || t.pending_volume_handoff.is_some()
                    || t.pending_mutation.is_some()
                    || t.pending_impact.is_some()
                    || t.pending_chapter_order.is_some()
                    || t.pending_setting_blocker.is_some()
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
        let length_blocked = is_length_publish_blocked(data);
        let plot_accept_open = published
            && data.get("plot_accept_passed").and_then(|v| v.as_bool()) == Some(false);
        // Clean publish → continue next chapter.
        // Hard-rule / length block → revise this chapter.
        // Other unpublished cases (consistency / P0) use the audit gate instead.
        if !published && !content_blocked && !length_blocked {
            return Ok(false);
        }
        let options = self.chapter_next_options(published, plot_accept_open);
        if options.is_empty() {
            return Ok(false);
        }
        let detail = data
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let report = data.get("report").and_then(|v| v.as_str()).unwrap_or("");
        let prompt = if published {
            chapter_next_published_prompt(chapter, plot_accept_open)
        } else if length_blocked {
            chapter_next_length_prompt(chapter, detail)
        } else {
            chapter_next_hard_rule_prompt(chapter, detail)
        };
        let revise_instructions = if length_blocked {
            Some(length_revise_instructions(&self.roots.config_root, detail))
        } else if content_blocked {
            Some(hard_rule_revise_instructions(
                &self.roots.projects_root,
                &self.roots.config_root,
                &project,
                chapter,
                detail,
                report,
            ))
        } else if plot_accept_open {
            let rationale = data
                .get("plot_accept_rationale")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            Some(if rationale.is_empty() {
                "对照剧情卡收束条件补写本章缺口；只改收束相关段落，勿整章重写。".into()
            } else {
                format!(
                    "对照剧情卡收束条件补写本章缺口：{rationale}；只改收束相关段落，勿整章重写。"
                )
            })
        } else {
            None
        };
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_chapter_next = Some(PendingChapterNext {
                project: project.clone(),
                chapter,
                published,
                plot_accept_open,
                suggest_next: None,
                revise_instructions,
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
        if pending.suggest_next.is_some() {
            // Legacy draft_exists pending — handled by studio_next now.
            return None;
        }
        match self.gates.resolve_chapter_next_visible(
            t,
            &pending.project,
            pending.chapter,
            pending.published,
            pending.plot_accept_open,
        )? {
            GateResolve::Tool { name, mut args } => {
                if name == "revise_chapter" {
                    if let Some(instr) = pending.revise_instructions {
                        if let Some(obj) = args.as_object_mut() {
                            obj.insert("instructions".into(), json!(instr));
                        }
                    }
                }
                Some((name, args))
            }
            GateResolve::SkipVolume
            | GateResolve::SteerInstructions { .. }
            | GateResolve::ApplyMutation
            | GateResolve::DiscardMutation
            | GateResolve::SyncImpact
            | GateResolve::SkipImpact
            | GateResolve::ActivatePlotWrite
            | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
        }
    }

    async fn parse_chapter_order_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_chapter_order.clone())?;
        match self
            .gates
            .resolve_chapter_order(t, &pending.project, pending.next_chapter)?
        {
            GateResolve::Tool { name, args } => Some((name, args)),
            GateResolve::SkipVolume
            | GateResolve::SteerInstructions { .. }
            | GateResolve::ApplyMutation
            | GateResolve::DiscardMutation
            | GateResolve::SyncImpact
            | GateResolve::SkipImpact
            | GateResolve::ActivatePlotWrite
            | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
        }
    }

    /// Returns `Some(("__apply_mutation"|"__discard_mutation", args))` when confirm gate matches.
    async fn parse_mutation_confirm_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<(String, Value)> {
        let t = text.trim();
        let pending = self
            .threads
            .read()
            .await
            .get(thread_id)
            .and_then(|th| th.pending_mutation.clone())?;
        match self.gates.resolve_mutation_confirm(t)? {
            GateResolve::ApplyMutation => Some((
                "__apply_mutation".into(),
                json!({
                    "mutation_id": pending.mutation_id,
                    "apply_tool": pending.apply_tool,
                    "apply_args": pending.apply_args,
                }),
            )),
            GateResolve::DiscardMutation => Some((
                "__discard_mutation".into(),
                json!({ "mutation_id": pending.mutation_id }),
            )),
            GateResolve::Tool { .. }
            | GateResolve::SkipVolume
            | GateResolve::SteerInstructions { .. }
            | GateResolve::SyncImpact
            | GateResolve::SkipImpact
            | GateResolve::ActivatePlotWrite
            | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
        }
    }

    async fn maybe_offer_mutation_confirm(
        &self,
        thread_id: &str,
        turn_id: &str,
        _tool_name: &str,
        _args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if data.get("needs_confirm").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(false);
        }
        let mutation_id = data
            .get("mutation_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let apply_tool = data
            .get("apply_tool")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let apply_args = data.get("apply_args").cloned().unwrap_or(json!({}));
        let summary = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("确认应用此次修改？")
            .to_string();
        let mutation_kind = data
            .get("mutation_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let preview = data.get("preview").cloned().unwrap_or(json!({}));
        let reaudit_after = data
            .get("reaudit_after")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if mutation_id.is_empty() || apply_tool.is_empty() {
            return Ok(false);
        }
        // Volume sync owns the turn; setup progress must yield to mutation preview
        // (design_master / arc / bible from the setup gate).
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_volume_sync.is_some() {
                    return Ok(false);
                }
            }
        }
        let options = self.gates.mutation_confirm_options();
        let prompt = format!("待确认：{summary}");
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            if matches!(
                apply_tool.as_str(),
                "design_master_outline" | "design_arc_outline"
            ) {
                t.outline_rewrite_active = true;
            }
            t.pending_mutation = Some(PendingMutation {
                mutation_id,
                mutation_kind,
                summary: summary.clone(),
                apply_tool,
                apply_args,
                preview: preview.clone(),
                reaudit_after,
            });
            // Mutation card must be the only human gate until apply/discard.
            t.pending_setup = None;
            t.pending_chapter_next = None;
            t.pending_chapter_order = None;
            t.pending_mutation_followup = None;
            t.pending_studio_next = None;
            t.awaiting_studio_next = None;
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &options,
            );
            // Attach preview diffs into the turn for Web DraftPatchCard / MutationPreviewCard.
            if let Some(diffs) = preview.get("diffs").cloned() {
                t.ui_turns = attach_ui_mutation_preview(
                    std::mem::take(&mut t.ui_turns),
                    turn_id,
                    &preview,
                    diffs,
                );
            } else if preview.get("markdown").is_some() || preview.get("fields").is_some() {
                t.ui_turns = attach_ui_mutation_preview(
                    std::mem::take(&mut t.ui_turns),
                    turn_id,
                    &preview,
                    json!([]),
                );
            }
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

    fn is_structure_mutation_tool(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "upsert_setting"
                | "supplement_setting"
                | "design_entity"
                | "delete_entity"
                | "design_master_outline"
                | "design_arc_outline"
        )
    }

    /// After setting/outline apply with no higher-priority gate — keep multi-step plans alive.
    async fn maybe_offer_mutation_followup(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        let _ = turn_id;
        if !Self::is_structure_mutation_tool(tool_name) {
            return Ok(false);
        }
        if data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(false);
        }
        if data.get("blocked").and_then(|v| v.as_bool()) == Some(true) {
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
        let outline_rewrite = matches!(
            tool_name,
            "design_master_outline" | "design_arc_outline" | "upsert_setting"
        );
        self.request_studio_next(
            thread_id,
            &project,
            &format!("「{tool_name}」已落盘，请给出未完成计划的下一步"),
            studio_next::StudioNextContext::Mutation {
                apply_tool: tool_name.to_string(),
            },
            outline_rewrite,
        )
        .await
    }

    /// After a mutation apply with `impact_source`, scan dependents and offer cascade gate.
    async fn maybe_offer_impact_cascade(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if !self.features.impact_cascade() {
            return Ok(false);
        }
        if data.get("blocked").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(false);
        }
        if data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(false);
        }
        let Some(source) = ImpactSource::from_tool_data(data) else {
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
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_mutation.is_some() || t.pending_volume_sync.is_some() {
                    return Ok(false);
                }
            }
        }
        let dir = project_dir(&self.roots.projects_root, &project);
        let report = scan_impact(&dir, &source);
        if self.features.impact_llm_refine() {
            // Reserved: optional LLM pass to drop false-positive hits (default off).
            tracing::debug!(%project, "impact_llm_refine enabled; using deterministic hits only");
        }
        // Draft source: only refresh gaps notice when non-zero.
        if report.is_empty() {
            if source.kind == novelx_pipeline::ImpactSourceKind::Draft
                && report.entity_gaps_count > 0
            {
                // Soft notice only — no gate.
                tracing::info!(
                    %project,
                    gaps = report.entity_gaps_count,
                    "impact: draft apply refreshed entity_gaps (no cascade hits)"
                );
            }
            return Ok(false);
        }
        let summary_markdown = report.summary_markdown(12);
        let hits: Vec<Value> = report.hits.iter().map(ImpactHit::to_json).collect();
        let prompt = format!(
            "「{tool_name}」已落盘。发现 {} 处可能受影响的依赖位点。是否自动同步修正？\n\n{summary_markdown}",
            hits.len()
        );
        let options = self.gates.impact_confirm_options();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_impact = Some(PendingImpact {
                project: project.clone(),
                source: source.to_json(),
                hits: hits.clone(),
                summary_markdown: summary_markdown.clone(),
                entity_gaps_count: report.entity_gaps_count,
                resume_tool: tool_name.to_string(),
                resume_args: args.clone(),
                resume_data: data.clone(),
            });
            // Impact owns the next choice; restore setup/handoff after sync/skip via resume_*.
            t.pending_setup = None;
            t.pending_chapter_next = None;
            t.pending_chapter_order = None;
            t.pending_volume_handoff = None;
            t.pending_mutation_followup = None;
            t.pending_studio_next = None;
            t.awaiting_studio_next = None;
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

    async fn parse_impact_confirm_op(&self, thread_id: &str, text: &str) -> Option<String> {
        let t = text.trim();
        {
            let guard = self.threads.read().await;
            guard.get(thread_id)?.pending_impact.as_ref()?;
        }
        match self.gates.resolve_impact_confirm(t)? {
            GateResolve::SyncImpact => Some("sync_impact".into()),
            GateResolve::SkipImpact => Some("skip_impact".into()),
            GateResolve::Tool { .. }
            | GateResolve::SkipVolume
            | GateResolve::SteerInstructions { .. }
            | GateResolve::ApplyMutation
            | GateResolve::DiscardMutation
            | GateResolve::ActivatePlotWrite
            | GateResolve::DismissGate
            | GateResolve::ContinueStudio => None,
        }
    }

    /// Cascade-revise dependent targets with confirm_skip (impact gate already approved).
    async fn run_impact_cascade(
        &self,
        thread_id: &str,
        turn_id: &str,
        pending: &PendingImpact,
    ) -> Result<String> {
        let Some(source) =
            ImpactSource::from_tool_data(&json!({ "impact_source": pending.source }))
        else {
            return Ok("无法解析影响源，已跳过级联。".into());
        };
        let mut hits: Vec<ImpactHit> = pending
            .hits
            .iter()
            .filter_map(|h| serde_json::from_value::<ImpactHit>(h.clone()).ok())
            .collect();
        if hits.is_empty() {
            // Re-scan if persisted hits failed to deserialize.
            let dir = project_dir(&self.roots.projects_root, &pending.project);
            hits = scan_impact(&dir, &source).hits;
        }
        hits = consolidate_impact_hits(hits);
        let mut lines = Vec::new();
        let mut revised_chapters: Vec<u32> = Vec::new();
        for hit in hits {
            // Plot: list-only in P0/P1 (no auto design_plot).
            if hit.target_kind == ImpactTargetKind::Plot {
                lines.push(format!(
                    "- 跳过剧情卡 {}（请手动核对）",
                    hit.target_ref
                ));
                continue;
            }
            let instructions = cascade_revise_instructions(&source, &hit);
            let (tool, mut args) = match hit.target_kind {
                ImpactTargetKind::Draft => {
                    let ch = hit.chapter.unwrap_or(0);
                    if ch == 0 {
                        lines.push(format!("- 跳过正文 {}（缺章号）", hit.target_ref));
                        continue;
                    }
                    (
                        "revise_chapter",
                        json!({
                            "project": pending.project,
                            "chapter": ch,
                            "instructions": instructions,
                        }),
                    )
                }
                ImpactTargetKind::Outline => {
                    let ch = hit.chapter.unwrap_or(0);
                    if ch == 0 {
                        lines.push(format!("- 跳过章纲 {}（缺章号）", hit.target_ref));
                        continue;
                    }
                    (
                        "revise_outline",
                        json!({
                            "project": pending.project,
                            "chapter": ch,
                            "instructions": instructions,
                            "force": true,
                        }),
                    )
                }
                ImpactTargetKind::ArcOutline => {
                    let vol = hit.volume.unwrap_or(0);
                    if vol == 0 {
                        lines.push(format!("- 跳过卷纲 {}（缺卷号）", hit.target_ref));
                        continue;
                    }
                    (
                        "design_arc_outline",
                        json!({
                            "project": pending.project,
                            "arc": vol,
                            "brief": instructions,
                            "force": true,
                        }),
                    )
                }
                ImpactTargetKind::MasterOutline => (
                    "design_master_outline",
                    json!({
                        "project": pending.project,
                        "brief": instructions,
                        "force": true,
                    }),
                ),
                ImpactTargetKind::Entity => {
                    let dir = project_dir(&self.roots.projects_root, &pending.project);
                    let (kind, name) = resolve_entity_hit_name(&dir, &hit.target_ref);
                    if name.is_empty() {
                        lines.push(format!("- 跳过实体 {}", hit.target_ref));
                        continue;
                    }
                    (
                        "design_entity",
                        json!({
                            "project": pending.project,
                            "kind": kind,
                            "name": name,
                            "brief": instructions,
                            "force": true,
                        }),
                    )
                }
                ImpactTargetKind::Bible => (
                    "upsert_setting",
                    json!({
                        "project": pending.project,
                        "topic": source.id,
                        "content": instructions,
                        "force": true,
                    }),
                ),
                ImpactTargetKind::Plot => continue,
            };
            args = novelx_tools::with_confirm_skip(args);
            match self
                .run_one_tool(thread_id, turn_id, tool, &args.to_string())
                .await
            {
                Ok((output, data)) => {
                    if data.get("blocked").and_then(|v| v.as_bool()) == Some(true) {
                        lines.push(format!(
                            "- {tool} {} 被拦截：{}",
                            hit.target_ref,
                            data.get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("blocked")
                        ));
                    } else {
                        let short: String = output.chars().take(120).collect();
                        lines.push(format!("- 已同步 {}：{short}", hit.target_ref));
                        if hit.target_kind == ImpactTargetKind::Draft {
                            if let Some(ch) = hit.chapter {
                                revised_chapters.push(ch);
                            }
                        }
                    }
                }
                Err(e) => {
                    lines.push(format!("- 同步 {} 失败：{e}", hit.target_ref));
                }
            }
        }
        revised_chapters.sort_unstable();
        revised_chapters.dedup();
        for ch in revised_chapters.into_iter().take(3) {
            let audit_args = json!({ "project": pending.project, "chapter": ch });
            let _ = self
                .run_one_tool(
                    thread_id,
                    turn_id,
                    "audit_chapter",
                    &audit_args.to_string(),
                )
                .await;
            lines.push(format!("- 已复审第{ch}章"));
        }
        if lines.is_empty() {
            Ok("无自动同步目标（或仅剧情卡提示）。".into())
        } else {
            Ok(lines.join("\n"))
        }
    }

    async fn maybe_offer_chapter_order(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        if tool_name != "continue_writing" {
            return Ok(false);
        }
        if data.get("blocked").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(false);
        }
        let reason = data.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        if reason != "chapter_skip" && reason != "chapter_gap" {
            return Ok(false);
        }
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let next_chapter = data
            .get("next_chapter")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if project.is_empty() || next_chapter == 0 {
            return Ok(false);
        }
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_mutation.is_some()
                    || t.pending_impact.is_some()
                    || t.pending_volume_sync.is_some()
                    || t.pending_volume_handoff.is_some()
                    || t.pending_setup.is_some()
                    || t.pending_audit.is_some()
                {
                    return Ok(false);
                }
            }
        }
        let options = self.gates.chapter_order_options(next_chapter);
        let prompt = format!("不能跳章。当前应写第{next_chapter}章，请选择：");
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_chapter_order = Some(PendingChapterOrder {
                project: project.clone(),
                next_chapter,
            });
            t.pending_chapter_next = None;
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

    async fn apply_expected_event_tool_side_effects(
        &self,
        thread_id: &str,
        tool_name: &str,
        data: &Value,
    ) {
        if tool_name != "resolve_expected_event" && tool_name != "review_expected_events" {
            return;
        }
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            if data.get("skip_once").and_then(|v| v.as_bool()) == Some(true) {
                if let Some(id) = data.get("event_id").and_then(|v| v.as_str()) {
                    if !t.skipped_expected_ids.iter().any(|s| s == id) {
                        t.skipped_expected_ids.push(id.to_string());
                    }
                }
            }
            if data.get("dismiss_expected_gate").and_then(|v| v.as_bool()) == Some(true)
                || data.get("approved").and_then(|v| v.as_bool()) == Some(true)
            {
                t.pending_expected_event = None;
            }
        }
    }

    async fn parse_expected_event_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<ExpectedEventGateOp> {
        let _ = (thread_id, text);
        None
    }

    async fn maybe_offer_expected_event(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        self.apply_expected_event_tool_side_effects(thread_id, tool_name, data)
            .await;

        let reason = data.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        let needs_choice = data.get("needs_user_choice").and_then(|v| v.as_bool()) == Some(true);
        let offer_volume_review =
            data.get("offer_expected_review").and_then(|v| v.as_bool()) == Some(true);

        let (kind, project, chapter, event_id, event_text, reason_text) = if tool_name
            == "continue_writing"
            && data.get("blocked").and_then(|v| v.as_bool()) == Some(true)
            && reason == "need_expected_review"
        {
            let project = data
                .get("project")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("project").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let chapter = data
                .get("chapter")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(0);
            (
                "need_review".to_string(),
                project,
                chapter,
                String::new(),
                String::new(),
                format!("第{chapter}章写前：有预处理预期硬条件已满足，需检阅或跳过"),
            )
        } else if (tool_name == "continue_writing"
            && data.get("blocked").and_then(|v| v.as_bool()) == Some(true)
            && reason == "expected_event_pending")
            || (tool_name == "review_expected_events" && needs_choice)
        {
            let project = data
                .get("project")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("project").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let chapter = data
                .get("chapter")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(0);
            let event_id = data
                .get("event_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let event_text = data
                .get("event_text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if event_id.is_empty() {
                return Ok(false);
            }
            let short: String = event_text.chars().take(40).collect();
            (
                "decide".to_string(),
                project,
                chapter,
                event_id.clone(),
                event_text,
                format!("预处理预期「{short}」(id={event_id}) 可纳入本次创作"),
            )
        } else if tool_name == "design_arc_outline" && offer_volume_review {
            let project = data
                .get("project")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("project").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let chapter = data
                .get("chapter")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(0);
            (
                "need_review".to_string(),
                project,
                chapter,
                String::new(),
                String::new(),
                "卷纲已更新，且有预处理预期硬条件已满足".to_string(),
            )
        } else {
            return Ok(false);
        };

        if project.is_empty() {
            return Ok(false);
        }
        let _ = turn_id;
        self.request_studio_next(
            thread_id,
            &project,
            &reason_text,
            studio_next::StudioNextContext::ExpectedEvent {
                kind,
                chapter,
                event_id,
                event_text,
            },
            false,
        )
        .await
    }

    async fn maybe_offer_plot_write(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        let _ = turn_id;
        if tool_name != "continue_writing" {
            return Ok(false);
        }
        if data.get("blocked").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(false);
        }
        let reason = data.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        if reason != "planned_inactive" && reason != "need_design_plot" {
            return Ok(false);
        }
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(false);
        }
        let title = data
            .get("plot_title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let volume = data
            .get("volume")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or_else(|| self.handoff_target_volume(&project));
        let title = if title.is_empty() {
            self.handoff_plot_title(&project, volume)
        } else {
            title
        };
        let nudge_reason = if reason == "need_design_plot" {
            "写章被拦：需要进行中的剧情卡".to_string()
        } else {
            format!("写章被拦：剧情卡「{title}」尚未激活")
        };
        self.request_studio_next(
            thread_id,
            &project,
            &nudge_reason,
            studio_next::StudioNextContext::PlotWrite {
                kind: reason.to_string(),
                title,
                volume,
            },
            false,
        )
        .await
    }

    /// After plot/chapter setting pass reports BLOCKER — Studio offers situational card.
    async fn maybe_offer_setting_blocker(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        data: &Value,
    ) -> Result<bool> {
        let _ = turn_id;
        if !matches!(
            tool_name,
            "continue_writing" | "revise_chapter" | "audit_chapter"
        ) {
            return Ok(false);
        }
        if data.get("plot_setting_blocker").and_then(|v| v.as_bool()) != Some(true) {
            return Ok(false);
        }
        // Volume-end path defers setting BLOCKER onto pending_volume_sync.
        if data.get("volume_ended").and_then(|v| v.as_u64()).is_some() {
            return Ok(false);
        }
        let project = data
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(false);
        }
        let chapter = data
            .get("chapter")
            .and_then(|v| v.as_u64())
            .or_else(|| args.get("chapter").and_then(|v| v.as_u64()))
            .unwrap_or(0) as u32;
        let detail = data
            .get("setting_blocker_detail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(240)
            .collect::<String>();
        let published = data.get("published").and_then(|v| v.as_bool()) == Some(true);
        let content_blocked =
            data.get("content_rule_blocked").and_then(|v| v.as_bool()) == Some(true);
        let length_blocked = is_length_publish_blocked(data);
        let plot_accept_open = published
            && data.get("plot_accept_passed").and_then(|v| v.as_bool()) == Some(false);
        let resume_chapter_next =
            chapter > 0 && (published || content_blocked || length_blocked);
        let detail_msg = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let revise_instructions = if length_blocked {
            Some(length_revise_instructions(&self.roots.config_root, detail_msg))
        } else if content_blocked {
            let report = data.get("report").and_then(|v| v.as_str()).unwrap_or("");
            Some(hard_rule_revise_instructions(
                &self.roots.projects_root,
                &self.roots.config_root,
                &project,
                chapter,
                detail_msg,
                report,
            ))
        } else if plot_accept_open {
            let rationale = data
                .get("plot_accept_rationale")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            Some(if rationale.is_empty() {
                "对照剧情卡收束条件补写本章缺口；只改收束相关段落，勿整章重写。".into()
            } else {
                format!(
                    "对照剧情卡收束条件补写本章缺口：{rationale}；只改收束相关段落，勿整章重写。"
                )
            })
        } else {
            None
        };
        let reason = if chapter > 0 {
            format!("第{chapter}章剧情后设定审计出现 BLOCKER：{detail}")
        } else {
            format!("设定审计出现 BLOCKER：{detail}")
        };
        self.request_studio_next(
            thread_id,
            &project,
            &reason,
            studio_next::StudioNextContext::SettingBlocker {
                chapter,
                detail,
                resume_chapter_next,
                published,
                plot_accept_open,
                content_blocked,
                revise_instructions,
                offer_volume_handoff_after: false,
            },
            false,
        )
        .await
    }

    async fn offer_setting_blocker_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        pending: PendingSettingBlocker,
    ) -> Result<bool> {
        let _ = turn_id;
        let reason = if pending.chapter > 0 {
            format!(
                "第{}章剧情后设定审计出现 BLOCKER：{}",
                pending.chapter, pending.detail
            )
        } else {
            format!("设定审计出现 BLOCKER：{}", pending.detail)
        };
        self.request_studio_next(
            thread_id,
            &pending.project,
            &reason,
            studio_next::StudioNextContext::SettingBlocker {
                chapter: pending.chapter,
                detail: pending.detail,
                resume_chapter_next: pending.resume_chapter_next,
                published: pending.published,
                plot_accept_open: pending.plot_accept_open,
                content_blocked: pending.content_blocked,
                revise_instructions: pending.revise_instructions,
                offer_volume_handoff_after: pending.offer_volume_handoff_after,
            },
            false,
        )
        .await
    }

    /// After setting BLOCKER resolved: resume chapter_next and/or volume handoff.
    async fn finish_setting_blocker_followups(
        &self,
        thread_id: &str,
        turn_id: &str,
        pending: &PendingSettingBlocker,
    ) -> Result<bool> {
        if pending.resume_chapter_next && pending.chapter > 0 {
            if self
                .offer_chapter_next_gate(
                    thread_id,
                    turn_id,
                    &pending.project,
                    pending.chapter,
                    pending.published,
                    pending.plot_accept_open,
                    pending.content_blocked,
                    pending.revise_instructions.clone(),
                )
                .await?
            {
                return Ok(true);
            }
        }
        if pending.offer_volume_handoff_after && !pending.project.is_empty() {
            return self
                .offer_volume_handoff_gate(
                    thread_id,
                    turn_id,
                    &pending.project,
                    VolumePhase::AwaitingNextArc,
                    None,
                )
                .await;
        }
        Ok(false)
    }

    async fn offer_chapter_next_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        chapter: u32,
        published: bool,
        plot_accept_open: bool,
        content_blocked: bool,
        revise_instructions: Option<String>,
    ) -> Result<bool> {
        if project.is_empty() || chapter == 0 {
            return Ok(false);
        }
        // Unpublished revise: hard-rule (`content_blocked`) or length (pre-filled expand instructions).
        if !published && !content_blocked && revise_instructions.is_none() {
            return Ok(false);
        }
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_audit.is_some()
                    || t.pending_volume_sync.is_some()
                    || t.pending_volume_audit.is_some()
                    || t.pending_setup.is_some()
                    || t.pending_volume_handoff.is_some()
                    || t.pending_mutation.is_some()
                    || t.pending_impact.is_some()
                    || t.pending_chapter_order.is_some()
                    || t.pending_setting_blocker.is_some()
                {
                    return Ok(false);
                }
            }
        }
        let options = self.chapter_next_options(published, plot_accept_open);
        if options.is_empty() {
            return Ok(false);
        }
        let length_revise = revise_instructions
            .as_deref()
            .is_some_and(|s| s.contains("扩写到") || s.contains("完整一章"));
        let prompt = if published {
            chapter_next_published_prompt(chapter, plot_accept_open)
        } else if length_revise {
            chapter_next_length_prompt(chapter, "正文字数未达发布门槛")
        } else {
            format!(
                "第{chapter}章因硬规则未发布。请选择：修正本章；也可点「其他」说明要求。"
            )
        };
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_chapter_next = Some(PendingChapterNext {
                project: project.to_string(),
                chapter,
                published,
                plot_accept_open,
                suggest_next: None,
                revise_instructions,
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

    async fn parse_setting_blocker_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<(String, Value, PendingSettingBlocker)> {
        // Hardcoded setting_blocker menu removed — use pending_studio_next.
        let _ = (thread_id, text);
        None
    }

    /// Returns plot-write gate action when the open card matches user text.
    async fn parse_plot_write_op(&self, thread_id: &str, text: &str) -> Option<PlotWriteGateAction> {
        let _ = (thread_id, text);
        None
    }

    async fn run_plot_write_gate_action(
        &self,
        thread_id: &str,
        turn_id: &str,
        action: PlotWriteGateAction,
    ) -> Result<()> {
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_plot_write = None;
            t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
        }
        let agent_item_id = new_id("item");
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemStarted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id.clone(),
                    text: String::new(),
                    status: ItemStatus::InProgress,
                },
            },
        )
        .await;
        let summary = match action {
            PlotWriteGateAction::Dismiss => {
                "已取消写章。需要时再说「继续」或「写下一章」。".to_string()
            }
            PlotWriteGateAction::ActivateAndWrite { project, title } => {
                let intro = format!("正在激活剧情卡「{title}」并继续写章…");
                self.emit_to_thread(
                    thread_id,
                    EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.to_string(),
                        turn_id: turn_id.to_string(),
                        item_id: agent_item_id.clone(),
                        delta: intro.clone(),
                    },
                )
                .await;
                let update_args = novelx_tools::with_confirm_skip(json!({
                    "project": project,
                    "title": title,
                    "status": "in_progress",
                    "set_active_main": true,
                }));
                let (upd_out, upd_data) = self
                    .run_one_tool(
                        thread_id,
                        turn_id,
                        "update_plot",
                        &update_args.to_string(),
                    )
                    .await?;
                if upd_data.get("blocked").and_then(|v| v.as_bool()) == Some(true)
                    || upd_data.get("error").is_some()
                {
                    format!("{intro}\n\n激活失败：{upd_out}")
                } else {
                    let write_args = novelx_tools::with_confirm_skip(json!({
                        "project": project,
                    }));
                    let (write_out, write_data) = self
                        .run_one_tool(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args.to_string(),
                        )
                        .await?;
                    let mut body = format!("{intro}\n\n{upd_out}\n\n{write_out}");
                    let _ = self
                        .maybe_offer_mutation_confirm(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args,
                            &write_data,
                        )
                        .await?;
                    let _ = self
                        .maybe_offer_chapter_order(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args,
                            &write_data,
                        )
                        .await?;
                    let _ = self
                        .maybe_offer_plot_write(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args,
                            &write_data,
                        )
                        .await?;
                    let _ = self
                        .maybe_offer_expected_event(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args,
                            &write_data,
                        )
                        .await?;
                    let _ = self
                        .maybe_offer_setting_blocker(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args,
                            &write_data,
                        )
                        .await?;
                    let _ = self
                        .maybe_offer_chapter_next(
                            thread_id,
                            turn_id,
                            "continue_writing",
                            &write_args,
                            &write_data,
                        )
                        .await?;
                    if write_data.get("blocked").and_then(|v| v.as_bool()) == Some(true) {
                        body.push_str("\n\n请按下方选项继续。");
                    }
                    body
                }
            }
            PlotWriteGateAction::Tool { name, args } => {
                let args = if name == "design_plot" || name == "continue_writing" {
                    novelx_tools::with_confirm_skip(args)
                } else {
                    args
                };
                let intro = format!("已选择：{name}…");
                self.emit_to_thread(
                    thread_id,
                    EventMsg::AgentMessageContentDelta {
                        thread_id: thread_id.to_string(),
                        turn_id: turn_id.to_string(),
                        item_id: agent_item_id.clone(),
                        delta: intro.clone(),
                    },
                )
                .await;
                let (output, data) = self
                    .run_one_tool(thread_id, turn_id, &name, &args.to_string())
                    .await?;
                let mut body = format!("{intro}\n\n{output}");
                if name == "design_plot"
                    && data.get("blocked").and_then(|v| v.as_bool()) != Some(true)
                    && data.get("error").is_none()
                {
                    let project = args
                        .get("project")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !project.is_empty() {
                        let write_args = novelx_tools::with_confirm_skip(json!({
                            "project": project,
                        }));
                        let (write_out, write_data) = self
                            .run_one_tool(
                                thread_id,
                                turn_id,
                                "continue_writing",
                                &write_args.to_string(),
                            )
                            .await?;
                        body = format!("{body}\n\n{write_out}");
                        let _ = self
                            .maybe_offer_chapter_next(
                                thread_id,
                                turn_id,
                                "continue_writing",
                                &write_args,
                                &write_data,
                            )
                            .await?;
                    }
                }
                body
            }
        };
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: summary.clone(),
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            });
            t.ui_turns = update_ui_turn_summary(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &summary,
            );
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id,
                    text: summary,
                    status: ItemStatus::Completed,
                },
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::TurnComplete {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            },
        )
        .await;
        Ok(())
    }
}

/// Result of applying Studio `offer_decisions`.
pub(crate) enum OfferApplyResult {
    /// No pending audit / not applicable.
    Ignored,
    /// Gate opened for the user.
    OpenedGate,
    /// Validation failed — surface to the model via tool content.
    Rejected(String),
}

#[allow(dead_code)]
pub(crate) enum PlotWriteGateAction {
    ActivateAndWrite { project: String, title: String },
    Dismiss,
    Tool { name: String, args: Value },
}

#[allow(dead_code)]
enum ExpectedEventGateOp {
    Dismiss,
    Tool { tool_name: String, args: Value },
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
///
/// Returns the full streamed progress text (for persisting process lines into ui_turns).
async fn coalesce_tool_progress(
    prog_rx: &mut mpsc::UnboundedReceiver<String>,
    core: &NovelxCore,
    thread_id: &str,
    turn_id: &str,
    item_id: &str,
    mirror_agent_item: Option<&str>,
) -> String {
    let mut buf = String::new();
    let mut acc = String::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(40));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    async fn flush(
        buf: &mut String,
        acc: &mut String,
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
        acc.push_str(&delta);
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
                                &mut acc,
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
                            &mut acc,
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
                    &mut acc,
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
    acc
}

fn is_audit_checklist_text(s: &str) -> bool {
    let s = s.trim();
    s.contains("### 问题清单")
        || (s.contains("问题清单") && s.contains("审校未通过"))
}

fn merge_tool_ui_output(streamed: &str, final_out: &str) -> String {
    let streamed = streamed.trim_end();
    let final_out = final_out.trim();
    if streamed.is_empty() {
        return final_out.to_string();
    }
    if final_out.is_empty() {
        return streamed.to_string();
    }
    if streamed.contains(final_out) {
        return streamed.to_string();
    }
    // Never append a fail coda that contradicts a streamed「一致性通过」step.
    let stream_consistency_pass = streamed.contains("一致性通过")
        || streamed.contains("结果：通过");
    let coda_consistency_fail = final_out.contains("一致性未通过");
    if stream_consistency_pass && coda_consistency_fail {
        // Prefer a hard-rule / next-step coda if present; otherwise keep stream only.
        if final_out.contains("硬规则") || final_out.contains("修正本章") {
            let hard = final_out
                .lines()
                .filter(|l| l.contains("硬规则") || l.contains("修正本章") || l.starts_with("——"))
                .collect::<Vec<_>>()
                .join("\n");
            if !hard.trim().is_empty() && !streamed.contains(hard.trim()) {
                return format!("{streamed}\n{hard}");
            }
        }
        return streamed.to_string();
    }
    // Audit / long reports: stream already has the body; final is often
    // `——\n第N章审校报告…` restating the same content — never paste twice.
    if tool_ui_looks_like_restated_report(streamed, final_out) {
        if final_out.lines().count() <= 4
            && (final_out.contains("详见上方")
                || final_out.contains("一致性未通过")
                || final_out.contains("一致性通过")
                || final_out.contains("硬规则未通过"))
        {
            return format!("{streamed}\n{final_out}");
        }
        return streamed.to_string();
    }
    format!("{streamed}\n{final_out}")
}

fn tool_ui_looks_like_restated_report(streamed: &str, final_out: &str) -> bool {
    let markers = [
        "一致性审计",
        "审校报告",
        "结果：未通过",
        "结果：通过",
        "### 问题清单",
        "（审校未通过",
    ];
    let stream_hits = markers.iter().filter(|m| streamed.contains(**m)).count();
    let final_hits = markers.iter().filter(|m| final_out.contains(**m)).count();
    if stream_hits >= 1 && final_hits >= 1 && final_out.chars().count() > 120 {
        return true;
    }
    // Identical large chunk already present (offer_decisions / short tools).
    if final_out.chars().count() >= 40 && streamed.contains(final_out) {
        return true;
    }
    false
}

/// Resolve display name for an entity hit path `entities/{group}/{slug}.md`.
fn resolve_entity_hit_name(project_dir: &std::path::Path, target_ref: &str) -> (String, String) {
    let kind = if target_ref.contains("/items/") {
        "item"
    } else if target_ref.contains("/locations/") {
        "location"
    } else {
        "character"
    };
    let group = match kind {
        "item" => "items",
        "location" => "locations",
        _ => "characters",
    };
    let slug = target_ref
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".md");
    if slug.is_empty() {
        return (kind.into(), String::new());
    }
    let folder = project_dir.join("entities").join(group);
    for card in novelx_pipeline::load_markdown_cards(&folder, group) {
        if card.slug == slug
            || novelx_pipeline::entity_names_equivalent(&card.name, slug)
            || novelx_pipeline::entity_names_equivalent(&card.slug, slug)
        {
            return (kind.into(), card.name);
        }
    }
    (kind.into(), slug.to_string())
}

fn consolidate_impact_hits(hits: Vec<ImpactHit>) -> Vec<ImpactHit> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, ImpactHit> = BTreeMap::new();
    for h in hits {
        let key = format!("{}:{}", h.target_kind.as_str(), h.target_ref);
        match map.get_mut(&key) {
            Some(existing) => {
                if !h.quote.is_empty() && !existing.quote.contains(&h.quote) {
                    existing.quote = format!("{}；{}", existing.quote, h.quote);
                    if existing.quote.chars().count() > 200 {
                        existing.quote = existing.quote.chars().take(200).collect();
                    }
                }
                if !h.matched_key.is_empty() && !existing.matched_key.contains(&h.matched_key) {
                    existing.matched_key = format!("{},{}", existing.matched_key, h.matched_key);
                }
            }
            None => {
                map.insert(key, h);
            }
        }
    }
    let mut out: Vec<ImpactHit> = map.into_values().collect();
    out.sort_by(|a, b| {
        a.target_kind
            .cascade_rank()
            .cmp(&b.target_kind.cascade_rank())
            .then(a.chapter.cmp(&b.chapter))
            .then(a.target_ref.cmp(&b.target_ref))
    });
    out
}

fn cascade_revise_instructions(source: &ImpactSource, hit: &ImpactHit) -> String {
    let before: String = source.before_snippet.chars().take(400).collect();
    let after: String = source.after_snippet.chars().take(400).collect();
    format!(
        "因「{}」（{}）已更新，请同步修正本处与「{}」相关的过时表述。\n\
         命中原因：{}\n命中原文：「{}」\n\
         旧要点节选：{before}\n新要点节选：{after}\n\
         只改与上述变更冲突或过时的句子；勿改无关情节，勿新增无关设定。",
        source.id,
        source.kind.as_str(),
        hit.matched_key,
        hit.reason,
        hit.quote.chars().take(120).collect::<String>(),
    )
}

/// Agent-bubble text after mutation confirm.
/// On success the tool card already shows `output` (e.g.「已写入设定卡 …」); keep only the ack.
/// Failures / blockers / nested previews still surface `output` in the bubble.
fn mutation_apply_agent_summary(intro: &str, output: &str, data: &Value) -> String {
    let needs_confirm = data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true);
    let failed = data.get("ok").and_then(|v| v.as_bool()) == Some(false)
        || data.get("blocked").and_then(|v| v.as_bool()) == Some(true)
        || data.get("error").is_some();
    if needs_confirm || failed || output.trim().is_empty() {
        if output.trim().is_empty() {
            intro.to_string()
        } else {
            format!("{intro}\n\n{output}")
        }
    } else {
        intro.to_string()
    }
}

/// Compact status lines suitable for the agent bubble (skip report dumps / separators).
///
/// Writer/draft ticks stay on the tool card + activity bar — do **not** mirror
/// `正文生成中` / `正文已写入` / `↻ draft` into the NovelX message (they clutter the reply).
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
        // Mid-stream prose / draft flush — tool card only.
        if t.contains("正文已写入")
            || t.contains("正文生成中")
            || t.contains("↻ draft")
            || (t.starts_with('…') && t.contains("生成中"))
        {
            continue;
        }
        let keep = t.starts_with('▶')
            || t.starts_with('✓')
            || t.starts_with('⏸')
            || t.starts_with('⚙')
            || t.starts_with('✕')
            || t.contains("调用模型")
            || t.contains("等待首包")
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

/// After `continue_writing` content-audit fail: do not pause-for-human yet —
/// Studio still needs a tool round for `offer_decisions` (or loop-end fallback).
fn defer_pause_for_studio_audit_offer(
    audit_fail: bool,
    awaiting_studio_audit_offer: bool,
) -> bool {
    audit_fail && awaiting_studio_audit_offer
}

/// Open the deterministic audit card when Studio never called `offer_decisions`.
/// Must not require `!pause_for_human` — write tools used to pause first and strand
/// `awaiting_offer` with no UI options.
fn should_open_audit_offer_fallback(
    awaiting_studio_audit_offer: bool,
    pending_awaiting_offer: bool,
) -> bool {
    awaiting_studio_audit_offer && pending_awaiting_offer
}

fn audit_failure_is_meta_only(data: &Value) -> bool {
    let Some(issues) = data.get("issues").and_then(|v| v.as_array()) else {
        // Fallback: report text from empty/unparseable auditor.
        let report = data
            .get("report")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("message").and_then(|v| v.as_str()))
            .unwrap_or("");
        return report.contains("模型返回为空")
            || report.contains("无法解析")
            || report.contains("一致性审计失败：无输出");
    };
    if issues.is_empty() {
        return false;
    }
    issues.iter().all(|i| {
        let ty = i.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let msg = i.get("message").and_then(|v| v.as_str()).unwrap_or("");
        ty.eq_ignore_ascii_case("META")
            && (msg.contains("返回为空") || msg.contains("无法解析") || msg.contains("请重试"))
    })
}

/// When hard-rule violations are only meta chapter refs, rewrite body deterministically.
fn try_autofix_meta_chapter_refs(
    projects_root: &std::path::Path,
    config_root: &std::path::Path,
    project: &str,
    chapter: u32,
) -> Option<String> {
    if project.is_empty() || chapter == 0 {
        return None;
    }
    let dir = project_dir(projects_root, project);
    let draft = read_chapter_draft(&dir, chapter)?;
    let naming = NamingRules::load_from_config_root(config_root);
    let content_rules = ContentRulesConfig::load_from_config_root(config_root);
    let violations = check_draft_with(&content_rules, &draft, &naming.forbidden_names);
    if violations.is_empty()
        || !violations.iter().all(|v| v.rule == "meta_chapter_ref")
    {
        return None;
    }
    let fixed = rewrite_meta_chapter_refs_with(&content_rules, &draft)?;
    if check_draft_with(&content_rules, &fixed, &naming.forbidden_names)
        .iter()
        .any(|v| v.rule == "meta_chapter_ref")
    {
        return None;
    }
    write_chapter_draft(&dir, chapter, &fixed).ok()?;
    Some(format!(
        "已自动清除正文中的章号元叙述（{} 处），标题行未改。",
        violations.len()
    ))
}

/// Concrete revise instructions for「修正本章」after a hard-rule block.
fn hard_rule_revise_instructions(
    projects_root: &std::path::Path,
    config_root: &std::path::Path,
    project: &str,
    chapter: u32,
    message: &str,
    report: &str,
) -> String {
    let detail = if !message.trim().is_empty() {
        message
    } else {
        report
    };
    let mut bullets: Vec<String> = Vec::new();
    for line in detail.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let msg = if let Some((idx, rest)) = t.split_once(". ") {
            if !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit()) {
                rest.trim()
            } else {
                t
            }
        } else {
            t
        };
        if msg.starts_with("正文出现")
            || msg.contains("禁名「")
            || msg.contains("元叙述")
            || msg.contains("[CONTRADICTION]")
            || msg.contains("[NEW_FACT]")
        {
            bullets.push(format!("· {msg}"));
        }
    }
    // Prefer live draft scan so instructions stay accurate after prior revise attempts.
    let content_rules = ContentRulesConfig::load_from_config_root(config_root);
    if let Some(draft) = read_chapter_draft(&project_dir(projects_root, project), chapter) {
        for (idx, line) in draft.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            for v in check_draft_with(&content_rules, line, &[]) {
                if v.rule == "meta_chapter_ref" || v.rule == "banned_name" {
                    let snippet: String = line.trim().chars().take(100).collect();
                    bullets.push(format!("· 约第{}行原文：「{snippet}」", idx + 1));
                }
            }
        }
    }
    bullets.sort();
    bullets.dedup();
    let mut out = format!(
        "硬规则阻断第{chapter}章发布。只改违规句，勿整章重写。\
         除标题行「# 第N章 …」外，正文禁止出现任何「第…章」字样；\
         改用故事内时间/事件指称（如「上次会面时」「那次接口被打开时」）。"
    );
    if !bullets.is_empty() {
        out.push_str("\n必须处理：\n");
        out.push_str(&bullets.join("\n"));
    }
    out
}

/// Prompt for content-rule block: include concrete violation lines from tool output when present.
fn chapter_next_published_prompt(chapter: u32, plot_accept_open: bool) -> String {
    if plot_accept_open {
        format!(
            "第{chapter}章已发布，但剧情收束条件尚未兑现。可继续推进本卡，或修正本章补写收束；也可点「其他」说明要求。"
        )
    } else {
        format!("第{chapter}章已发布。请选择：继续创作；也可点「其他」说明要求。")
    }
}

/// True when publish was blocked by chapter length hard gate / SoftShort streak escalate.
fn is_length_publish_blocked(data: &Value) -> bool {
    matches!(
        data.get("length_status").and_then(|v| v.as_str()).unwrap_or(""),
        "hard_short" | "soft_short_escalated"
    )
}

fn length_revise_instructions(config_root: &std::path::Path, detail: &str) -> String {
    let budget = novelx_harness::ChapterBudget::load_from_config_root(config_root);
    let mut msg = format!(
        "扩写到{}字完整一章；保持情节与人物一致，补足场景与对话，勿注水、勿另起主线。",
        budget.range_label()
    );
    let hint: String = detail
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && (l.contains("字数") || l.contains("偏短") || l.contains("硬门控"))
        })
        .take(3)
        .collect::<Vec<_>>()
        .join("；");
    if !hint.is_empty() {
        msg.push_str(" 参考：");
        msg.push_str(&hint);
    }
    msg
}

fn chapter_next_length_prompt(chapter: u32, tool_message: &str) -> String {
    let hint = tool_message
        .lines()
        .map(str::trim)
        .find(|l| l.contains("字数") || l.contains("偏短") || l.contains("硬门控"))
        .unwrap_or("正文字数未达发布门槛");
    format!(
        "第{chapter}章因字数未发布（{hint}）。请选择：修正本章（扩写）；也可点「其他」说明要求。"
    )
}

fn chapter_next_hard_rule_prompt(chapter: u32, tool_message: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in tool_message.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let msg = if let Some((idx, rest)) = t.split_once(". ") {
            if !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit()) {
                rest.trim()
            } else {
                t
            }
        } else {
            t
        };
        let looks = msg.starts_with("正文出现")
            || msg.contains("禁名「")
            || msg.contains("元叙述")
            || msg.contains("[CONTRADICTION]")
            || msg.contains("[NEW_FACT]");
        if looks {
            lines.push(format!("· {msg}"));
        }
    }
    if lines.is_empty() {
        format!(
            "第{chapter}章因硬规则未发布（如正文出现「第N章」元叙述或禁名）。请选择：修正本章；也可点「其他」说明要求。"
        )
    } else {
        format!(
            "第{chapter}章因硬规则未发布：\n{}\n\n请选择：修正本章；也可点「其他」说明要求。",
            lines.join("\n")
        )
    }
}

fn load_chapter_audit_brief(
    projects_root: &std::path::Path,
    project: &str,
    chapter: u32,
) -> Option<String> {
    let path = projects_root
        .join(project)
        .join("chapters")
        .join(format!("{chapter:03}"))
        .join("audit.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let brief = format_audit_choice_brief(&v, chapter);
    if brief.trim().is_empty() {
        None
    } else {
        Some(brief)
    }
}

/// Compact issue checklist for the chat bubble (full report stays on the tool card).
fn format_audit_choice_brief(data: &Value, chapter: u32) -> String {
    let mut issues = data
        .get("issues")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    issues = with_issue_ids(issues);
    if !issues.is_empty() {
        return format_audit_issue_checklist(&issues, chapter);
    }
    // Infra / empty parse — short message only, never paste a second full report.
    let message = data
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !message.is_empty() {
        let short: String = message.chars().take(280).collect();
        return format!("## 第{chapter}章审校\n\n{short}");
    }
    format!("## 第{chapter}章审校未通过\n\n（无结构化问题；请展开审校工具卡）")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_publish_blocked_detects_hard_and_escalate() {
        assert!(is_length_publish_blocked(&json!({"length_status": "hard_short"})));
        assert!(is_length_publish_blocked(
            &json!({"length_status": "soft_short_escalated"})
        ));
        assert!(!is_length_publish_blocked(&json!({"length_status": "soft_short"})));
        assert!(!is_length_publish_blocked(&json!({"length_status": "ok"})));
        assert!(!is_length_publish_blocked(&json!({})));
    }

    #[test]
    fn length_prompt_mentions_expand() {
        let p = chapter_next_length_prompt(3, "正文字数 4200，低于硬门控 4500");
        assert!(p.contains("第3章"));
        assert!(p.contains("字数"));
        assert!(p.contains("修正本章"));
    }

    #[test]
    fn merge_tool_ui_skips_restated_audit_report() {
        let streamed = format!(
            "一致性审计\n## 第17章审校报告\n结果：未通过\n{}",
            "长文".repeat(80)
        );
        let final_out = format!("——\n{streamed}");
        let merged = merge_tool_ui_output(&streamed, &final_out);
        assert_eq!(merged, streamed.trim_end());
        assert_eq!(
            merged.matches("第17章审校报告").count(),
            1,
            "must not paste the audit report twice"
        );
    }

    #[test]
    fn merge_tool_ui_keeps_short_fail_coda() {
        let streamed = "一致性审计\n## 审校报告\n结果：未通过\n细节…";
        let coda = "——\n一致性未通过（详见上方流式输出；请在下方决策卡选择）";
        let merged = merge_tool_ui_output(streamed, coda);
        assert!(merged.contains("详见上方"));
        assert!(merged.starts_with("一致性审计"));
    }

    #[test]
    fn merge_tool_ui_drops_contradictory_consistency_fail_coda() {
        let streamed = "✓ 一致性审计: 一致性通过\n✓ 节奏审查: 节奏审查完成";
        let bad = "——\n一致性未通过（详见上方流式输出；请在下方决策卡选择）";
        let merged = merge_tool_ui_output(streamed, bad);
        assert!(!merged.contains("一致性未通过"));
        assert!(merged.contains("一致性通过"));
    }

    #[test]
    fn merge_tool_ui_keeps_hard_rule_coda_after_consistency_pass() {
        let streamed = "✓ 一致性审计: 一致性通过\n✓ 节奏审查: 节奏审查完成";
        let coda = "——\n第22章硬规则未通过（未发布；详见上方；请在下方选择修正本章）";
        let merged = merge_tool_ui_output(streamed, coda);
        assert!(merged.contains("硬规则未通过"));
        assert!(merged.contains("一致性通过"));
    }

    #[test]
    fn checklist_text_detector() {
        assert!(is_audit_checklist_text(
            "## 第17章审校未通过\n\n### 问题清单\n1. `p0` "
        ));
        assert!(!is_audit_checklist_text("复审通过，本轮已正常结束。"));
    }

    #[test]
    fn mutation_apply_summary_omits_success_tool_output() {
        let intro = "已确认，正在应用：将写入character设定卡「甲」";
        let output = "已写入设定卡 /tmp/sample-novel/entities/characters/甲.md";
        let ok = mutation_apply_agent_summary(intro, output, &json!({"path": "/tmp/x"}));
        assert_eq!(ok, intro);
        let blocked = mutation_apply_agent_summary(
            intro,
            "设定审计 BLOCKER，未写入",
            &json!({"blocked": true}),
        );
        assert!(blocked.contains("BLOCKER"));
    }

    #[test]
    fn continue_writing_audit_fail_defers_pause_and_fallback_opens() {
        // Zombie-gate regression: write tool + audit fail must not pause before
        // Studio offer / must still open fallback when awaiting_offer remains.
        assert!(defer_pause_for_studio_audit_offer(true, true));
        assert!(!defer_pause_for_studio_audit_offer(true, false));
        assert!(!defer_pause_for_studio_audit_offer(false, true));
        // Fallback must not depend on !pause_for_human (write path used to pause first).
        assert!(should_open_audit_offer_fallback(true, true));
        assert!(!should_open_audit_offer_fallback(true, false));
        assert!(!should_open_audit_offer_fallback(false, true));
    }

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
