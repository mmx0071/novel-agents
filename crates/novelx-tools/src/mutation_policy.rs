//! Load `config/mutation_policy.yaml` — routine vs high mutation severity.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
struct File {
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    high_tools: Vec<String>,
    #[serde(default)]
    routine_tools: Vec<String>,
    #[serde(default)]
    impact: ImpactPolicyFile,
}

fn default_mode() -> String {
    "severity".into()
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ImpactPolicyFile {
    #[serde(default = "default_auto_max_hits")]
    auto_max_hits: usize,
    #[serde(default)]
    never_auto_sources: Vec<String>,
    #[serde(default)]
    never_auto_targets: Vec<String>,
    #[serde(default = "default_true")]
    block_auto_on_draft_targets: bool,
}

fn default_auto_max_hits() -> usize {
    5
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct ImpactAutoPolicy {
    pub auto_max_hits: usize,
    pub never_auto_sources: HashSet<String>,
    pub never_auto_targets: HashSet<String>,
    pub block_auto_on_draft_targets: bool,
}

impl Default for ImpactAutoPolicy {
    fn default() -> Self {
        Self {
            auto_max_hits: 5,
            never_auto_sources: HashSet::from([
                "bible".into(),
                "master_outline".into(),
            ]),
            never_auto_targets: HashSet::from([
                "draft".into(),
                "arc_outline".into(),
                "master_outline".into(),
                "bible".into(),
            ]),
            block_auto_on_draft_targets: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MutationPolicy {
    /// When true, only high tools require human confirm.
    pub severity_mode: bool,
    high: HashSet<String>,
    routine: HashSet<String>,
    pub impact: ImpactAutoPolicy,
}

impl Default for MutationPolicy {
    fn default() -> Self {
        let mut high = HashSet::new();
        for t in [
            "delete_entity",
            "design_master_outline",
            "design_arc_outline",
            "upsert_setting",
            "supplement_setting",
            "replan_volume",
            "restore_version_node",
        ] {
            high.insert(t.into());
        }
        let mut routine = HashSet::new();
        for t in [
            "continue_writing",
            "continue_writing_batch",
            "revise_chapter",
            "split_chapter",
            "revise_outline",
            "design_plot",
            "update_plot",
            "design_entity",
            "activate_agents",
            "enqueue_expected_event",
            "update_expected_event",
            "resolve_expected_event",
        ] {
            routine.insert(t.into());
        }
        Self {
            severity_mode: true,
            high,
            routine,
            impact: ImpactAutoPolicy::default(),
        }
    }
}

impl MutationPolicy {
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("mutation_policy.yaml");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(file) = serde_yaml::from_str::<File>(&raw) else {
            tracing::warn!(path = %path.display(), "mutation_policy.yaml parse failed; defaults");
            return Self::default();
        };
        let severity_mode = file.mode.trim() == "severity";
        let high: HashSet<String> = file.high_tools.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let routine: HashSet<String> = file
            .routine_tools
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let mut never_auto_targets: HashSet<String> = file
            .impact
            .never_auto_targets
            .into_iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if never_auto_targets.is_empty() {
            never_auto_targets = ImpactAutoPolicy::default().never_auto_targets;
        }
        let impact = ImpactAutoPolicy {
            auto_max_hits: file.impact.auto_max_hits.max(1),
            never_auto_sources: file
                .impact
                .never_auto_sources
                .into_iter()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            never_auto_targets,
            block_auto_on_draft_targets: file.impact.block_auto_on_draft_targets,
        };
        Self {
            severity_mode,
            high: if high.is_empty() {
                Self::default().high
            } else {
                high
            },
            routine: if routine.is_empty() {
                Self::default().routine
            } else {
                routine
            },
            impact,
        }
    }

    pub fn is_high(&self, kind: &str) -> bool {
        let k = kind.trim();
        if self.high.contains(k) {
            return true;
        }
        if self.routine.contains(k) {
            return false;
        }
        // Unknown → high (safer).
        true
    }

    pub fn is_routine(&self, kind: &str) -> bool {
        !self.is_high(kind)
    }
}

/// Whether `studio.mutation_severity_policy` is on (features.yaml).
pub fn severity_policy_enabled(config_root: &Path) -> bool {
    let path = config_root.join("features.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return true;
    };
    #[derive(Deserialize)]
    struct FeaturesFile {
        #[serde(default)]
        features: std::collections::HashMap<String, bool>,
    }
    let Ok(file) = serde_yaml::from_str::<FeaturesFile>(&raw) else {
        return true;
    };
    file.features
        .get("studio.mutation_severity_policy")
        .copied()
        .unwrap_or(true)
}

/// Cached policy for hot path (reload when mtime changes).
pub fn cached_policy(config_root: &Path) -> MutationPolicy {
    static CACHE: OnceLock<std::sync::Mutex<Option<(std::time::SystemTime, MutationPolicy)>>> =
        OnceLock::new();
    let path = config_root.join("mutation_policy.yaml");
    let mtime = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let lock = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((ts, pol)) = guard.as_ref() {
        if *ts == mtime {
            return pol.clone();
        }
    }
    let pol = MutationPolicy::load(config_root);
    *guard = Some((mtime, pol.clone()));
    pol
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn routine_vs_high() {
        let p = MutationPolicy::default();
        assert!(p.is_routine("continue_writing"));
        assert!(p.is_high("delete_entity"));
        assert!(p.is_high("unknown_tool"));
    }

    #[test]
    fn loads_yaml() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-mutation-policy-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("mutation_policy.yaml")).unwrap();
        write!(
            f,
            "version: 1\nmode: severity\nhigh_tools: [delete_entity]\nroutine_tools: [continue_writing]\nimpact:\n  auto_max_hits: 3\n"
        )
        .unwrap();
        let p = MutationPolicy::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(p.severity_mode);
        assert!(p.is_routine("continue_writing"));
        assert!(p.is_high("delete_entity"));
        assert_eq!(p.impact.auto_max_hits, 3);
    }
}
