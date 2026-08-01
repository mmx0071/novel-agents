//! Shadow git version nodes (Cursor/Claude-style) per project.
//!
//! Layout:
//! - `GIT_DIR` = `projects/<name>/.novelx/versions.git`
//! - `GIT_WORK_TREE` = `projects/<name>`
//! - Index: `projects/<name>/.novelx/version_nodes.jsonl`

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn now_label_ts() -> String {
    now_rfc3339()
}

const EXCLUDE_LINES: &[&str] = &[
    ".novelx/versions.git/",
    ".novelx/studio_thread.json",
    ".novelx/ops_journal.jsonl",
    ".novelx/version_nodes.jsonl",
    "lore/index.sqlite",
    "lore/index.sqlite-*",
    "**/*.gz",
    ".DS_Store",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionNode {
    pub sha: String,
    pub label: String,
    pub summary: String,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

pub fn novelx_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".novelx")
}

pub fn git_dir(project_dir: &Path) -> PathBuf {
    novelx_dir(project_dir).join("versions.git")
}

pub fn nodes_path(project_dir: &Path) -> PathBuf {
    novelx_dir(project_dir).join("version_nodes.jsonl")
}

pub fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_git(project_dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    let gd = git_dir(project_dir);
    let mut cmd = Command::new("git");
    cmd.env("GIT_DIR", &gd);
    cmd.env("GIT_WORK_TREE", project_dir);
    cmd.env("GIT_AUTHOR_NAME", "NovelX");
    cmd.env("GIT_AUTHOR_EMAIL", "novelx@local");
    cmd.env("GIT_COMMITTER_NAME", "NovelX");
    cmd.env("GIT_COMMITTER_EMAIL", "novelx@local");
    cmd.args(args);
    cmd.output()
        .with_context(|| format!("git {:?} in {}", args, project_dir.display()))
}

