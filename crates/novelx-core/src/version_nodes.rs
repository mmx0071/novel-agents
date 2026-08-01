//! Feature-gated wrappers around pipeline shadow-git version nodes + ops journal.

use crate::ops_journal;
use crate::NovelxCore;
use anyhow::Result;
use novelx_pipeline::{
    commit_node, ensure_version_repo, list_version_nodes, project_dir, restore_version_node,
    version_git_available, VersionNode,
};
use novelx_protocol::OpsJournalKind;
use serde_json::{json, Value};

impl NovelxCore {
    pub(crate) fn version_nodes_enabled(&self) -> bool {
        self.features.version_nodes() && version_git_available()
    }

    pub(crate) async fn checkpoint_version_node(
        &self,
        project: &str,
        label: &str,
        summary: &str,
        chapter: Option<u32>,
        mutation_id: Option<&str>,
        meta: Option<Value>,
        thread_id: Option<&str>,
        turn_id: Option<&str>,
    ) -> Option<VersionNode> {
        if !self.version_nodes_enabled() || project.is_empty() {
            return None;
        }
        let dir = project_dir(&self.roots.projects_root, project);
        match commit_node(&dir, label, summary, chapter, mutation_id, meta.clone()) {
            Ok(node) => {
                self.journal_ops(
                    Some(project),
                    thread_id,
                    turn_id,
                    chapter,
                    mutation_id,
                    OpsJournalKind::CheckpointCreated,
                    format!("checkpoint {}: {}", node.label, node.sha),
                    json!({
                        "sha": node.sha,
                        "label": node.label,
                        "summary": ops_journal::truncate_str(&node.summary, 400),
                        "meta": meta,
                    }),
                )
                .await;
                Some(node)
            }
            Err(e) => {
                tracing::warn!(%project, error = %e, "version node commit failed");
                None
            }
        }
    }

    pub fn list_project_version_nodes(
        &self,
        project: &str,
        limit: usize,
    ) -> Result<Vec<VersionNode>> {
        let dir = project_dir(&self.roots.projects_root, project);
        if self.version_nodes_enabled() {
            let _ = ensure_version_repo(&dir);
        }
        list_version_nodes(&dir, limit)
    }

    /// Clear in-memory gates so a pre-restore mutation preview cannot apply onto
    /// the restored tree.
    pub(crate) async fn clear_gates_after_restore(&self, thread_id: Option<&str>) {
        let Some(tid) = thread_id else {
            return;
        };
        if let Some(t) = self.threads.write().await.get_mut(tid) {
            t.pending_mutation = None;
            t.pending_impact = None;
            t.pending_audit = None;
            t.pending_chapter_next = None;
            t.pending_chapter_order = None;
            t.pending_studio_next = None;
            t.awaiting_studio_next = None;
            t.pending_volume_sync = None;
            t.pending_volume_handoff = None;
            t.pending_setup = None;
            t.council_auto_steer = None;
            t.council_auto_continue = None;
            t.ui_turns = crate::ui_sync::strip_ui_approvals(std::mem::take(&mut t.ui_turns));
        }
        self.clear_queued_inputs(tid).await;
        let _ = self.persist_thread(tid).await;
        self.publish_session_phase(tid).await;
    }

    pub async fn restore_project_version_node(
        &self,
        project: &str,
        sha: &str,
        thread_id: Option<&str>,
        turn_id: Option<&str>,
    ) -> Result<VersionNode> {
        if !self.features.version_nodes() {
            anyhow::bail!("studio.version_nodes 已关闭");
        }
        if !version_git_available() {
            anyhow::bail!("本机未安装 git，无法回退版本节点");
        }
        let dir = project_dir(&self.roots.projects_root, project);
        let node = restore_version_node(&dir, sha)?;
        self.clear_gates_after_restore(thread_id).await;
        let restored_sha = node
            .meta
            .as_ref()
            .and_then(|m| m.get("restored_sha"))
            .and_then(|v| v.as_str())
            .unwrap_or(sha);
        self.journal_ops(
            Some(project),
            thread_id,
            turn_id,
            None,
            None,
            OpsJournalKind::CheckpointRestored,
            format!("restored worktree to {restored_sha}"),
            json!({
                "milestone_sha": node.sha,
                "restored_sha": restored_sha,
                "label": node.label,
                "summary": ops_journal::truncate_str(&node.summary, 400),
                "meta": node.meta,
            }),
        )
        .await;
        Ok(node)
    }

    /// After tool `restore_version_node` succeeds — journal + clear gates.
    pub(crate) async fn after_restore_version_tool(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: &str,
        data: &Value,
    ) {
        if data.get("restored").and_then(|v| v.as_bool()) != Some(true) {
            return;
        }
        self.clear_gates_after_restore(Some(thread_id)).await;
        let restored_sha = data
            .get("node")
            .and_then(|n| n.get("meta"))
            .and_then(|m| m.get("restored_sha"))
            .and_then(|v| v.as_str())
            .or_else(|| data.get("node").and_then(|n| n.get("sha")).and_then(|v| v.as_str()))
            .unwrap_or("");
        let milestone = data
            .get("node")
            .and_then(|n| n.get("sha"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let label = data
            .get("node")
            .and_then(|n| n.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or("restore");
        self.journal_ops(
            Some(project),
            Some(thread_id),
            Some(turn_id),
            None,
            None,
            OpsJournalKind::CheckpointRestored,
            format!("restored worktree to {restored_sha}"),
            json!({
                "milestone_sha": milestone,
                "restored_sha": restored_sha,
                "label": label,
                "via": "tool",
            }),
        )
        .await;
    }
}

/// Tools that may mutate project files on disk.
pub fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "continue_writing"
            | "continue_writing_batch"
            | "revise_chapter"
            | "split_chapter"
            | "revise_outline"
            | "design_plot"
            | "update_plot"
            | "design_entity"
            | "delete_entity"
            | "upsert_setting"
            | "supplement_setting"
            | "design_master_outline"
            | "design_arc_outline"
            | "replan_volume"
            | "activate_agents"
            | "sync_volume"
            | "confirm_volume_memory"
            | "enqueue_expected_event"
            | "update_expected_event"
            | "resolve_expected_event"
            | "restore_version_node"
    )
}
