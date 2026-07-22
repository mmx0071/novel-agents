//! Role → skill name + whether the role is a pipeline step agent.

use novelx_harness::PipelineConfig;
use std::path::Path;

/// Roles that may be spawned as SubAgents (excludes studio/orchestrator roots).
pub fn is_spawnable_role(role: &str) -> bool {
    !matches!(role, "studio_agent" | "orchestrator" | "")
}

pub fn skill_name_for_role(role: &str) -> String {
    role.replace('_', "-")
}

/// True when `role` is a chapter-pipeline step (`config/pipeline.yaml` order).
pub fn is_pipeline_role(role: &str, config_root: &Path) -> bool {
    PipelineConfig::load(config_root).is_pipeline_agent(role)
}

pub fn max_spawn_depth() -> u32 {
    4
}

pub fn max_children() -> usize {
    32
}
