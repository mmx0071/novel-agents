//! Persist studio threads under `projects/<name>/.novelx/`.

use crate::ThreadState;
use anyhow::Result;
use novelx_llm::ChatMessage;
use novelx_protocol::{AgentLifecycle, AgentPath, SessionSource, ThreadSummary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedThread {
    pub thread_id: String,
    pub project: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub ui_turns: serde_json::Value,
    #[serde(default)]
    pub pending_audit: Option<crate::PendingAudit>,
    #[serde(default)]
    pub pending_volume_sync: Option<crate::PendingVolumeSync>,
    #[serde(default)]
    pub pending_volume_audit: Option<crate::PendingVolumeAudit>,
    #[serde(default)]
    pub pending_setup: Option<crate::PendingSetup>,
    #[serde(default)]
    pub pending_chapter_next: Option<crate::PendingChapterNext>,
}

pub fn project_novelx_dir(projects_root: &Path, project: &str) -> PathBuf {
    projects_root.join(project).join(".novelx")
}

pub fn thread_path(projects_root: &Path, project: &str) -> PathBuf {
    project_novelx_dir(projects_root, project).join("studio_thread.json")
}

pub fn load_project_thread(projects_root: &Path, project: &str) -> Option<PersistedThread> {
    let path = thread_path(projects_root, project);
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_project_thread(projects_root: &Path, persisted: &PersistedThread) -> Result<()> {
    let Some(project) = persisted.project.as_deref() else {
        return Ok(());
    };
    let dir = project_novelx_dir(projects_root, project);
    std::fs::create_dir_all(&dir)?;
    let path = thread_path(projects_root, project);
    let text = serde_json::to_string_pretty(persisted)?;
    std::fs::write(path, text)?;
    Ok(())
}

pub fn clear_project_thread(projects_root: &Path, project: &str) -> Result<()> {
    let path = thread_path(projects_root, project);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn from_state(thread_id: &str, state: &ThreadState) -> PersistedThread {
    PersistedThread {
        thread_id: thread_id.to_string(),
        project: state.summary.project.clone(),
        messages: state.messages.clone(),
        ui_turns: state.ui_turns.clone(),
        pending_audit: state.pending_audit.clone(),
        pending_volume_sync: state.pending_volume_sync.clone(),
        pending_volume_audit: state.pending_volume_audit.clone(),
        pending_setup: state.pending_setup.clone(),
        pending_chapter_next: state.pending_chapter_next.clone(),
    }
}

pub fn into_state(p: PersistedThread) -> (String, ThreadState) {
    let summary = ThreadSummary {
        id: p.thread_id.clone(),
        project: p.project,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        session_source: Some(SessionSource::Root),
        agent_path: Some(AgentPath::root()),
    };
    (
        p.thread_id,
        ThreadState {
            summary,
            messages: p.messages,
            abort: false,
            ui_turns: p.ui_turns,
            pending_audit: p.pending_audit,
            pending_volume_sync: p.pending_volume_sync,
            pending_volume_audit: p.pending_volume_audit,
            pending_setup: p.pending_setup,
            pending_chapter_next: p.pending_chapter_next,
            session_source: SessionSource::Root,
            lifecycle: AgentLifecycle::Running,
            subagent_job: None,
        },
    )
}
