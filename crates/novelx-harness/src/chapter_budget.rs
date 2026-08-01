//! Load `config/chapter.yaml` — chapter word targets for writer prompts.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthAssessment {
    /// Within soft band [`word_min`, `word_max`].
    Ok,
    /// Below `word_min` but at/above `word_hard_min` — warn, do not block publish
    /// (unless soft-short streak forces escalate).
    SoftShort,
    /// Below `word_hard_min` — block publish.
    HardShort,
    /// Above `word_max` but at/below `word_hard_max` — warn, do not block.
    SoftLong,
    /// Above `word_hard_max` — block publish (split or compress).
    HardLong,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChapterBudget {
    #[serde(default = "default_word_min")]
    pub word_min: u32,
    #[serde(default = "default_word_max")]
    pub word_max: u32,
    #[serde(default = "default_word_hard_min")]
    pub word_hard_min: u32,
    /// Above this → HardLong (block). 0 = disable hard-long gate.
    #[serde(default = "default_word_hard_max")]
    pub word_hard_max: u32,
    /// When consecutive SoftShort reaches this, SoftShort escalates to block publish.
    /// 0 = never escalate. Overridden by `longform.yaml` when both set (longform wins if >0).
    #[serde(default = "default_soft_short_auto")]
    pub soft_short_auto_revise_after: u32,
}

fn default_word_min() -> u32 {
    5000
}
fn default_word_max() -> u32 {
    6000
}
fn default_word_hard_min() -> u32 {
    4500
}
fn default_word_hard_max() -> u32 {
    11000
}
fn default_soft_short_auto() -> u32 {
    3
}

impl Default for ChapterBudget {
    fn default() -> Self {
        Self {
            word_min: default_word_min(),
            word_max: default_word_max(),
            word_hard_min: default_word_hard_min(),
            word_hard_max: default_word_hard_max(),
            soft_short_auto_revise_after: default_soft_short_auto(),
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
                if b.word_hard_min == 0 {
                    b.word_hard_min = default_word_hard_min();
                }
                if b.word_hard_min > b.word_min {
                    b.word_hard_min = b.word_min;
                }
                // 0 keeps hard-long disabled; otherwise ensure ≥ word_max.
                if b.word_hard_max > 0 && b.word_hard_max < b.word_max {
                    b.word_hard_max = b.word_max;
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
        let mut b = Self::load(&config_root.join("chapter.yaml"));
        // longform.yaml may override streak threshold.
        let lf = crate::longform::LongformConfig::load_from_config_root(config_root);
        if lf.soft_short_auto_revise_after > 0 {
            b.soft_short_auto_revise_after = lf.soft_short_auto_revise_after;
        }
        b
    }

    /// e.g. `5000–6000`
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

    /// Assess body length (chars after title line).
    pub fn assess_body_chars(&self, body_chars: usize) -> LengthAssessment {
        let hard_min = self.word_hard_min as usize;
        let soft_min = self.word_min as usize;
        let soft_max = self.word_max as usize;
        let hard_max = self.word_hard_max as usize;
        if body_chars < hard_min {
            LengthAssessment::HardShort
        } else if body_chars < soft_min {
            LengthAssessment::SoftShort
        } else if hard_max > 0 && body_chars > hard_max {
            LengthAssessment::HardLong
        } else if body_chars > soft_max {
            LengthAssessment::SoftLong
        } else {
            LengthAssessment::Ok
        }
    }

    pub fn hard_short_message(&self, body_chars: usize) -> String {
        format!(
            "正文字数不足（硬门控）：当前 {body_chars} 字，至少需要 {} 字才能发布（创作目标 {}）",
            self.word_hard_min,
            self.range_label()
        )
    }

    pub fn soft_short_message(&self, body_chars: usize) -> String {
        format!(
            "正文字数偏短（软警告）：当前 {body_chars} 字，创作目标 {}（不阻断发布）",
            self.range_label()
        )
    }

    pub fn soft_short_escalate_message(&self, body_chars: usize, streak: u32) -> String {
        format!(
            "正文字数连续偏短（已连续 {streak} 章未达 {}）：当前 {body_chars} 字，阻断发布，请扩写后再发布",
            self.range_label()
        )
    }

    pub fn soft_long_message(&self, body_chars: usize) -> String {
        format!(
            "正文字数偏长（软警告）：当前 {body_chars} 字，创作目标 {}（不阻断发布）",
            self.range_label()
        )
    }

    pub fn hard_long_message(&self, body_chars: usize) -> String {
        format!(
            "正文字数严重超限（硬门控）：当前 {body_chars} 字，上限 {} 字（创作目标 {}）。系统将自动拆成两章；若失败请 split_chapter 或压缩后再发布",
            self.word_hard_max,
            self.range_label()
        )
    }

    pub fn compress_revise_instructions(&self) -> String {
        format!(
            "本章严重超长。压缩到约 {} 字完整一章：删除重复机理/复述，保留情节推进与章末钩子；勿另起主线，勿注水反写更长。",
            self.range_label()
        )
    }
}

/// Read consecutive SoftShort counter from project state meta.
pub fn consecutive_soft_short_from_meta(meta: &std::collections::HashMap<String, serde_json::Value>) -> u32 {
    meta.get("consecutive_soft_short")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
}

pub fn set_consecutive_soft_short(
    meta: &mut std::collections::HashMap<String, serde_json::Value>,
    n: u32,
) {
    if n == 0 {
        meta.remove("consecutive_soft_short");
    } else {
        meta.insert(
            "consecutive_soft_short".into(),
            serde_json::json!(n),
        );
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
        std::fs::write(
            &path,
            "word_min: 2000\nword_max: 4000\nword_hard_min: 1500\n",
        )
        .unwrap();
        let b = ChapterBudget::load(&path);
        assert_eq!(b.word_min, 2000);
        assert_eq!(b.word_max, 4000);
        assert_eq!(b.word_hard_min, 1500);
        assert!(b.range_label().contains("2000"));
    }

    #[test]
    fn assess_hard_soft_ok() {
        let b = ChapterBudget::default();
        assert_eq!(b.assess_body_chars(4400), LengthAssessment::HardShort);
        assert_eq!(b.assess_body_chars(4500), LengthAssessment::SoftShort);
        assert_eq!(b.assess_body_chars(5000), LengthAssessment::Ok);
        assert_eq!(b.assess_body_chars(5600), LengthAssessment::Ok);
        assert_eq!(b.assess_body_chars(6500), LengthAssessment::SoftLong);
        assert_eq!(b.assess_body_chars(11000), LengthAssessment::SoftLong);
        assert_eq!(b.assess_body_chars(11001), LengthAssessment::HardLong);
    }

    #[test]
    fn clamps_hard_min_above_soft() {
        let dir = std::env::temp_dir().join("novelx-chapter-budget-clamp");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("chapter.yaml");
        std::fs::write(
            &path,
            "word_min: 5000\nword_max: 6000\nword_hard_min: 5500\n",
        )
        .unwrap();
        let b = ChapterBudget::load(&path);
        assert_eq!(b.word_hard_min, 5000);
    }

    #[test]
    fn soft_short_meta_roundtrip() {
        let mut meta = std::collections::HashMap::new();
        assert_eq!(consecutive_soft_short_from_meta(&meta), 0);
        set_consecutive_soft_short(&mut meta, 2);
        assert_eq!(consecutive_soft_short_from_meta(&meta), 2);
        set_consecutive_soft_short(&mut meta, 0);
        assert_eq!(consecutive_soft_short_from_meta(&meta), 0);
    }
}
