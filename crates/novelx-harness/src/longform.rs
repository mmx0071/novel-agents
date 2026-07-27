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
    #[serde(default = "default_soft_short_streak")]
    pub soft_short_auto_revise_after: u32,
}

fn default_batch_max() -> u32 {
    20
}
fn default_soft_short_streak() -> u32 {
    3
}

impl Default for LongformConfig {
    fn default() -> Self {
        Self {
            quality_tier: QualityTier::Balanced,
            audit_tier: AuditTier::Layered,
            impact_scan_mode: ImpactScanMode::Indexed,
            batch_max_chapters: default_batch_max(),
            soft_short_auto_revise_after: default_soft_short_streak(),
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
        assert_eq!(c.soft_short_auto_revise_after, 3);
    }

    #[test]
    fn parses_file() {
        let dir = std::env::temp_dir().join("novelx-longform-cfg");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("longform.yaml");
        std::fs::write(
            &path,
            "quality_tier: economy\naudit_tier: full\nimpact_scan_mode: volume\nbatch_max_chapters: 5\n",
        )
        .unwrap();
        let c = LongformConfig::load(&path);
        assert_eq!(c.quality_tier, QualityTier::Economy);
        assert_eq!(c.audit_tier, AuditTier::Full);
        assert_eq!(c.impact_scan_mode, ImpactScanMode::Volume);
        assert_eq!(c.batch_max_chapters, 5);
    }
}
