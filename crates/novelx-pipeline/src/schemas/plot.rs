//! Plot cards: `plots/*.md` (YAML FM + fixed H2 body).

use super::error::SchemaError;
use super::md::{has_h2, join_fm, rewrite_h2_aliases, split_fm};
use std::collections::HashMap;

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

fn meta_missing(meta: &HashMap<String, String>, key: &str) -> bool {
    meta.get(key).map(|s| s.trim().is_empty()).unwrap_or(true)
}

fn coerce_needs_bridge(raw: &str) -> Option<&'static str> {
    let s = raw.trim().to_ascii_lowercase();
    match s.as_str() {
        "true" | "yes" | "1" | "y" | "需要" | "要" | "是" => Some("true"),
        "false" | "no" | "0" | "n" | "不需要" | "不要" | "否" => Some("false"),
        _ => None,
    }
}

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
    // Agent/legacy cards may omit newer keys — fill schema defaults before hard fail.
    let mut meta = meta;
    if meta_missing(&meta, "scope") {
        meta.insert("scope".into(), "local".into());
    }
    if meta_missing(&meta, "plot_type") {
        meta.insert("plot_type".into(), "main".into());
    }
    if meta_missing(&meta, "needs_bridge") {
        meta.insert("needs_bridge".into(), "false".into());
    }
    if meta_missing(&meta, "status") {
        meta.insert("status".into(), "planned".into());
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
    let Some(nb_norm) = coerce_needs_bridge(nb) else {
        return Err(SchemaError::new(
            "plot",
            "needs_bridge",
            format!("needs_bridge 须为 true/false，当前「{nb}」"),
        ));
    };
    meta.insert("needs_bridge".into(), nb_norm.into());

    // 卷纲统揽整卷；剧情卡只能是卷内一段。legacy `scope: volume` → `local`。
    let scope = meta
        .get("scope")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if scope == "volume" || scope == "整卷" || scope.is_empty() {
        meta.insert("scope".into(), "local".into());
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

/// Pipeline / design_plot path: **never discard** model output on schema mismatch.
///
/// Fills missing FM keys and required H2 sections (placeholder when unknown), then
/// returns `(text, repairs)`. Empty `repairs` means input already validated cleanly.
/// Only empty input yields a minimal skeleton.
pub fn normalize_plot_card_best_effort(title: &str, text: &str) -> (String, Vec<String>) {
    let title = title.trim();
    let title = if title.is_empty() { "未命名剧情" } else { title };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (
            skeleton_plot_card(title),
            vec!["生成内容为空，已写入最小骨架".into()],
        );
    }

    // Prefer strict validate when already compliant (preserves original ordering).
    if let Ok(ok) = validate_plot_card(trimmed) {
        return (ok, Vec::new());
    }

    let mut issues = Vec::new();
    let (mut meta, mut body) = split_fm(trimmed);
    if meta.is_empty() && !trimmed.starts_with("---") {
        body = trimmed.to_string();
        issues.push("缺少 frontmatter，已自动补全".into());
    }

    if meta_missing(&meta, "title") {
        meta.insert("title".into(), title.to_string());
        issues.push("已补全 title".into());
    }
    if meta_missing(&meta, "scope") {
        meta.insert("scope".into(), "local".into());
        issues.push("已补全 scope=local".into());
    }
    if meta_missing(&meta, "plot_type") {
        meta.insert("plot_type".into(), "main".into());
        issues.push("已补全 plot_type=main".into());
    }
    if meta_missing(&meta, "status") {
        meta.insert("status".into(), "planned".into());
        issues.push("已补全 status=planned".into());
    }
    if meta_missing(&meta, "needs_bridge") {
        meta.insert("needs_bridge".into(), "false".into());
        issues.push("已补全 needs_bridge=false".into());
    } else {
        let raw = meta.get("needs_bridge").cloned().unwrap_or_default();
        match coerce_needs_bridge(&raw) {
            Some(norm) => {
                if raw.trim() != norm {
                    issues.push(format!("needs_bridge「{raw}」已规范为 {norm}"));
                }
                meta.insert("needs_bridge".into(), norm.into());
            }
            None => {
                meta.insert("needs_bridge".into(), "false".into());
                issues.push(format!("needs_bridge「{raw}」无法识别，已改为 false"));
            }
        }
    }
    if meta_missing(&meta, "category") {
        meta.insert("category".into(), "plot".into());
    }

    let scope = meta
        .get("scope")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if scope == "volume" || scope == "整卷" {
        meta.insert("scope".into(), "local".into());
        issues.push("scope=volume 已改为 local".into());
    }

    let body_trim = body.trim();
    if !body_trim.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("# ") && !t.starts_with("## ")
    }) {
        body = if body_trim.is_empty() {
            format!("# {title}\n")
        } else {
            format!("# {title}\n\n{body_trim}\n")
        };
        issues.push("已补全标题 H1".into());
    }

    // Fold leading prose (before first H2) into ## 概览 so generation is not stranded.
    if !has_h2(&body, &["概览"]) {
        if let Some((prose, rest)) = split_leading_prose(&body) {
            body = insert_overview_section(&rest, &prose);
            issues.push("已将生成正文收入 ## 概览".into());
        }
    }

    // Salvage exit_condition into 收束条件 when section missing.
    if !has_h2(&body, &["收束条件", "落点条件", "exit_condition"]) {
        if let Some(exit) = meta.get("exit_condition").filter(|s| !s.trim().is_empty()) {
            body = format!(
                "{}\n\n## 收束条件\n\n{}\n",
                body.trim_end(),
                exit.trim()
            );
            issues.push("已从 exit_condition 补全 ## 收束条件".into());
        }
    }

    for (canon, aliases) in BODY_SECTIONS {
        if !has_h2(&body, aliases) {
            let placeholder = if *canon == "出场人物" {
                "- （待补全）"
            } else {
                "（待补全）"
            };
            body = format!("{}\n\n## {canon}\n\n{placeholder}\n", body.trim_end());
            issues.push(format!("已补全章节 ## {canon}"));
        }
    }

    let norm_body = rewrite_h2_aliases(body.trim(), ALIAS_MAP);
    let out = join_fm(&meta, &norm_body);
    match validate_plot_card(&out) {
        Ok(ok) => (ok, issues),
        Err(e) => {
            // Last resort: keep repaired text rather than discard generation.
            issues.push(format!("形状仍有问题，已尽力保留原文：{}", e.message));
            (out, issues)
        }
    }
}

