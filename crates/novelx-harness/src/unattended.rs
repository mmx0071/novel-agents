//! Load `config/unattended.yaml` — soft-gate / soft-phase skip defaults for unattended paths.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct SoftSkipPolicy {
    #[serde(default = "default_true")]
    pub skip_volume_audit_mid: bool,
    #[serde(default = "default_true")]
    pub skip_expected_review: bool,
    /// Skip foreshadow `pressure_high` soft phase on batch / auto-continue.
    #[serde(default = "default_true")]
    pub skip_foreshadow_pressure: bool,
}

impl Default for SoftSkipPolicy {
    fn default() -> Self {
        Self {
            skip_volume_audit_mid: true,
            skip_expected_review: true,
            skip_foreshadow_pressure: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnattendedPolicy {
    #[serde(default)]
    pub batch: SoftSkipPolicy,
    #[serde(default)]
    pub council_auto_continue: SoftSkipPolicy,
    #[serde(default = "default_true")]
    pub record_skipped_soft_gates: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UnattendedPolicy {
    fn default() -> Self {
        Self {
            batch: SoftSkipPolicy::default(),
            council_auto_continue: SoftSkipPolicy::default(),
            record_skipped_soft_gates: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BatchSoftSkips {
    pub skip_volume_audit: bool,
    pub skip_expected: bool,
    pub skip_foreshadow_pressure: bool,
    /// True when at least one skip came from unattended policy (for summary notes).
    pub applied_by_policy: bool,
}

#[derive(Debug, Deserialize)]
struct FeaturesFile {
    #[serde(default)]
    features: HashMap<String, bool>,
}

impl UnattendedPolicy {
    pub fn load_from_config_root(config_root: &Path) -> Self {
        let path = config_root.join("unattended.yaml");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_yaml::from_str::<Self>(&raw).unwrap_or_default()
    }

    /// Master switch: `studio.unattended_soft_skip` (default true).
    pub fn soft_skip_enabled(config_root: &Path) -> bool {
        let path = config_root.join("features.yaml");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return true;
        };
        serde_yaml::from_str::<FeaturesFile>(&raw)
            .ok()
            .and_then(|f| f.features.get("studio.unattended_soft_skip").copied())
            .unwrap_or(true)
    }

    /// Resolve skip flags for batch. When `respect_soft_gates`, policy defaults are off.
    pub fn resolve_batch_skips(
        &self,
        config_root: &Path,
        respect_soft_gates: bool,
        explicit_skip_volume_audit: bool,
        explicit_skip_expected: bool,
        explicit_skip_foreshadow: bool,
    ) -> BatchSoftSkips {
        if respect_soft_gates || !Self::soft_skip_enabled(config_root) {
            return BatchSoftSkips {
                skip_volume_audit: explicit_skip_volume_audit,
                skip_expected: explicit_skip_expected,
                skip_foreshadow_pressure: explicit_skip_foreshadow,
                applied_by_policy: false,
            };
        }
        let skip_vol = explicit_skip_volume_audit || self.batch.skip_volume_audit_mid;
        let skip_exp = explicit_skip_expected || self.batch.skip_expected_review;
        let skip_fsh = explicit_skip_foreshadow || self.batch.skip_foreshadow_pressure;
        let applied_by_policy = (!explicit_skip_volume_audit && self.batch.skip_volume_audit_mid)
            || (!explicit_skip_expected && self.batch.skip_expected_review)
            || (!explicit_skip_foreshadow && self.batch.skip_foreshadow_pressure);
        BatchSoftSkips {
            skip_volume_audit: skip_vol,
            skip_expected: skip_exp,
            skip_foreshadow_pressure: skip_fsh,
            applied_by_policy: applied_by_policy && self.record_skipped_soft_gates,
        }
    }

    /// Resolve skip flags for council auto-continue (single chapter).
    /// Foreshadow soft-skip is batch-only; council never uses it.
    pub fn resolve_council_skips(&self, config_root: &Path) -> (bool, bool) {
        if !Self::soft_skip_enabled(config_root) {
            return (false, false);
        }
        (
            self.council_auto_continue.skip_volume_audit_mid,
            self.council_auto_continue.skip_expected_review,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn defaults_skip_soft_gates() {
        let p = UnattendedPolicy::default();
        assert!(p.batch.skip_volume_audit_mid);
        assert!(p.batch.skip_expected_review);
        assert!(p.batch.skip_foreshadow_pressure);
        assert!(p.council_auto_continue.skip_foreshadow_pressure);
    }

    #[test]
    fn resolve_batch_applies_policy() {
        let root = std::env::temp_dir().join("novelx-unattended-resolve");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("features.yaml"),
            "features:\n  studio.unattended_soft_skip: true\n",
        )
        .unwrap();
        let p = UnattendedPolicy::default();
        let s = p.resolve_batch_skips(&root, false, false, false, false);
        assert!(s.skip_volume_audit && s.skip_expected && s.skip_foreshadow_pressure);
        assert!(s.applied_by_policy);
        let s2 = p.resolve_batch_skips(&root, true, false, false, false);
        assert!(!s2.skip_volume_audit && !s2.skip_expected && !s2.skip_foreshadow_pressure);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_yaml_overrides() {
        let root = std::env::temp_dir().join("novelx-unattended-load");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("unattended.yaml"),
            "version: 1\nbatch:\n  skip_volume_audit_mid: false\n  skip_expected_review: true\n  skip_foreshadow_pressure: false\n",
        )
        .unwrap();
        let p = UnattendedPolicy::load_from_config_root(&root);
        assert!(!p.batch.skip_volume_audit_mid);
        assert!(p.batch.skip_expected_review);
        assert!(!p.batch.skip_foreshadow_pressure);
        let _ = fs::remove_dir_all(&root);
    }
}
