//! NovelX protocol — Codex-inspired Thread / Turn / Item / Submission model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type ThreadId = String;
pub type TurnId = String;
pub type ItemId = String;
pub type SubmissionId = String;
pub type AgentRole = String;

/// Tree path for agents, e.g. `/root`, `/root/writer`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentPath(pub String);

impl AgentPath {
    pub fn root() -> Self {
        Self("/root".into())
    }

    pub fn child(&self, role: &str) -> Self {
        Self(format!("{}/{}", self.0.trim_end_matches('/'), role))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionSource {
    #[default]
    Root,
    SubAgent {
        parent_thread_id: ThreadId,
        role: AgentRole,
        depth: u32,
        agent_path: AgentPath,
    },
}

impl SessionSource {
    pub fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }

    pub fn role(&self) -> Option<&str> {
        match self {
            Self::Root => None,
            Self::SubAgent { role, .. } => Some(role.as_str()),
        }
    }

    pub fn agent_path(&self) -> AgentPath {
        match self {
            Self::Root => AgentPath::root(),
            Self::SubAgent { agent_path, .. } => agent_path.clone(),
        }
    }

    pub fn parent_thread_id(&self) -> Option<&str> {
        match self {
            Self::Root => None,
            Self::SubAgent {
                parent_thread_id, ..
            } => Some(parent_thread_id.as_str()),
        }
    }

    pub fn depth(&self) -> u32 {
        match self {
            Self::Root => 0,
            Self::SubAgent { depth, .. } => *depth,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycle {
    Spawned,
    #[default]
    Running,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub thread_id: ThreadId,
    pub agent_path: AgentPath,
    pub role: Option<AgentRole>,
    pub parent_thread_id: Option<ThreadId>,
    pub lifecycle: AgentLifecycle,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnItem {
    UserMessage {
        id: ItemId,
        text: String,
    },
    AgentMessage {
        id: ItemId,
        text: String,
        status: ItemStatus,
    },
    Reasoning {
        id: ItemId,
        text: String,
        status: ItemStatus,
    },
    SkillLoad {
        id: ItemId,
        name: String,
        path: Option<String>,
        status: ItemStatus,
    },
    ToolCall {
        id: ItemId,
        name: String,
        arguments: serde_json::Value,
        output: Option<String>,
        status: ItemStatus,
        duration_ms: Option<u64>,
    },
    DraftPatch {
        id: ItemId,
        project: String,
        chapter: u32,
        start_para: usize,
        end_para: usize,
        before: String,
        after: String,
        status: ItemStatus,
    },
    PipelineStep {
        id: ItemId,
        agent: String,
        status: ItemStatus,
        summary: Option<String>,
    },
    AuditReport {
        id: ItemId,
        passed: bool,
        report: String,
        status: ItemStatus,
    },
    AgentSpawn {
        id: ItemId,
        child_thread_id: ThreadId,
        role: AgentRole,
        agent_path: AgentPath,
        status: ItemStatus,
    },
}

impl TurnItem {
    pub fn id(&self) -> &str {
        match self {
            TurnItem::UserMessage { id, .. }
            | TurnItem::AgentMessage { id, .. }
            | TurnItem::Reasoning { id, .. }
            | TurnItem::SkillLoad { id, .. }
            | TurnItem::ToolCall { id, .. }
            | TurnItem::DraftPatch { id, .. }
            | TurnItem::PipelineStep { id, .. }
            | TurnItem::AuditReport { id, .. }
            | TurnItem::AgentSpawn { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    Text {
        text: String,
        #[serde(default)]
        skills: Vec<String>,
    },
    Skill {
        name: String,
        path: Option<String>,
    },
    Mention {
        name: String,
        path: String,
    },
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            skills: vec![],
        }
    }

    pub fn primary_text(items: &[UserInput]) -> String {
        items
            .iter()
            .filter_map(|i| match i {
                UserInput::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn collect_skills(items: &[UserInput]) -> Vec<String> {
        let mut out = Vec::new();
        for i in items {
            match i {
                UserInput::Text { skills, .. } => out.extend(skills.clone()),
                UserInput::Skill { name, .. } => out.push(name.clone()),
                UserInput::Mention { .. } => {}
            }
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterAgentCommunication {
    pub author_thread_id: ThreadId,
    pub recipient_thread_id: ThreadId,
    pub content: String,
    /// When true, idle recipient starts a Regular turn to drain mailbox.
    #[serde(default = "default_true")]
    pub trigger_turn: bool,
    #[serde(default)]
    pub kind: InterAgentKind,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InterAgentKind {
    #[default]
    Message,
    Spawn,
    Followup,
    Result,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventMsg {
    SessionConfigured {
        thread_id: ThreadId,
        project: Option<String>,
        #[serde(default)]
        agent_path: Option<AgentPath>,
        #[serde(default)]
        session_source: Option<SessionSource>,
    },
    TurnStarted {
        thread_id: ThreadId,
        turn_id: TurnId,
    },
    TurnComplete {
        thread_id: ThreadId,
        turn_id: TurnId,
    },
    TurnAborted {
        thread_id: ThreadId,
        turn_id: TurnId,
        reason: String,
    },
    ItemStarted {
        thread_id: ThreadId,
        turn_id: TurnId,
        item: TurnItem,
    },
    ItemCompleted {
        thread_id: ThreadId,
        turn_id: TurnId,
        item: TurnItem,
    },
    AgentMessageContentDelta {
        thread_id: ThreadId,
        turn_id: TurnId,
        item_id: ItemId,
        delta: String,
    },
    ReasoningContentDelta {
        thread_id: ThreadId,
        turn_id: TurnId,
        item_id: ItemId,
        delta: String,
    },
    ToolCallOutputDelta {
        thread_id: ThreadId,
        turn_id: TurnId,
        item_id: ItemId,
        delta: String,
    },
    RequestUserInput {
        thread_id: ThreadId,
        turn_id: TurnId,
        prompt: String,
        options: Vec<UserInputOption>,
    },
    AgentStatusChanged {
        status: AgentStatus,
    },
    Error {
        thread_id: Option<ThreadId>,
        message: String,
    },
    Warning {
        thread_id: Option<ThreadId>,
        message: String,
    },
    /// Codex-style progressive todo list (exactly one `in_progress` while active).
    TodoUpdated {
        thread_id: ThreadId,
        todos: Vec<TodoItem>,
    },
    /// Studio cleared prior chat history when starting a new-chapter write.
    /// When `keep_turn_id` is set, the client should keep that turn and drop the rest.
    ChatHistoryReset {
        thread_id: ThreadId,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keep_turn_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    StartThread {
        project: Option<String>,
    },
    ResumeThread {
        thread_id: ThreadId,
    },
    /// Codex-style user input (preferred).
    UserInput {
        thread_id: ThreadId,
        items: Vec<UserInput>,
        #[serde(default)]
        skills: Vec<String>,
    },
    /// Thin wrapper kept for Web/WS compatibility → UserInput.
    StartTurn {
        thread_id: ThreadId,
        text: String,
        #[serde(default)]
        skills: Vec<String>,
    },
    InterruptTurn {
        thread_id: ThreadId,
        #[serde(default)]
        turn_id: Option<TurnId>,
    },
    /// Mid-turn steer (pending input on active Regular turn).
    SteerTurn {
        thread_id: ThreadId,
        #[serde(default)]
        turn_id: Option<TurnId>,
        text: String,
    },
    RespondUserInput {
        thread_id: ThreadId,
        turn_id: TurnId,
        option_id: String,
        free_text: Option<String>,
    },
    InterAgentCommunication {
        communication: InterAgentCommunication,
    },
    Shutdown {
        thread_id: ThreadId,
    },
}

impl Op {
    pub fn thread_id(&self) -> Option<&str> {
        match self {
            Op::StartThread { .. } => None,
            Op::ResumeThread { thread_id }
            | Op::UserInput { thread_id, .. }
            | Op::StartTurn { thread_id, .. }
            | Op::InterruptTurn { thread_id, .. }
            | Op::SteerTurn { thread_id, .. }
            | Op::RespondUserInput { thread_id, .. }
            | Op::Shutdown { thread_id } => Some(thread_id.as_str()),
            Op::InterAgentCommunication { communication } => {
                Some(communication.recipient_thread_id.as_str())
            }
        }
    }

    /// Normalize legacy StartTurn into UserInput.
    pub fn into_canonical(self) -> Self {
        match self {
            Op::StartTurn {
                thread_id,
                text,
                skills,
            } => Op::UserInput {
                thread_id,
                items: vec![UserInput::Text {
                    text,
                    skills: skills.clone(),
                }],
                skills,
            },
            other => other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submission {
    pub id: SubmissionId,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<String>,
}

impl Submission {
    pub fn new(op: Op) -> Self {
        Self {
            id: new_id("sub"),
            op,
            client_user_message_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub id: ThreadId,
    pub project: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub session_source: Option<SessionSource>,
    #[serde(default)]
    pub agent_path: Option<AgentPath>,
}

pub fn new_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}

/// Decision / execution audit trail kinds (append-only ops journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpsJournalKind {
    TurnStarted,
    TurnComplete,
    TurnAborted,
    UserInput,
    IntentMatched,
    GateOpened,
    GateResponded,
    ToolStarted,
    ToolCompleted,
    MutationPreview,
    MutationConfirm,
    MutationDiscard,
    MutationApplied,
    ImpactOffered,
    ImpactResolved,
    PipelineStep,
    PublishResult,
    /// Consistency / setting audit report item (not the same as publish).
    AuditReport,
    HardBlock,
    HistoryReset,
}

impl OpsJournalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnStarted => "turn_started",
            Self::TurnComplete => "turn_complete",
            Self::TurnAborted => "turn_aborted",
            Self::UserInput => "user_input",
            Self::IntentMatched => "intent_matched",
            Self::GateOpened => "gate_opened",
            Self::GateResponded => "gate_responded",
            Self::ToolStarted => "tool_started",
            Self::ToolCompleted => "tool_completed",
            Self::MutationPreview => "mutation_preview",
            Self::MutationConfirm => "mutation_confirm",
            Self::MutationDiscard => "mutation_discard",
            Self::MutationApplied => "mutation_applied",
            Self::ImpactOffered => "impact_offered",
            Self::ImpactResolved => "impact_resolved",
            Self::PipelineStep => "pipeline_step",
            Self::PublishResult => "publish_result",
            Self::AuditReport => "audit_report",
            Self::HardBlock => "hard_block",
            Self::HistoryReset => "history_reset",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "turn_started" => Some(Self::TurnStarted),
            "turn_complete" => Some(Self::TurnComplete),
            "turn_aborted" => Some(Self::TurnAborted),
            "user_input" => Some(Self::UserInput),
            "intent_matched" => Some(Self::IntentMatched),
            "gate_opened" => Some(Self::GateOpened),
            "gate_responded" => Some(Self::GateResponded),
            "tool_started" => Some(Self::ToolStarted),
            "tool_completed" => Some(Self::ToolCompleted),
            "mutation_preview" => Some(Self::MutationPreview),
            "mutation_confirm" => Some(Self::MutationConfirm),
            "mutation_discard" => Some(Self::MutationDiscard),
            "mutation_applied" => Some(Self::MutationApplied),
            "impact_offered" => Some(Self::ImpactOffered),
            "impact_resolved" => Some(Self::ImpactResolved),
            "pipeline_step" => Some(Self::PipelineStep),
            "publish_result" => Some(Self::PublishResult),
            "audit_report" => Some(Self::AuditReport),
            "hard_block" => Some(Self::HardBlock),
            "history_reset" => Some(Self::HistoryReset),
            _ => None,
        }
    }
}

impl std::fmt::Display for OpsJournalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// One append-only ops journal line under `projects/<name>/.novelx/ops_journal.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsJournalEntry {
    pub ts: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub kind: OpsJournalKind,
    pub summary: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl EventMsg {
    pub fn session_configured(
        thread_id: impl Into<ThreadId>,
        project: Option<String>,
        source: &SessionSource,
    ) -> Self {
        Self::SessionConfigured {
            thread_id: thread_id.into(),
            project,
            agent_path: Some(source.agent_path()),
            session_source: Some(source.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_roundtrip() {
        let ev = EventMsg::TurnStarted {
            thread_id: "thr_1".into(),
            turn_id: "turn_1".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: EventMsg = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, EventMsg::TurnStarted { .. }));
    }

    #[test]
    fn start_turn_canonicalizes() {
        let op = Op::StartTurn {
            thread_id: "t".into(),
            text: "hi".into(),
            skills: vec!["studio".into()],
        }
        .into_canonical();
        match op {
            Op::UserInput { items, skills, .. } => {
                assert_eq!(UserInput::primary_text(&items), "hi");
                assert_eq!(skills, vec!["studio".to_string()]);
            }
            _ => panic!("expected UserInput"),
        }
    }

    #[test]
    fn agent_path_child() {
        assert_eq!(
            AgentPath::root().child("writer").as_str(),
            "/root/writer"
        );
    }

    #[test]
    fn ops_journal_kind_roundtrip() {
        let entry = OpsJournalEntry {
            ts: "2026-07-28T00:00:00Z".into(),
            project: "sample-novel".into(),
            thread_id: Some("thr_1".into()),
            turn_id: Some("turn_1".into()),
            chapter: Some(1),
            correlation_id: Some("mut_1".into()),
            kind: OpsJournalKind::MutationApplied,
            summary: "applied".into(),
            data: serde_json::json!({}),
        };
        let s = serde_json::to_string(&entry).unwrap();
        assert!(s.contains("mutation_applied"));
        let back: OpsJournalEntry = serde_json::from_str(&s).unwrap();
        assert_eq!(back.kind, OpsJournalKind::MutationApplied);
        assert_eq!(OpsJournalKind::parse("gate_opened"), Some(OpsJournalKind::GateOpened));
    }
}
