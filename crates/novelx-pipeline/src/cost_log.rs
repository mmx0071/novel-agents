//! Per-chapter / per-agent pipeline cost / timing log (approx tokens).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEntry {
    pub chapter: u32,
    pub agent: String,
    pub approx_tokens: u32,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostAgentSummary {
    pub agent: String,
    pub calls: u32,
    pub approx_tokens: u64,
    pub elapsed_ms: u64,
}

pub fn cost_log_path(project_dir: &Path) -> std::path::PathBuf {
    project_dir.join("lore/cost_log.jsonl")
}

pub fn append_cost_entry(project_dir: &Path, entry: &CostEntry) -> Result<()> {
    let path = cost_log_path(project_dir);
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

pub fn load_recent_entries(project_dir: &Path, limit: usize) -> Vec<CostEntry> {
    let Ok(text) = std::fs::read_to_string(cost_log_path(project_dir)) else {
        return Vec::new();
    };
    let mut all: Vec<CostEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if all.len() > limit {
        all = all.split_off(all.len() - limit);
    }
    all
}

/// Aggregate recent entries by agent (for Studio / Web visibility).
pub fn summarize_cost_by_agent(project_dir: &Path, limit: usize) -> Vec<CostAgentSummary> {
    let recent = load_recent_entries(project_dir, limit);
    let mut map: BTreeMap<String, (u32, u64, u64)> = BTreeMap::new();
    for e in &recent {
        let slot = map.entry(e.agent.clone()).or_insert((0, 0, 0));
        slot.0 = slot.0.saturating_add(1);
        slot.1 = slot.1.saturating_add(e.approx_tokens as u64);
        slot.2 = slot.2.saturating_add(e.elapsed_ms);
    }
    let mut out: Vec<CostAgentSummary> = map
        .into_iter()
        .map(|(agent, (calls, approx_tokens, elapsed_ms))| CostAgentSummary {
            agent,
            calls,
            approx_tokens,
            elapsed_ms,
        })
        .collect();
    out.sort_by(|a, b| b.approx_tokens.cmp(&a.approx_tokens));
    out
}

/// One-line summary for get_project_status (chapter means + note that steps are logged).
pub fn format_cost_status_line(project_dir: &Path) -> Option<String> {
    let recent = load_recent_entries(project_dir, 400);
    if recent.is_empty() {
        return None;
    }
    // Prefer per-agent rows; ignore `pipeline` rollup to avoid ~2× double-count.
    let mut by_chapter: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    let mut chapter_has_agent: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for e in &recent {
        if e.agent != "pipeline" {
            chapter_has_agent.insert(e.chapter);
        }
    }
    for e in &recent {
        if e.agent == "pipeline" && chapter_has_agent.contains(&e.chapter) {
            continue;
        }
        let slot = by_chapter.entry(e.chapter).or_insert((0, 0));
        slot.0 = slot.0.saturating_add(e.approx_tokens);
        slot.1 = slot.1.saturating_add(e.elapsed_ms);
    }
    let chapters: Vec<_> = by_chapter.keys().copied().collect();
    let last_n: Vec<_> = chapters.iter().rev().take(10).copied().collect();
    if last_n.is_empty() {
        return None;
    }
    let mut sum_tok = 0u64;
    let mut sum_ms = 0u64;
    for ch in &last_n {
        if let Some((t, m)) = by_chapter.get(ch) {
            sum_tok += *t as u64;
            sum_ms += *m;
        }
    }
    let n = last_n.len() as u64;
    let agent_n = recent
        .iter()
        .filter(|e| e.agent != "pipeline")
        .map(|e| e.agent.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    Some(format!(
        "成本近{}章均值≈{} tok / {} ms（记录 {} 条·约 {} 类 agent）",
        n,
        sum_tok / n.max(1),
        sum_ms / n.max(1),
        recent.len(),
        agent_n.max(1)
    ))
}

/// Compact by-agent breakdown for status / Web.
pub fn format_cost_by_agent_line(project_dir: &Path) -> Option<String> {
    let summary = summarize_cost_by_agent(project_dir, 400);
    if summary.is_empty() {
        return None;
    }
    let top: Vec<String> = summary
        .iter()
        .filter(|s| s.agent != "pipeline")
        .take(6)
        .map(|s| format!("{}≈{}k", s.agent, s.approx_tokens / 1000))
        .collect();
    if top.is_empty() {
        // Legacy logs may only have pipeline rows.
        let p = summary.iter().find(|s| s.agent == "pipeline")?;
        return Some(format!(
            "按 agent：pipeline≈{}k tok（{} 次；升级后将按步骤累计）",
            p.approx_tokens / 1000,
            p.calls
        ));
    }
    Some(format!("按 agent：{}", top.join(" · ")))
}

/// Rough char→token estimate for logging (Chinese-heavy ≈ chars).
pub fn approx_tokens_from_chars(chars: usize) -> u32 {
    chars.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn status_line_skips_pipeline_rollup_when_agent_rows_exist() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-cost-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        append_cost_entry(
            &dir,
            &CostEntry {
                chapter: 1,
                agent: "writer".into(),
                approx_tokens: 5000,
                elapsed_ms: 1000,
                note: String::new(),
            },
        )
        .unwrap();
        append_cost_entry(
            &dir,
            &CostEntry {
                chapter: 1,
                agent: "pipeline".into(),
                approx_tokens: 5000,
                elapsed_ms: 1000,
                note: "rollup".into(),
            },
        )
        .unwrap();
        let line = format_cost_status_line(&dir).unwrap();
        // Mean should be ~5000, not ~10000.
        assert!(
            line.contains("≈5000 tok") || line.contains("≈5"),
            "unexpected double-count: {line}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
