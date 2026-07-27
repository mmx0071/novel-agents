//! Load `config/naming_rules.yaml` — anti DeepSeek-flavored / corpus-common names.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NamingRules {
    #[serde(default)]
    pub forbidden_names: Vec<String>,
    #[serde(default)]
    pub naming_principles: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
}

impl NamingRules {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            tracing::warn!(path = %path.display(), "naming_rules.yaml missing; using empty rules");
            return Self::default();
        };
        match serde_yaml::from_str::<NamingRules>(&text) {
            Ok(rules) => {
                tracing::info!(
                    forbidden = rules.forbidden_names.len(),
                    "naming rules loaded"
                );
                rules
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse naming_rules.yaml");
                Self::default()
            }
        }
    }

    pub fn load_from_config_root(config_root: &Path) -> Self {
        Self::load(&config_root.join("naming_rules.yaml"))
    }

    /// Validate YAML text for Web PUT.
    pub fn parse_yaml(text: &str) -> Result<Self, String> {
        serde_yaml::from_str::<NamingRules>(text)
            .map_err(|e| format!("naming_rules.yaml 解析失败：{e}"))
    }

    /// Compact block for LLM system/user prompts.
    pub fn prompt_block(&self) -> String {
        if self.forbidden_names.is_empty() && self.naming_principles.is_empty() {
            return String::new();
        }
        let mut out = String::from("# 取名硬约束（反语料脸谱名）\n");
        if !self.forbidden_names.is_empty() {
            out.push_str("禁止使用下列名字及明显变体（DeepSeek/网文高频脸谱）：\n");
            for n in &self.forbidden_names {
                out.push_str(&format!("- {n}\n"));
            }
        }
        if !self.naming_principles.is_empty() {
            out.push_str("\n原则：\n");
            for p in &self.naming_principles {
                out.push_str(&format!("- {p}\n"));
            }
        }
        out
    }

    pub fn find_in_text(&self, text: &str) -> Vec<String> {
        self.forbidden_names
            .iter()
            .filter(|n| !n.is_empty() && text.contains(n.as_str()))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn loads_forbidden_names_from_yaml() {
        let dir = std::env::temp_dir().join("novelx-naming-rules-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("naming_rules.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "forbidden_names:\n  - 林远\n  - 苏瑶\nnaming_principles:\n  - test\n"
        )
        .unwrap();
        let rules = NamingRules::load(&path);
        assert!(rules.forbidden_names.contains(&"林远".into()));
        assert!(rules.find_in_text("主角林远出场").len() == 1);
        assert!(!rules.prompt_block().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
