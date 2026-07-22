//! Plot cards: `plots/*.md` (YAML FM + fixed H2 body).

use super::error::SchemaError;
use super::md::{has_h2, join_fm, rewrite_h2_aliases, split_fm};

const FM_REQUIRED: &[&str] = &["title", "scope", "plot_type", "status", "needs_bridge"];

const BODY_SECTIONS: &[(&str, &[&str])] = &[
    ("概览", &["概览"]),
    ("剧情走向", &["剧情走向", "剧情走向（叙事节点，无章号）"]),
    ("冲突与赌注", &["冲突与赌注", "冲突/赌注", "赌注"]),
    ("出场人物", &["出场人物"]),
    ("收束条件", &["收束条件", "落点条件", "exit_condition"]),
];

const ALIAS_MAP: &[(&str, &[&str])] = &[
    ("概览", &["概览"]),
    ("剧情走向", &["剧情走向", "剧情走向（叙事节点，无章号）"]),
    ("冲突与赌注", &["冲突与赌注", "冲突/赌注", "赌注"]),
    ("出场人物", &["出场人物"]),
    ("收束条件", &["收束条件", "落点条件", "exit_condition"]),
];

/// Validate full card text (frontmatter + body). Returns normalized full text.
pub fn validate_plot_card(text: &str) -> Result<String, SchemaError> {
    let (meta, body) = split_fm(text);
    if meta.is_empty() && !text.trim_start().starts_with("---") {
        return Err(SchemaError::new(
            "plot",
            "frontmatter",
            "剧情卡须含 YAML frontmatter（--- … ---）",
        ));
    }
    let mut missing_fm = Vec::new();
    for k in FM_REQUIRED {
        match meta.get(*k) {
            Some(v) if !v.trim().is_empty() => {}
            _ => missing_fm.push((*k).to_string()),
        }
    }
    if !missing_fm.is_empty() {
        return Err(SchemaError::new(
            "plot",
            "frontmatter",
            format!("缺少 frontmatter 字段：{}", missing_fm.join("、")),
        ));
    }
    // needs_bridge must be bool-ish
    let nb = meta.get("needs_bridge").map(|s| s.trim()).unwrap_or("");
    if !matches!(nb, "true" | "false" | "True" | "False" | "yes" | "no" | "1" | "0") {
        return Err(SchemaError::new(
            "plot",
            "needs_bridge",
            format!("needs_bridge 须为 true/false，当前「{nb}」"),
        ));
    }

    let mut body_work = body;
    // If exit_condition only in FM, inject ## 收束条件 so body validates.
    if !has_h2(&body_work, &["收束条件", "落点条件", "exit_condition"]) {
        if let Some(exit) = meta.get("exit_condition").filter(|s| !s.trim().is_empty()) {
            body_work = format!(
                "{}\n\n## 收束条件\n\n{}\n",
                body_work.trim_end(),
                exit.trim()
            );
        }
    }

    let mut missing_sec = Vec::new();
    for (canon, aliases) in BODY_SECTIONS {
        if !has_h2(&body_work, aliases) {
            missing_sec.push(format!("## {canon}"));
        }
    }
    if !missing_sec.is_empty() {
        return Err(SchemaError::new(
            "plot",
            "sections",
            format!("正文缺少必填节：{}", missing_sec.join("、")),
        ));
    }

    let norm_body = rewrite_h2_aliases(&body_work, ALIAS_MAP);
    Ok(join_fm(&meta, &norm_body))
}

/// Display body only (reader shows FM-stripped body today).
pub fn display_plot_card_body(text: &str) -> String {
    let (_meta, body) = split_fm(text);
    rewrite_h2_aliases(body.trim(), ALIAS_MAP)
}

/// Validate body-only edit against existing full file (for Web PUT).
pub fn validate_plot_card_body_edit(
    existing_full: &str,
    new_body: &str,
) -> Result<String, SchemaError> {
    let (meta, _) = split_fm(existing_full);
    let mut synthetic = join_fm(&meta, new_body);
    // If FM lacked required keys, still fail via validate.
    synthetic = validate_plot_card(&synthetic)?;
    let (_m, body) = split_fm(&synthetic);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"---
title: 测试卡
scope: local
plot_type: main
status: planned
needs_bridge: false
---

# 测试卡

## 概览
发生什么。

## 剧情走向
开端→落点。

## 冲突与赌注
赌注。

## 出场人物
- 甲

## 收束条件
抵达。
"#
    }

    #[test]
    fn accepts_valid() {
        assert!(validate_plot_card(sample()).is_ok());
    }

    #[test]
    fn rejects_missing_overview() {
        let t = sample().replace("## 概览\n发生什么。\n\n", "");
        let e = validate_plot_card(&t).unwrap_err();
        assert!(e.message.contains("概览"));
    }

    #[test]
    fn injects_exit_from_fm() {
        let t = r#"---
title: 卡
scope: volume
plot_type: main
status: planned
needs_bridge: true
exit_condition: 落点兑现
---

# 卡

## 概览
x

## 剧情走向
y

## 冲突与赌注
z

## 出场人物
- 甲
"#;
        let out = validate_plot_card(t).unwrap();
        assert!(out.contains("## 收束条件"));
    }
}
