//! Master outline: `artifacts/master_outline.md`.

use super::error::SchemaError;
use super::md::{has_h2, rewrite_h2_aliases};

const ALIAS_MAP: &[(&str, &[&str])] = &[
    ("一句话卖点", &["一句话卖点", "Logline", "logline", "卖点"]),
    ("三幕结构", &["三幕结构", "分卷", "分卷结构", "三幕结构（映射为多卷）"]),
    ("主角弧", &["主角弧", "主角弧光", "人物弧（主角）"]),
    ("主线冲突", &["主线冲突", "核心冲突", "主冲突"]),
];

pub fn validate_master_outline(text: &str) -> Result<String, SchemaError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(SchemaError::new(
            "master_outline",
            "master_outline.md",
            "总纲不能为空",
        ));
    }
    let mut missing = Vec::new();
    if !has_h2(t, &["一句话卖点", "Logline", "logline", "卖点"]) {
        missing.push("## 一句话卖点（或 ## Logline）");
    }
    if !has_h2(t, &["三幕结构", "分卷", "分卷结构"])
        && !has_h2_prefix(t, "三幕结构")
        && !has_h2_prefix(t, "分卷")
    {
        missing.push("## 三幕结构 或 ## 分卷");
    }
    if !has_h2(t, &["主角弧", "主角弧光"]) && !has_h2_prefix(t, "主角弧") {
        missing.push("## 主角弧");
    }
    if !has_h2(t, &["主线冲突", "核心冲突", "主冲突"]) && !has_h2_prefix(t, "主线冲突") {
        missing.push("## 主线冲突");
    }
    if !missing.is_empty() {
        return Err(SchemaError::new(
            "master_outline",
            "sections",
            format!("缺少必填节：{}", missing.join("、")),
        ));
    }
    Ok(display_master_outline(t))
}

pub fn display_master_outline(text: &str) -> String {
    rewrite_h2_aliases(text.trim(), ALIAS_MAP)
}

fn has_h2_prefix(text: &str, prefix: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let r = rest.trim();
            r == prefix || r.starts_with(&format!("{prefix}（")) || r.starts_with(&format!("{prefix}("))
                || r.starts_with(&format!("{prefix} ·"))
                || r.starts_with(prefix)
        } else {
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"# 总纲

## 一句话卖点
卖点句。

## 三幕结构
起承转合。

## 主角弧
成长。

## 主线冲突
对抗。
"#
    }

    #[test]
    fn accepts_valid() {
        assert!(validate_master_outline(sample()).is_ok());
    }

    #[test]
    fn accepts_logline_alias() {
        let t = sample().replace("## 一句话卖点", "## Logline");
        assert!(validate_master_outline(&t).is_ok());
    }

    #[test]
    fn rejects_missing_conflict() {
        let t = sample().replace("## 主线冲突\n对抗。\n", "");
        let e = validate_master_outline(&t).unwrap_err();
        assert!(e.message.contains("主线冲突"));
    }
}
