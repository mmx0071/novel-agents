//! Short-drama script shape gates from `config/script.yaml`.

use serde::Deserialize;
use std::path::Path;

/// Hard shape rules for `episodes/NNN/script.md` (validate_script).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ScriptShapeConfig {
    /// Minimum total `【画面】` markers in the script body.
    #[serde(default = "default_picture_min_total")]
    pub picture_min_total: u32,
    /// Minimum `【画面】` markers per `## 场` section.
    #[serde(default = "default_picture_min_per_scene")]
    pub picture_min_per_scene: u32,
    /// Substrings forbidden in the title line (case-insensitive for ASCII).
    #[serde(default = "default_reject_title_contains")]
    pub reject_title_contains: Vec<String>,
    /// Standalone title-suffix tokens stripped during normalize (e.g. 「章」).
    #[serde(default = "default_strip_title_noise")]
    pub strip_title_noise: Vec<String>,
}

fn default_picture_min_total() -> u32 {
    2
}
fn default_picture_min_per_scene() -> u32 {
    1
}
fn default_reject_title_contains() -> Vec<String> {
    vec!["```".into(), "markdown".into()]
}
fn default_strip_title_noise() -> Vec<String> {
    vec!["章".into()]
}

impl Default for ScriptShapeConfig {
    fn default() -> Self {
        Self {
            picture_min_total: default_picture_min_total(),
            picture_min_per_scene: default_picture_min_per_scene(),
            reject_title_contains: default_reject_title_contains(),
            strip_title_noise: default_strip_title_noise(),
        }
    }
}

impl ScriptShapeConfig {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_yaml::from_str::<ScriptShapeConfig>(&text) {
            Ok(mut c) => {
                if c.picture_min_total == 0 {
                    c.picture_min_total = default_picture_min_total();
                }
                if c.picture_min_per_scene == 0 {
                    c.picture_min_per_scene = default_picture_min_per_scene();
                }
                if c.reject_title_contains.is_empty() {
                    c.reject_title_contains = default_reject_title_contains();
                }
                if c.strip_title_noise.is_empty() {
                    c.strip_title_noise = default_strip_title_noise();
                }
                c
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "failed to parse script shape from yaml");
                Self::default()
            }
        }
    }

    pub fn load_from_config_root(config_root: &Path) -> Self {
        let script = config_root.join("script.yaml");
        if script.exists() {
            Self::load(&script)
        } else {
            Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn loads_shape_knobs_from_yaml() {
        let root = std::env::temp_dir().join(format!(
            "novelx-script-shape-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("script.yaml"),
            r#"
version: 1
word_min: 800
word_max: 3500
picture_min_total: 3
picture_min_per_scene: 1
reject_title_contains:
  - "```"
  - "markdown"
strip_title_noise:
  - "章"
"#,
        )
        .unwrap();
        let c = ScriptShapeConfig::load_from_config_root(&root);
        assert_eq!(c.picture_min_total, 3);
        assert_eq!(c.picture_min_per_scene, 1);
        assert!(c.reject_title_contains.iter().any(|s| s == "```"));
        assert_eq!(c.strip_title_noise, vec!["章".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn defaults_when_missing_file() {
        let root = std::env::temp_dir().join(format!(
            "novelx-script-shape-miss-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let c = ScriptShapeConfig::load_from_config_root(&root);
        assert_eq!(c, ScriptShapeConfig::default());
        let _ = fs::remove_dir_all(&root);
    }
}
