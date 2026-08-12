//! Agent invocation catalog from `config/agents.yaml`.
//!
//! Separates pipeline steps / domain tools / SubAgent bypass (`allow_spawn`).

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentInvocation {
    Orchestration,
    Pipeline,
    DomainTool,
    InProcess,
    #[default]
    #[serde(other)]
    Unknown,
}

impl AgentInvocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orchestration => "orchestration",
            Self::Pipeline => "pipeline",
            Self::DomainTool => "domain_tool",
            Self::InProcess => "in_process",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentCatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub invocation: AgentInvocation,
    pub allow_spawn: bool,
    /// Optional tool whitelist for SubAgent bypass (Phase B). Empty = no extra restrict.
    pub tools: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AgentsFile {
    #[serde(default)]
    agents: std::collections::HashMap<String, AgentYaml>,
}

#[derive(Debug, Deserialize)]
struct AgentYaml {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    invocation: AgentInvocation,
    #[serde(default)]
    allow_spawn: bool,
    #[serde(default)]
    tools: Vec<String>,
}

fn load_file(config_root: &Path) -> Option<AgentsFile> {
    let path = config_root.join("agents.yaml");
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&text).ok()
}

/// Load all agent catalog entries (sorted by id).
pub fn load_agent_catalog(config_root: &Path) -> Vec<AgentCatalogEntry> {
    let Some(file) = load_file(config_root) else {
        return Vec::new();
    };
    let mut out: Vec<AgentCatalogEntry> = file
        .agents
        .into_iter()
        .map(|(id, a)| AgentCatalogEntry {
            id,
            name: a.name,
            description: a.description,
            invocation: a.invocation,
            allow_spawn: a.allow_spawn,
            tools: a.tools,
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn lookup_agent(config_root: &Path, role: &str) -> Option<AgentCatalogEntry> {
    load_agent_catalog(config_root)
        .into_iter()
        .find(|e| e.id == role)
}

/// Spawn allowed only when YAML explicitly sets `allow_spawn: true`.
/// Unknown / missing role → false (safe default).
pub fn is_spawn_allowed(config_root: &Path, role: &str) -> bool {
    if role.is_empty() || matches!(role, "studio_agent" | "orchestrator") {
        return false;
    }
    lookup_agent(config_root, role)
        .map(|e| e.allow_spawn)
        .unwrap_or(false)
}

pub fn list_spawnable_agents(config_root: &Path) -> Vec<AgentCatalogEntry> {
    load_agent_catalog(config_root)
        .into_iter()
        .filter(|e| e.allow_spawn)
        .collect()
}

/// Optional tools whitelist for a spawnable role. `None` = role unknown or no whitelist field.
pub fn spawn_tools_for_role(config_root: &Path, role: &str) -> Option<Vec<String>> {
    let entry = lookup_agent(config_root, role)?;
    if entry.tools.is_empty() {
        None
    } else {
        Some(entry.tools)
    }
}

/// Pipeline-handler roles use `execute_single_agent_step` after spawn.
/// Domain-tool / unwired roles use the LLM tool loop (must publish_result for wait_agent).
pub fn subagent_uses_pipeline_step(config_root: &Path, role: &str) -> bool {
    crate::PipelineConfig::load(config_root)
        .handler_for(role)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_config() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config")
    }

    #[test]
    fn repo_spawn_whitelist() {
        let root = repo_config();
        if !root.join("agents.yaml").is_file() {
            return;
        }
        assert!(!is_spawn_allowed(&root, "writer"));
        assert!(!is_spawn_allowed(&root, "decision_council"));
        assert!(!is_spawn_allowed(&root, "studio_agent"));
        assert!(!is_spawn_allowed(&root, "volume_auditor"));
        assert!(!is_spawn_allowed(&root, "unknown_role_xyz"));
        assert!(is_spawn_allowed(&root, "literary_editor"));
        assert!(is_spawn_allowed(&root, "material_researcher"));
        assert!(is_spawn_allowed(&root, "setting_auditor"));
        let spawnable = list_spawnable_agents(&root);
        let ids: Vec<_> = spawnable.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"literary_editor"));
        assert!(ids.contains(&"material_researcher"));
        assert!(ids.contains(&"setting_auditor"));
        assert!(!ids.contains(&"writer"));
    }

    #[test]
    fn repo_invocations_and_tools() {
        let root = repo_config();
        if !root.join("agents.yaml").is_file() {
            return;
        }
        let writer = lookup_agent(&root, "writer").expect("writer");
        assert_eq!(writer.invocation, AgentInvocation::Pipeline);
        assert!(!writer.allow_spawn);
        let lore = lookup_agent(&root, "lore_librarian").expect("lore");
        assert_eq!(lore.invocation, AgentInvocation::InProcess);
        let tools = spawn_tools_for_role(&root, "literary_editor").expect("tools");
        assert!(tools.iter().any(|t| t == "read_chapter"));
        assert!(spawn_tools_for_role(&root, "writer").is_none());
        let setting_tools = spawn_tools_for_role(&root, "setting_auditor").expect("setting tools");
        assert!(
            setting_tools.iter().any(|t| t == "audit_setting"),
            "setting_auditor must include audit_setting: {setting_tools:?}"
        );
        assert!(subagent_uses_pipeline_step(&root, "literary_editor"));
        assert!(!subagent_uses_pipeline_step(&root, "material_researcher"));
        assert!(!subagent_uses_pipeline_step(&root, "setting_auditor"));
    }
}
