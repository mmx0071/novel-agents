//! Role → skill name; spawnability and full-chapter mode guards.

/// Roles that may be spawned as SubAgents (excludes studio root; `orchestrator` kept denied for legacy ids).
pub fn is_spawnable_role(role: &str) -> bool {
    !matches!(role, "studio_agent" | "orchestrator" | "")
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
    fn pipeline_roles_still_resolved_from_config() {
        use novelx_harness::PipelineConfig;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config");
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
