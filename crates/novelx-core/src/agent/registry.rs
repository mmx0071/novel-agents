//! Role → skill name; spawnability and full-chapter mode guards.

use std::path::Path;

/// Roles that may be spawned as SubAgents.
/// Requires `allow_spawn: true` in `config/agents.yaml` (unknown → denied).
pub fn is_spawnable_role(config_root: &Path, role: &str) -> bool {
    novelx_harness::is_spawn_allowed(config_root, role)
}

/// Human-oriented denial when spawn is blocked by catalog / hard deny.
pub fn spawn_denied_message(config_root: &Path, role: &str) -> String {
    if matches!(role, "studio_agent" | "orchestrator" | "") {
        return format!("role '{role}' is not spawnable");
    }
    if let Some(entry) = novelx_harness::lookup_agent(config_root, role) {
        return format!(
            "role '{}' (invocation={}) 不允许 spawn_agent。\
整章续写/修订/审校请用 continue_writing / revise_chapter / audit_chapter（或 audit_chapters）；\
领域操作请用对应 design_* / audit_* / research_materials 等工具。\
可 spawn 旁路目录见 list_agents(filter=spawnable)。",
            role,
            entry.invocation.as_str()
        );
    }
    format!(
        "role '{role}' 未在 agents.yaml 注册或不允许 spawn。\
可 spawn 旁路目录见 list_agents(filter=spawnable)。"
    )
}

pub fn skill_name_for_role(role: &str) -> String {
    role.replace('_', "-")
}

/// Full-chapter pipeline modes must use continue_writing / revise_chapter / audit_*.
/// SubAgent spawn is only for single-step specialist / read-only side paths.
pub fn pipeline_mode_spawn_forbidden(chapter: Option<u32>, mode: Option<&str>) -> Option<&'static str> {
    if chapter.is_none() {
        return None;
    }
    match mode {
        Some("continue") | Some("revise") | Some("audit_only") => Some(
            "整章续写/修订/审校请用 continue_writing / revise_chapter / audit_chapter（或 audit_chapters），\
勿 spawn_agent(mode=continue|revise|audit_only)；SubAgent 仅用于单步专精或只读旁路",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_config() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config")
    }

    #[test]
    fn rejects_chapter_plus_pipeline_mode() {
        assert!(pipeline_mode_spawn_forbidden(Some(3), Some("continue")).is_some());
        assert!(pipeline_mode_spawn_forbidden(Some(1), Some("revise")).is_some());
        assert!(pipeline_mode_spawn_forbidden(Some(2), Some("audit_only")).is_some());
    }

    #[test]
    fn allows_specialist_without_pipeline_mode() {
        assert!(pipeline_mode_spawn_forbidden(Some(3), None).is_none());
        assert!(pipeline_mode_spawn_forbidden(None, Some("continue")).is_none());
        assert!(pipeline_mode_spawn_forbidden(Some(1), Some("polish")).is_none());
    }

    #[test]
    fn spawn_whitelist_from_agents_yaml() {
        let root = repo_config();
        if !root.join("agents.yaml").is_file() {
            return;
        }
        assert!(!is_spawnable_role(&root, "writer"));
        assert!(!is_spawnable_role(&root, "decision_council"));
        assert!(!is_spawnable_role(&root, "studio_agent"));
        assert!(is_spawnable_role(&root, "literary_editor"));
        assert!(is_spawnable_role(&root, "material_researcher"));
        let msg = spawn_denied_message(&root, "writer");
        assert!(msg.contains("continue_writing"), "{msg}");
        assert!(msg.contains("list_agents"), "{msg}");
    }

    #[test]
    fn pipeline_roles_still_resolved_from_config() {
        use novelx_harness::PipelineConfig;
        let root = repo_config();
        if !root.join("pipeline.yaml").is_file() {
            return;
        }
        let cfg = PipelineConfig::load(&root);
        assert!(cfg.is_pipeline_agent("writer"));
        assert!(!cfg.is_pipeline_agent("studio_agent"));
    }
}

pub fn max_spawn_depth() -> u32 {
    4
}

pub fn max_children() -> usize {
    32
}
