//! Load `config/longform.yaml` — ultra-longform quality / scan / batch knobs.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    Economy,
    #[default]
    Balanced,
    Quality,
}

impl QualityTier {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "economy" | "lean" => Self::Economy,
            "quality" | "full" => Self::Quality,
            _ => Self::Balanced,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Economy => "economy",
            Self::Balanced => "balanced",
            Self::Quality => "quality",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuditTier {
    Full,
    #[default]
    Layered,
}

impl AuditTier {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" | "always_full" => Self::Full,
            _ => Self::Layered,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Layered => "layered",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImpactScanMode {
    Volume,
    #[default]
    Indexed,
    All,
}

impl ImpactScanMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "all" | "full" => Self::All,
            "volume" | "active_volume" => Self::Volume,
            _ => Self::Indexed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Volume => "volume",
            Self::Indexed => "indexed",
            Self::All => "all",
        }
    }
}

/// Foreshadow debt tiers for batch brake vs Studio inventory.
///
/// Batch only counts **pressure** debt (overdue near/mid). Fresh plants within
/// grace and explicit `far` / low-urgency long-horizon lines do not block
/// recent-chapter publishing.
#[derive(Debug, Clone, Deserialize)]
pub struct ForeshadowDebtConfig {
    /// Chapters after plant before an open thread can count as pressure debt.
    #[serde(default = "default_debt_grace")]
    pub grace_chapters: u32,
    /// Age above this (and not explicit far) is treated as long-horizon for batch
    /// (excluded from brake; still visible in total dangling).
    #[serde(default = "default_debt_far_after")]
    pub far_after_chapters: u32,
    /// When true, mid-band overdue (between grace and far_after) counts for batch.
    #[serde(default = "default_true")]
    pub batch_count_mid: bool,
}

fn default_debt_grace() -> u32 {
    6
}
fn default_debt_far_after() -> u32 {
    40
}
fn default_true() -> bool {
    true
}

impl Default for ForeshadowDebtConfig {
    fn default() -> Self {
        Self {
            grace_chapters: default_debt_grace(),
            far_after_chapters: default_debt_far_after(),
            batch_count_mid: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LongformConfig {
    #[serde(default)]
    pub quality_tier: QualityTier,
    #[serde(default)]
    pub audit_tier: AuditTier,
    #[serde(default)]
    pub impact_scan_mode: ImpactScanMode,
    #[serde(default = "default_batch_max")]
    pub batch_max_chapters: u32,
    /// Max auto-revise attempts per chapter in continue_writing_batch (hard/consistency/length).
    #[serde(default = "default_batch_max_auto_revise")]
    pub batch_max_auto_revise: u32,
    #[serde(default = "default_soft_short_streak")]
    pub soft_short_auto_revise_after: u32,
    /// Pause continue_writing_batch when **pressure** foreshadow debt exceeds this (0 = off).
    /// Pressure = overdue near (+ mid if enabled); not total dangling / not far-horizon.
    #[serde(default = "default_batch_max_dangling")]
    pub batch_max_dangling_foreshadow: u32,
    #[serde(default)]
    pub foreshadow_debt: ForeshadowDebtConfig,
}

fn default_batch_max() -> u32 {
    20
}
fn default_batch_max_auto_revise() -> u32 {
    2
}
fn default_soft_short_streak() -> u32 {
    3
}
fn default_batch_max_dangling() -> u32 {
    40
}

impl Default for LongformConfig {
    fn default() -> Self {
        Self {
            quality_tier: QualityTier::Balanced,
            audit_tier: AuditTier::Layered,
            impact_scan_mode: ImpactScanMode::Indexed,
            batch_max_chapters: default_batch_max(),
            batch_max_auto_revise: default_batch_max_auto_revise(),
            soft_short_auto_revise_after: default_soft_short_streak(),
            batch_max_dangling_foreshadow: default_batch_max_dangling(),
            foreshadow_debt: ForeshadowDebtConfig::default(),
        }
    }
}

impl LongformConfig {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_yaml::from_str::<LongformConfig>(&text) {
            Ok(mut c) => {
                if c.batch_max_chapters == 0 {
                    c.batch_max_chapters = default_batch_max();
                }
                if c.batch_max_auto_revise == 0 {
                    c.batch_max_auto_revise = default_batch_max_auto_revise();
                }
                c.batch_max_auto_revise = c.batch_max_auto_revise.min(5);
                c
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse longform.yaml");
                Self::default()
            }
        }
    }

    pub fn load_from_config_root(config_root: &Path) -> Self {
        Self::load(&config_root.join("longform.yaml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_longform_safe() {
        let c = LongformConfig::default();
        assert_eq!(c.quality_tier, QualityTier::Balanced);
        assert_eq!(c.audit_tier, AuditTier::Layered);
        assert_eq!(c.impact_scan_mode, ImpactScanMode::Indexed);
        assert_eq!(c.batch_max_chapters, 20);
        assert_eq!(c.batch_max_auto_revise, 2);
        assert_eq!(c.soft_short_auto_revise_after, 3);
        assert_eq!(c.foreshadow_debt.grace_chapters, 6);
        assert_eq!(c.foreshadow_debt.far_after_chapters, 40);
        assert!(c.foreshadow_debt.batch_count_mid);
    }

    #[test]
    fn parses_file() {
        let dir = std::env::temp_dir().join("novelx-longform-cfg");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("longform.yaml");
        std::fs::write(
            &path,
            "quality_tier: economy\naudit_tier: full\nimpact_scan_mode: volume\nbatch_max_chapters: 5\n\
             foreshadow_debt:\n  grace_chapters: 3\n  far_after_chapters: 20\n  batch_count_mid: false\n",
        )
        .unwrap();
        let c = LongformConfig::load(&path);
        assert_eq!(c.quality_tier, QualityTier::Economy);
        assert_eq!(c.audit_tier, AuditTier::Full);
        assert_eq!(c.impact_scan_mode, ImpactScanMode::Volume);
        assert_eq!(c.batch_max_chapters, 5);
        assert_eq!(c.foreshadow_debt.grace_chapters, 3);
        assert_eq!(c.foreshadow_debt.far_after_chapters, 20);
        assert!(!c.foreshadow_debt.batch_count_mid);
    }
}