fn run_git_ok(project_dir: &Path, args: &[&str]) -> Result<String> {
    let out = run_git(project_dir, args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        bail!("git {:?} failed: {}{}", args, stderr, stdout);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_exclude(project_dir: &Path) -> Result<()> {
    let exclude = git_dir(project_dir).join("info").join("exclude");
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for line in EXCLUDE_LINES {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(exclude, body)?;
    Ok(())
}

/// Ensure shadow git exists; create initial empty commit if needed.
pub fn ensure_repo(project_dir: &Path) -> Result<()> {
    if !git_available() {
        bail!("git not found on PATH; version nodes disabled");
    }
    std::fs::create_dir_all(novelx_dir(project_dir))?;
    let gd = git_dir(project_dir);
    if !gd.join("HEAD").exists() {
        std::fs::create_dir_all(&gd)?;
        let out = Command::new("git")
            .args(["init", "--bare"])
            .arg(&gd)
            .output()
            .context("git init --bare")?;
        if !out.status.success() {
            bail!(
                "git init failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let _ = Command::new("git")
            .args(["--git-dir"])
            .arg(&gd)
            .args(["symbolic-ref", "HEAD", "refs/heads/main"])
            .output();
    }
    write_exclude(project_dir)?;
    let has_commit = run_git(project_dir, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_commit {
        let _ = run_git_ok(project_dir, &["add", "-A"])?;
        let out = run_git(
            project_dir,
            &["commit", "--allow-empty", "-m", "node: init"],
        )?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("nothing to commit") {
                bail!("initial commit failed: {stderr}");
            }
        }
        let sha = current_head(project_dir)?;
        if sha.is_empty() {
            bail!("initial commit produced empty HEAD");
        }
        append_node_record(
            project_dir,
            &VersionNode {
                sha,
                label: "init".into(),
                summary: "shadow git initialized".into(),
                ts: now_rfc3339(),
                chapter: None,
                mutation_id: None,
                meta: None,
            },
        )?;
    }
    Ok(())
}

pub fn current_head(project_dir: &Path) -> Result<String> {
    run_git_ok(project_dir, &["rev-parse", "HEAD"])
}

fn append_node_record(project_dir: &Path, node: &VersionNode) -> Result<()> {
    if node.sha.trim().is_empty() {
        bail!("refuse to record version node with empty sha");
    }
    let path = nodes_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{}", serde_json::to_string(node)?)?;
    Ok(())
}

fn wants_empty_milestone(label: &str) -> bool {
    label.starts_with("chapter/")
        || label.starts_with("plot/")
        || label.starts_with("volume/")
        || label.starts_with("pre/")
        || label.starts_with("pre-restore/")
        || label.starts_with("restore/")
}

/// Stage all tracked/untracked (respecting exclude) and commit a version node.
pub fn commit_node(
    project_dir: &Path,
    label: &str,
    summary: &str,
    chapter: Option<u32>,
    mutation_id: Option<&str>,
    meta: Option<Value>,
) -> Result<VersionNode> {
    ensure_repo(project_dir)?;
    let before = current_head(project_dir).unwrap_or_default();
    let _ = run_git_ok(project_dir, &["add", "-A"])?;
    let msg = format!("node: {label} — {summary}");
    let status = run_git(project_dir, &["status", "--porcelain"])?;
    let dirty = !status.stdout.is_empty();
    let mut committed = false;
    if dirty {
        let out = run_git(project_dir, &["commit", "-m", &msg])?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("nothing to commit") {
                bail!("commit failed: {stderr}");
            }
        } else {
            committed = true;
        }
    } else if wants_empty_milestone(label) {
        let out = run_git(
            project_dir,
            &["commit", "--allow-empty", "-m", &msg],
        )?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("empty milestone commit failed: {stderr}");
        }
        committed = true;
    }
    let sha = current_head(project_dir)?;
    // Skip duplicate index rows when nothing new was committed.
    if !committed && sha == before {
        return Ok(VersionNode {
            sha,
            label: label.to_string(),
            summary: summary.to_string(),
            ts: now_rfc3339(),
            chapter,
            mutation_id: mutation_id.map(|s| s.to_string()),
            meta,
        });
    }
    let node = VersionNode {
        sha: sha.clone(),
        label: label.to_string(),
        summary: summary.to_string(),
        ts: now_rfc3339(),
        chapter,
        mutation_id: mutation_id.map(|s| s.to_string()),
        meta,
    };
    append_node_record(project_dir, &node)?;
    Ok(node)
}

/// List recent nodes (newest first) from jsonl index; fall back to git log.
pub fn list_nodes(project_dir: &Path, limit: usize) -> Result<Vec<VersionNode>> {
    let limit = limit.clamp(1, 500);
    let path = nodes_path(project_dir);
    let mut nodes = Vec::new();
    if path.is_file() {
        let f = std::fs::File::open(&path)?;
        for line in std::io::BufReader::new(f).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(n) = serde_json::from_str::<VersionNode>(&line) {
                nodes.push(n);
            }
        }
        nodes.reverse();
        nodes.truncate(limit);
        return Ok(nodes);
    }
    if !git_dir(project_dir).join("HEAD").exists() {
        return Ok(vec![]);
    }
    let log = run_git_ok(
        project_dir,
        &["log", &format!("-{limit}"), "--format=%H\t%cI\t%s"],
    )?;
    for line in log.lines() {
        let mut parts = line.splitn(3, '\t');
        let sha = parts.next().unwrap_or("").to_string();
        let ts = parts.next().unwrap_or("").to_string();
        let summary = parts.next().unwrap_or("").to_string();
        if sha.is_empty() {
            continue;
        }
        nodes.push(VersionNode {
            sha,
            label: "git".into(),
            summary,
            ts,
            chapter: None,
            mutation_id: None,
            meta: None,
        });
    }
    Ok(nodes)
}

/// Align worktree + index to `sha`, deleting paths that exist only after that commit.
/// Respects `info/exclude` (studio_thread, ops_journal, versions.git, sqlite, …).
fn checkout_tree_exact(project_dir: &Path, full_sha: &str) -> Result<()> {
    // Replace index with target tree, materialize files, then drop leftovers.
    run_git_ok(project_dir, &["read-tree", full_sha])?;
    run_git_ok(project_dir, &["checkout-index", "-a", "-f"])?;
    // Untracked / removed-from-index files (not excluded) → delete.
    let clean = run_git(project_dir, &["clean", "-fd"])?;
    if !clean.status.success() {
        let stderr = String::from_utf8_lossy(&clean.stderr);
        bail!("git clean after restore failed: {stderr}");
    }
    Ok(())
}

/// Restore worktree to `sha`. Creates a `pre-restore/<ts>` node first.
pub fn restore_node(project_dir: &Path, sha: &str) -> Result<VersionNode> {
    let sha = sha.trim();
    if sha.is_empty() {
        bail!("sha 必填");
    }
    ensure_repo(project_dir)?;
    let full = run_git_ok(
        project_dir,
        &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
    )?;
    let pre = commit_node(
        project_dir,
        &format!("pre-restore/{}", now_label_ts()),
        &format!("safety checkpoint before restore to {full}"),
        None,
        None,
        Some(json!({ "target_sha": full })),
    )?;
    checkout_tree_exact(project_dir, &full)?;
    // Disk hooks: indexes / meta (lore sqlite wiped + rebuilt elsewhere too).
    let _ = post_restore_disk_hooks(project_dir);
    let node = commit_node(
        project_dir,
        &format!("restore/{full}"),
        &format!("restored worktree to {full} (pre={})", pre.sha),
        None,
        None,
        Some(json!({
            "restored_sha": full,
            "pre_restore_sha": pre.sha,
        })),
    )?;
    Ok(node)
}

/// Refresh derived indexes after a content restore (best-effort).
pub fn post_restore_disk_hooks(project_dir: &Path) -> Result<()> {
    let _ = crate::project::refresh_meta_flags(project_dir);
    let _ = crate::plots::rebuild_plot_index(project_dir);
    let _ = crate::chapter_index::rebuild_index(project_dir);
    let _ = crate::foreshadow::rebuild_foreshadow_index(project_dir);
    crate::lore_index::rebuild_lore_index_from_disk(project_dir)?;
    Ok(())
}

pub fn nodes_to_json(nodes: &[VersionNode]) -> Value {
    json!(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::init_project;

    #[test]
    fn commit_list_restore_roundtrip() {
        if !git_available() {
            eprintln!("skip: git not available");
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "novelx-ver-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let dir = init_project(&root, "sample-novel", "未定", 10).unwrap();
        ensure_repo(&dir).unwrap();
        let draft = dir.join("chapters/001/draft.md");
        std::fs::create_dir_all(draft.parent().unwrap()).unwrap();
        std::fs::write(&draft, "version-a\n").unwrap();
        let n1 = commit_node(&dir, "chapter/1", "first draft", Some(1), None, None).unwrap();
        assert!(!n1.sha.is_empty());
        std::fs::write(&draft, "version-b\n").unwrap();
        let _n2 = commit_node(&dir, "chapter/1b", "second draft", Some(1), None, None).unwrap();
        let listed = list_nodes(&dir, 10).unwrap();
        assert!(listed.len() >= 2);
        restore_node(&dir, &n1.sha).unwrap();
        let body = std::fs::read_to_string(&draft).unwrap();
        assert!(body.contains("version-a"), "{body}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_deletes_files_absent_from_target() {
        if !git_available() {
            eprintln!("skip: git not available");
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "novelx-ver-del-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let dir = init_project(&root, "sample-novel", "未定", 10).unwrap();
        ensure_repo(&dir).unwrap();
        let d1 = dir.join("chapters/001/draft.md");
        std::fs::create_dir_all(d1.parent().unwrap()).unwrap();
        std::fs::write(&d1, "ch1\n").unwrap();
        let n1 = commit_node(&dir, "chapter/1", "only ch1", Some(1), None, None).unwrap();
        let d2 = dir.join("chapters/002/draft.md");
        std::fs::create_dir_all(d2.parent().unwrap()).unwrap();
        std::fs::write(&d2, "ch2-should-vanish\n").unwrap();
        let _ = commit_node(&dir, "chapter/2", "added ch2", Some(2), None, None).unwrap();
        assert!(d2.is_file());
        restore_node(&dir, &n1.sha).unwrap();
        assert!(d1.is_file(), "ch1 draft should remain");
        assert!(
            !d2.exists(),
            "ch2 must be deleted when restoring to pre-ch2 node"
        );
        // Excluded session file must survive.
        let thread = dir.join(".novelx/studio_thread.json");
        std::fs::create_dir_all(thread.parent().unwrap()).unwrap();
        std::fs::write(&thread, "{\"keep\":true}\n").unwrap();
        let n_mid = commit_node(&dir, "chapter/1x", "mid", Some(1), None, None).unwrap();
        std::fs::write(&d1, "changed\n").unwrap();
        let _ = commit_node(&dir, "chapter/1y", "changed", Some(1), None, None).unwrap();
        restore_node(&dir, &n_mid.sha).unwrap();
        assert!(thread.is_file(), "studio_thread must not be wiped by restore");
        let _ = std::fs::remove_dir_all(&root);
    }
}
