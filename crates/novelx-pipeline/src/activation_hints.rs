//! Surface non-pipeline activation suggestions (e.g. volume_auditor) for Studio.

use crate::phases::{resolve_volume_phase, VolumePhase};
use crate::project::ProjectState;
use crate::volume::{active_volume_for_chapter, volume_chapter_span};
use novelx_harness::{collect_signals, evaluate_activation, ActivationSuggestion};
use std::path::Path;

/// Studio-visible hint when an extended agent should be invoked via tool (not chapter pipeline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudioActivationHint {
    pub agent: String,
    pub reason: String,
    /// Concrete tool call guidance for Studio, e.g. `audit_volume(project=\"…\")`.
    pub tool_hint: String,
}

fn enrich_signals(
    project_dir: &Path,
    state: &ProjectState,
    chapter: u32,
    draft: &str,
) -> novelx_harness::ActivationSignals {
    let mut signals = collect_signals(project_dir, state.published_count, draft);
    signals.chapter = chapter.max(1);
    if let Some(vol) = active_volume_for_chapter(project_dir, signals.chapter) {
        let (start, end) = volume_chapter_span(&vol, signals.chapter);
        signals.chapters_in_current_arc = end.saturating_sub(start).saturating_add(1);
    } else {
        signals.chapters_in_current_arc = state.published_count.min(signals.chapter);
    }
    let phase = resolve_volume_phase(project_dir);
    signals.volume_handoff = matches!(
        phase,
        VolumePhase::AwaitingSync | VolumePhase::AwaitingNextArc | VolumePhase::AwaitingNextPlot
    );
    signals
}

/// Evaluate agents.yaml and return hints for agents that do **not** run in the chapter pipeline.
pub fn collect_studio_activation_hints(
    config_root: &Path,
    project_dir: &Path,
    state: &ProjectState,
    chapter: u32,
    draft: &str,
    project_name: &str,
) -> Vec<StudioActivationHint> {
    let signals = enrich_signals(project_dir, state, chapter, draft);
    let suggestions = evaluate_activation(config_root, &signals);
    suggestions
        .into_iter()
        .filter_map(|s| hint_for_suggestion(&s, project_name))
        .collect()
}

fn hint_for_suggestion(s: &ActivationSuggestion, project_name: &str) -> Option<StudioActivationHint> {
    match s.agent.as_str() {
        "material_researcher" => Some(StudioActivationHint {
            agent: s.agent.clone(),
            reason: s.reason.clone(),
            tool_hint: format!(
                "research_materials(project=\"{project_name}\", reason=\"plot_drought\")"
            ),
        }),
        "volume_auditor" => Some(StudioActivationHint {
            agent: s.agent.clone(),
            reason: s.reason.clone(),
            tool_hint: format!(
                "请调用 audit_volume(project=\"{project_name}\") 做整卷摘要层复盘（不要用 audit_chapters 代替）"
            ),
        }),
        _ => None,
    }
}

/// Single markdown block for tool output / Studio system prompt. `None` if nothing to suggest.
pub fn format_studio_activation_hints_block(hints: &[StudioActivationHint]) -> Option<String> {
    if hints.is_empty() {
        return None;
    }
    let mut lines = vec!["## 长程 QA 建议（Studio）".to_string()];
    for h in hints {
        lines.push(format!("- [{}] {} → {}", h.agent, h.reason, h.tool_hint));
    }
    Some(lines.join("\n"))
}

/// Convenience: volume_auditor (and other non-pipeline) hints for current project state.
pub fn studio_activation_hints_for_project(
    config_root: &Path,
    project_dir: &Path,
    project_name: &str,
) -> Vec<StudioActivationHint> {
    let Ok(state) = crate::project::load_project_state(project_dir) else {
        return Vec::new();
    };
    let chapter = state.next_chapter.max(state.published_count).max(1);
    let draft = crate::project::read_chapter_draft(project_dir, chapter).unwrap_or_default();
    collect_studio_activation_hints(
        config_root,
        project_dir,
        &state,
        chapter,
        &draft,
        project_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{init_project, save_project_state};
    use std::fs;

    #[test]
    fn volume_auditor_hint_when_arc_long() {
        let root = std::env::temp_dir().join("novelx-vol-auditor-hint");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let projects = root.join("projects");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("agents.yaml"),
            r#"
agents:
  volume_auditor:
    name: 卷级复盘
    layer: qa
    tier: extended
    description: test
    activation:
      - condition: chapters_in_arc_gt
        threshold: 40
        reason: 本卷已写 40+ 章
"#,
        )
        .unwrap();
        init_project(&projects, "sample-novel", "未定", 900).unwrap();
        let dir = projects.join("sample-novel");
        let mut state = crate::project::load_project_state(&dir).unwrap();
        state.published_count = 45;
        state.next_chapter = 46;
        save_project_state(&dir, &state).unwrap();
        // Default volume span → chapters_in_current_arc ≈ 45 when no bounds.
        let hints =
            collect_studio_activation_hints(&config, &dir, &state, 46, "", "sample-novel");
        assert!(
            hints.iter().any(|h| h.agent == "volume_auditor"),
            "expected volume_auditor hint, got {hints:?}"
        );
        let block = format_studio_activation_hints_block(&hints).unwrap();
        assert!(block.contains("audit_volume"));
        assert!(block.contains("sample-novel"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_hint_when_arc_short() {
        let root = std::env::temp_dir().join("novelx-vol-auditor-hint-short");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let projects = root.join("projects");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("agents.yaml"),
            r#"
agents:
  volume_auditor:
    activation:
      - condition: chapters_in_arc_gt
        threshold: 40
        reason: 本卷已写 40+ 章
"#,
        )
        .unwrap();
        init_project(&projects, "sample-novel", "未定", 100).unwrap();
        let dir = projects.join("sample-novel");
        let state = crate::project::load_project_state(&dir).unwrap();
        let hints = collect_studio_activation_hints(&config, &dir, &state, 1, "", "sample-novel");
        assert!(hints.is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
