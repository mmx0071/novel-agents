//! Append-only decision / execution audit trail.
//!
//! Path: `projects/<name>/.novelx/ops_journal.jsonl`
//! Survives `ChatHistoryReset` (chat wipe ≠ journal wipe).

use crate::thread_store::project_novelx_dir;
use anyhow::{Context, Result};
use novelx_protocol::{OpsJournalEntry, OpsJournalKind};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// Soft cap for summary / string fields written into journal `data`.
pub const MAX_FIELD_CHARS: usize = 2000;

pub fn journal_path(projects_root: &Path, project: &str) -> PathBuf {
    project_novelx_dir(projects_root, project).join("ops_journal.jsonl")
}

pub fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Truncate long strings inside a JSON value (best-effort; keeps structure).
pub fn truncate_value(v: &Value, max: usize) -> Value {
    match v {
        Value::String(s) => Value::String(truncate_str(s, max)),
        Value::Array(arr) => {
            let limited: Vec<Value> = arr
                .iter()
                .take(32)
                .map(|x| truncate_value(x, max))
                .collect();
            Value::Array(limited)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map.iter().take(48) {
                out.insert(k.clone(), truncate_value(val, max));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct OpsJournalQuery {
    pub limit: usize,
    pub chapter: Option<u32>,
    pub kind: Option<OpsJournalKind>,
    /// RFC3339 lower bound (inclusive); entries with `ts >= after`.
    pub after: Option<String>,
}

impl OpsJournalQuery {
    pub fn recent(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }
}

pub fn build_entry(
    project: &str,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
    chapter: Option<u32>,
    correlation_id: Option<&str>,
    kind: OpsJournalKind,
    summary: impl Into<String>,
    data: Value,
) -> OpsJournalEntry {
    OpsJournalEntry {
        ts: chrono::Utc::now().to_rfc3339(),
        project: project.to_string(),
        thread_id: thread_id.map(|s| s.to_string()),
        turn_id: turn_id.map(|s| s.to_string()),
        chapter,
        correlation_id: correlation_id.map(|s| s.to_string()),
        kind,
        summary: truncate_str(&summary.into(), MAX_FIELD_CHARS),
        data: truncate_value(&data, MAX_FIELD_CHARS),
    }
}

pub fn append_entry(projects_root: &Path, entry: &OpsJournalEntry) -> Result<()> {
    if entry.project.trim().is_empty() {
        return Ok(());
    }
    let path = journal_path(projects_root, &entry.project);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let line = serde_json::to_string(entry)?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Whether a tool `data` payload represents a successful apply (for mutation_applied).
///
/// `run_one_tool` converts dispatch errors into `Ok` + `{ok:false}` — those must not
/// be recorded as applied.
pub fn tool_apply_succeeded(data: &Value) -> bool {
    if data.get("blocked").and_then(|v| v.as_bool()) == Some(true) {
        return false;
    }
    if data.get("content_rule_blocked").and_then(|v| v.as_bool()) == Some(true) {
        return false;
    }
    if data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true) {
        return false;
    }
    if data.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }
    true
}

/// Background writer: keep sync disk I/O off the emit / turn hot path.
pub fn spawn_writer(projects_root: PathBuf) -> mpsc::UnboundedSender<OpsJournalEntry> {
    let (tx, mut rx) = mpsc::unbounded_channel::<OpsJournalEntry>();
    tokio::spawn(async move {
        while let Some(entry) = rx.recv().await {
            let root = projects_root.clone();
            let result = tokio::task::spawn_blocking(move || append_entry(&root, &entry)).await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "ops_journal append failed");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "ops_journal writer join failed");
                }
            }
        }
    });
    tx
}

/// Load and filter journal rows (newest last; returns up to `limit` from the end).
pub fn query_entries(projects_root: &Path, project: &str, q: &OpsJournalQuery) -> Vec<OpsJournalEntry> {
    let Ok(text) = std::fs::read_to_string(journal_path(projects_root, project)) else {
        return Vec::new();
    };
    let mut all: Vec<OpsJournalEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if let Some(kind) = q.kind {
        all.retain(|e| e.kind == kind);
    }
    if let Some(ch) = q.chapter {
        all.retain(|e| e.chapter == Some(ch));
    }
    if let Some(after) = q.after.as_deref() {
        all.retain(|e| e.ts.as_str() >= after);
    }
    let limit = q.limit.max(1).min(2000);
    if all.len() > limit {
        all = all.split_off(all.len() - limit);
    }
    all
}

