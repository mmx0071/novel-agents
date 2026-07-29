//! Config-driven feature flags (`config/features.yaml`).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
struct FeaturesFile {
    #[serde(default)]
    features: HashMap<String, bool>,
}

#[derive(Debug, Clone)]
pub struct FeatureFlags {
    map: Arc<HashMap<String, bool>>,
}

impl FeatureFlags {
    pub fn load(config_root: &Path) -> Result<Self> {
        let path = config_root.join("features.yaml");
        if !path.exists() {
            tracing::warn!(path = %path.display(), "features.yaml missing; using defaults");
            return Ok(Self::defaults());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let file: FeaturesFile = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse {}", path.display()))?;
        tracing::info!(count = file.features.len(), "studio features loaded");
        Ok(Self {
            map: Arc::new(file.features),
        })
    }

    pub fn defaults() -> Self {
        let mut m = HashMap::new();
        m.insert("studio.deterministic_intents".into(), true);
        // Default agentic: narrate after clean write before pausing on chapter_next.
        m.insert("studio.pause_after_clean_write".into(), false);
        m.insert("studio.stream_reasoning".into(), true);
        m.insert("studio.clear_history_on_new_chapter".into(), true);
        m.insert("studio.auto_reaudit_after_steer".into(), true);
        m.insert("studio.reject_weak_ui_turns".into(), true);
        m.insert("studio.enforce_setup_gate".into(), true);
        m.insert("studio.enforce_volume_phase".into(), true);
        m.insert("studio.enforce_chapter_order".into(), true);
        m.insert("studio.require_mutation_confirm".into(), true);
        m.insert("studio.impact_cascade".into(), true);
        m.insert("studio.impact_scan_all_drafts".into(), false);
        // Default false: use longform.yaml impact_scan_mode (indexed) instead of full-book scan.
        m.insert("studio.impact_scan_all_on_setting".into(), false);
        m.insert("pipeline.longform_lean".into(), true);
        m.insert("studio.require_volume_audit_mid".into(), true);
        m.insert("studio.require_volume_audit_handoff".into(), true);
        m.insert("studio.cold_archive_drafts".into(), true);
        // Append-only decision/execution audit trail under .novelx/ops_journal.jsonl.
        m.insert("studio.ops_journal".into(), true);
        Self { map: Arc::new(m) }
    }

    pub fn enabled(&self, key: &str) -> bool {
        self.map.get(key).copied().unwrap_or(false)
    }

    pub fn deterministic_intents(&self) -> bool {
        self.enabled("studio.deterministic_intents")
    }

    /// When true, clean publish after write/revise always pauses for human.
    /// When false (default), allow one LLM narration round then pause.
    pub fn pause_after_clean_write(&self) -> bool {
        self.map
            .get("studio.pause_after_clean_write")
            .copied()
            .unwrap_or(false)
    }

    pub fn stream_reasoning(&self) -> bool {
        self.map
            .get("studio.stream_reasoning")
            .copied()
            .unwrap_or(true)
    }

    pub fn clear_history_on_new_chapter(&self) -> bool {
        self.enabled("studio.clear_history_on_new_chapter")
    }

    pub fn auto_reaudit_after_steer(&self) -> bool {
        self.enabled("studio.auto_reaudit_after_steer")
    }

    pub fn reject_weak_ui_turns(&self) -> bool {
        self.enabled("studio.reject_weak_ui_turns")
    }

    pub fn enforce_setup_gate(&self) -> bool {
        // Default on when key missing.
        self.map
            .get("studio.enforce_setup_gate")
            .copied()
            .unwrap_or(true)
    }

    pub fn enforce_volume_phase(&self) -> bool {
        self.map
            .get("studio.enforce_volume_phase")
            .copied()
            .unwrap_or(true)
    }

    pub fn require_mutation_confirm(&self) -> bool {
        self.map
            .get("studio.require_mutation_confirm")
            .copied()
            .unwrap_or(true)
    }

    pub fn impact_cascade(&self) -> bool {
        self.map
            .get("studio.impact_cascade")
            .copied()
            .unwrap_or(true)
    }

    pub fn enforce_chapter_order(&self) -> bool {
        self.map
            .get("studio.enforce_chapter_order")
            .copied()
            .unwrap_or(true)
    }

    /// Persist decision/execution ops journal (tools, gates, mutations, publish).
    pub fn ops_journal(&self) -> bool {
        self.map
            .get("studio.ops_journal")
            .copied()
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_journal_defaults_on() {
        assert!(FeatureFlags::defaults().ops_journal());
        let off = FeatureFlags {
            map: Arc::new(HashMap::from([("studio.ops_journal".into(), false)])),
        };
        assert!(!off.ops_journal());
    }
}