fn skeleton_plot_card(title: &str) -> String {
    format!(
        "---\ntitle: {title}\nscope: local\nplot_type: main\nstatus: planned\nneeds_bridge: false\ncategory: plot\n---\n\n\
         # {title}\n\n\
         ## 概览\n\n（待补全）\n\n\
         ## 剧情走向\n\n（待补全）\n\n\
         ## 冲突与赌注\n\n（待补全）\n\n\
         ## 出场人物\n\n- （待补全）\n\n\
         ## 收束条件\n\n（待补全）\n"
    )
}

/// Split leading prose (after optional H1, before first H2) from the remainder.
/// Returns `(prose, rest_with_optional_h1_and_h2s)`, or `None` if no usable prose.
fn split_leading_prose(body: &str) -> Option<(String, String)> {
    let mut kept_prefix: Vec<&str> = Vec::new(); // H1 only
    let mut prose: Vec<&str> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();

    let mut lines = body.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") {
            rest.push(line);
            rest.extend(lines.by_ref());
            break;
        }
        if kept_prefix.is_empty() && trimmed.starts_with("# ") && !trimmed.starts_with("## ") {
            kept_prefix.push(line);
            continue;
        }
        prose.push(line);
    }

    let prose = prose.join("\n").trim().to_string();
    if prose.is_empty() {
        return None;
    }
    let mut rest_out = String::new();
    if !kept_prefix.is_empty() {
        rest_out.push_str(&kept_prefix.join("\n"));
        if !rest.is_empty() {
            rest_out.push_str("\n\n");
            rest_out.push_str(&rest.join("\n"));
        }
    } else if !rest.is_empty() {
        rest_out.push_str(&rest.join("\n"));
    }
    Some((prose, rest_out.trim().to_string()))
}

/// Place `## 概览` after optional H1, before any remaining H2 sections.
fn insert_overview_section(rest: &str, prose: &str) -> String {
    let rest = rest.trim();
    if rest.is_empty() {
        return format!("## 概览\n\n{prose}\n");
    }
    let mut lines = rest.lines();
    let first = lines.next().unwrap_or("").trim_start();
    if first.starts_with("# ") && !first.starts_with("## ") {
        let h1 = first;
        let after = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        if after.is_empty() {
            format!("{h1}\n\n## 概览\n\n{prose}\n")
        } else {
            format!("{h1}\n\n## 概览\n\n{prose}\n\n{after}\n")
        }
    } else {
        format!("## 概览\n\n{prose}\n\n{rest}\n")
    }
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
        assert!(out.contains("scope: local"), "volume scope coerced to local");
        assert!(!out.contains("scope: volume"));
    }

    #[test]
    fn defaults_missing_scope_plot_type_needs_bridge() {
        let t = r#"---
title: 旧卡
status: planned
---

# 旧卡

## 概览
x

## 剧情走向
y

## 冲突与赌注
z

## 出场人物
- 甲

## 收束条件
抵达
"#;
        let out = validate_plot_card(t).unwrap();
        assert!(out.contains("scope: local"), "{out}");
        assert!(out.contains("plot_type: main"), "{out}");
        assert!(out.contains("needs_bridge: false"), "{out}");
    }

    #[test]
    fn best_effort_keeps_prose_and_fills_fm() {
        let raw = "主角发现密文线索，并在旧站对上号。\n\n相关人物：甲、乙。";
        let (out, repairs) = normalize_plot_card_best_effort("密文初现", raw);
        assert!(!repairs.is_empty(), "should report repairs");
        assert!(out.contains("主角发现密文线索"), "must keep generated prose: {out}");
        assert!(
            out.find("## 概览").unwrap() < out.find("主角发现密文线索").unwrap(),
            "prose should live under 概览: {out}"
        );
        assert!(validate_plot_card(&out).is_ok(), "{out}");
        assert!(out.contains("scope: local"), "{out}");
    }

    #[test]
    fn best_effort_repairs_incomplete_fm_without_discard() {
        let raw = "---\ntitle: 残缺卡\nstatus: planned\n---\n\n# 残缺卡\n\n正文还在。\n";
        let (out, repairs) = normalize_plot_card_best_effort("残缺卡", raw);
        assert!(repairs.iter().any(|r| r.contains("scope") || r.contains("plot_type") || r.contains("needs_bridge")));
        assert!(out.contains("正文还在"), "{out}");
        assert!(validate_plot_card(&out).is_ok(), "{out}");
    }

    #[test]
    fn best_effort_empty_yields_skeleton() {
        let (out, repairs) = normalize_plot_card_best_effort("空卡", "");
        assert!(repairs.iter().any(|r| r.contains("空")));
        assert!(validate_plot_card(&out).is_ok(), "{out}");
    }
}