/// Compact one-line for CLI.
pub fn format_entry_line(e: &OpsJournalEntry) -> String {
    let ch = e
        .chapter
        .map(|c| format!(" ch{c}"))
        .unwrap_or_default();
    let corr = e
        .correlation_id
        .as_deref()
        .map(|c| format!(" [{c}]"))
        .unwrap_or_default();
    format!(
        "{} {} {}{}{} — {}",
        e.ts,
        e.kind.as_str(),
        e.project,
        ch,
        corr,
        e.summary
    )
}

/// Best-effort chapter extraction from tool args / data.
pub fn chapter_from_value(v: &Value) -> Option<u32> {
    v.get("chapter")
        .and_then(|c| c.as_u64())
        .filter(|&n| n > 0 && n <= u64::from(u32::MAX))
        .map(|n| n as u32)
}

pub fn args_summary(args: &Value) -> Value {
    truncate_value(args, 400)
}

pub fn empty_data() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;
    use novelx_protocol::OpsJournalKind;
    use std::fs;

    fn tmp_root() -> PathBuf {
        std::env::temp_dir().join(format!("novelx-ops-{}", uuid::Uuid::new_v4().simple()))
    }

    #[test]
    fn append_and_query_filters() {
        let root = tmp_root();
        let _ = fs::remove_dir_all(&root);
        let project = "sample-novel";
        append_entry(
            &root,
            &build_entry(
                project,
                Some("thr_1"),
                Some("turn_1"),
                Some(3),
                Some("mut_abc"),
                OpsJournalKind::MutationPreview,
                "preview setting",
                json!({"tool": "upsert_setting"}),
            ),
        )
        .unwrap();
        append_entry(
            &root,
            &build_entry(
                project,
                Some("thr_1"),
                Some("turn_2"),
                Some(3),
                Some("mut_abc"),
                OpsJournalKind::MutationApplied,
                "applied",
                json!({}),
            ),
        )
        .unwrap();
        append_entry(
            &root,
            &build_entry(
                project,
                Some("thr_1"),
                Some("turn_3"),
                Some(4),
                None,
                OpsJournalKind::HistoryReset,
                "cleared chat",
                json!({}),
            ),
        )
        .unwrap();

        let all = query_entries(&root, project, &OpsJournalQuery::recent(50));
        assert_eq!(all.len(), 3);

        let mut q = OpsJournalQuery::recent(50);
        q.kind = Some(OpsJournalKind::MutationApplied);
        let applied = query_entries(&root, project, &q);
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].kind, OpsJournalKind::MutationApplied);

        q.kind = None;
        q.chapter = Some(3);
        let ch3 = query_entries(&root, project, &q);
        assert_eq!(ch3.len(), 2);

        // History reset does not wipe journal.
        let still = query_entries(&root, project, &OpsJournalQuery::recent(50));
        assert_eq!(still.len(), 3);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn truncate_long_strings() {
        let long = "甲".repeat(3000);
        let t = truncate_str(&long, MAX_FIELD_CHARS);
        assert!(t.chars().count() <= MAX_FIELD_CHARS);
        assert!(t.ends_with('…'));
        let v = truncate_value(&json!({"body": long}), 100);
        assert!(v["body"].as_str().unwrap().chars().count() <= 100);
    }

    #[test]
    fn empty_project_skips_append() {
        let root = tmp_root();
        let entry = build_entry(
            "",
            None,
            None,
            None,
            None,
            OpsJournalKind::UserInput,
            "noop",
            json!({}),
        );
        append_entry(&root, &entry).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn tool_apply_succeeded_rejects_failures() {
        assert!(tool_apply_succeeded(&json!({})));
        assert!(tool_apply_succeeded(&json!({"ok": true})));
        assert!(!tool_apply_succeeded(&json!({"ok": false, "error": "boom"})));
        assert!(!tool_apply_succeeded(&json!({"blocked": true})));
        assert!(!tool_apply_succeeded(&json!({"needs_confirm": true})));
        assert!(!tool_apply_succeeded(&json!({"content_rule_blocked": true})));
    }
}
