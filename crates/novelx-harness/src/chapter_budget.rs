//! Load `config/chapter.yaml` — chapter word targets for writer prompts.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct ChapterBudget {
    #[serde(default = "default_word_min")]
    pub word_min: u32,
    #[serde(default = "default_word_max")]
    pub word_max: u32,
}

fn default_word_min() -> u32 {
    3000
}
fn default_word_max() -> u32 {
    5000
}

impl Default for ChapterBudget {
    fn default() -> Self {
        Self {
            word_min: default_word_min(),
            word_max: default_word_max(),
        }
    }
}

impl ChapterBudget {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_yaml::from_str::<ChapterBudget>(&text) {
            Ok(mut b) => {
                if b.word_min == 0 {
                    b.word_min = default_word_min();
                }
                if b.word_max < b.word_min {
                    b.word_max = b.word_min;
                }
                b
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse chapter.yaml");
                Self::default()
            }
        }
    }

    pub fn load_from_config_root(config_root: &Path) -> Self {
        Self::load(&config_root.join("chapter.yaml"))
    }

    /// e.g. `3000–5000`
    pub fn range_label(&self) -> String {
        format!("{}–{}", self.word_min, self.word_max)
    }

    pub fn writer_target_line(&self) -> String {
        format!("请撰写 {} 字正文 Markdown。", self.range_label())
    }

    pub fn revise_length_hint(&self) -> String {
        format!(
            "字数要求：按章纲与指令扩写/重写为完整一章，目标约 {} 字（除非指令另有明确字数）。",
            self.range_label()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_range() {
        let dir = std::env::temp_dir().join("novelx-chapter-budget-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("chapter.yaml");
        std::fs::write(&path, "word_min: 2000\nword_max: 4000\n").unwrap();
        let b = ChapterBudget::load(&path);
        assert_eq!(b.word_min, 2000);
        assert_eq!(b.word_max, 4000);
        assert!(b.range_label().contains("2000"));
    }
}
