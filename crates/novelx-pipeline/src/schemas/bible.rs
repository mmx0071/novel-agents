//! World bible: `artifacts/bible.md`.

use super::error::SchemaError;
use super::md::{h1_line, normalize_blank_lines};

/// Required numbered sections (at least these must appear as H2).
const REQUIRED_NUMS: &[(u32, &str)] = &[
    (0, "一句话世界"),
    (1, "时代与叙事框架"),
    (2, "全局势力与阵营"),
    (7, "开放问题"),
];

pub fn validate_bible(text: &str) -> Result<String, SchemaError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(SchemaError::new("bible", "bible.md", "世界观不能为空"));
    }
    let h1 = h1_line(t).unwrap_or_default();
    if !h1_is_world(&h1) {
        return Err(SchemaError::new(
            "bible",
            "title",
            format!(
                "首个 H1 须为「# 世界观」或「# 世界观 Bible」，当前：{}",
                if h1.is_empty() { "（缺失）" } else { &h1 }
            ),
        ));
    }
    let mut missing = Vec::new();
    for (num, label) in REQUIRED_NUMS {
        if !has_numbered_h2(t, *num) {
            missing.push(format!("## {num}. {label}"));
        }
    }
    if !missing.is_empty() {
        return Err(SchemaError::new(
            "bible",
            "sections",
            format!("缺少必填节：{}", missing.join("、")),
        ));
    }
    Ok(display_bible(t))
}

pub fn display_bible(text: &str) -> String {
    normalize_blank_lines(text.trim())
}

fn h1_is_world(h1: &str) -> bool {
    let t = h1.trim();
    t == "世界观"
        || t.eq_ignore_ascii_case("世界观 Bible")
        || t.starts_with("世界观")
        || t.eq_ignore_ascii_case("World Bible")
        || t.eq_ignore_ascii_case("Bible")
}

fn has_numbered_h2(text: &str, num: u32) -> bool {
    let prefixes = [
        format!("## {num}."),
        format!("## {num}．"),
        format!("## {num} "),
        format!("## {num}、"),
    ];
    text.lines().any(|line| {
        let t = line.trim();
        prefixes.iter().any(|p| t.starts_with(p.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"# 世界观 Bible

## 0. 一句话世界
世界。

## 1. 时代与叙事框架
时代。

## 2. 全局势力与阵营
势力。

## 7. 开放问题
待揭。
"#
    }

    #[test]
    fn accepts_valid() {
        assert!(validate_bible(sample()).is_ok());
    }

    #[test]
    fn rejects_missing_open() {
        let t = sample().replace("## 7. 开放问题\n待揭。\n", "");
        let e = validate_bible(&t).unwrap_err();
        assert!(e.message.contains("7."));
    }
}
