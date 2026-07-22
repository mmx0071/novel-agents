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
        m.insert("studio.clear_history_on_new_chapter".into(), true);
        m.insert("studio.auto_reaudit_after_steer".into(), true);
        m.insert("studio.reject_weak_ui_turns".into(), true);
        m.insert("studio.enforce_setup_gate".into(), true);
        m.insert("studio.enforce_volume_phase".into(), true);
        Self { map: Arc::new(m) }
    }

    pub fn enabled(&self, key: &str) -> bool {
        self.map.get(key).copied().unwrap_or(false)
    }

    pub fn deterministic_intents(&self) -> bool {
        self.enabled("studio.deterministic_intents")
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
}
