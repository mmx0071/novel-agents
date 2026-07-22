//! Arc / volume outline: `artifacts/arc_outline.md`.

use super::error::SchemaError;
use super::md::{h1_line, has_h2, list_items_under_h2, rewrite_h2_aliases};

const REQUIRED: &[(&str, &[&str])] = &[
    ("卷定位", &["卷定位"]),
    ("开卷状态", &["开卷状态"]),
    ("冲突升级阶梯", &["冲突升级阶梯"]),
    ("关键节点", &["关键节点"]),
    ("人物弧", &["人物弧", "人物弧（本卷）"]),
    ("伏笔", &["伏笔"]),
    ("卷末终止条件", &["卷末终止条件"]),
    ("卷末交付", &["卷末交付"]),
];

const ALIAS_MAP: &[(&str, &[&str])] = &[
    ("卷定位", &["卷定位"]),
    ("开卷状态", &["开卷状态"]),
    ("冲突升级阶梯", &["冲突升级阶梯"]),
    ("关键节点", &["关键节点"]),
    ("人物弧", &["人物弧", "人物弧（本卷）"]),
    ("伏笔", &["伏笔"]),
    ("卷末终止条件", &["卷末终止条件"]),
    ("卷末交付", &["卷末交付"]),
];

pub fn validate_arc_outline(text: &str) -> Result<String, SchemaError> {
    let t = text.trim();
    if t.is_empty() {
        return Err(SchemaError::new(
            "arc_outline",
            "arc_outline.md",
            "卷纲不能为空",
        ));
    }
    let h1 = h1_line(t).unwrap_or_default();
    if !h1_looks_like_volume(&h1) {
        return Err(SchemaError::new(
            "arc_outline",
            "title",
            format!("首个 H1 须为「# 第X卷 · …」，当前：{}", if h1.is_empty() { "（缺失）" } else { &h1 }),
        ));
    }
    let mut missing = Vec::new();
    for (canon, aliases) in REQUIRED {
        if !has_h2(t, aliases) && !has_h2_starts_with(t, canon) {
            missing.push(format!("## {canon}"));
        }
    }
    if !missing.is_empty() {
        return Err(SchemaError::new(
            "arc_outline",
            "sections",
            format!("缺少必填节：{}", missing.join("、")),
        ));
    }
    let n = list_items_under_h2(t, &["卷末终止条件"]);
    if n < 2 {
        return Err(SchemaError::new(
            "arc_outline",
            "卷末终止条件",
            format!("「## 卷末终止条件」下至少 2 条列表项，当前 {n}"),
        ));
    }
    Ok(display_arc_outline(t))
}

pub fn display_arc_outline(text: &str) -> String {
    rewrite_h2_aliases(text.trim(), ALIAS_MAP)
}

fn h1_looks_like_volume(h1: &str) -> bool {
    let t = h1.trim();
    // 第X卷 …
    let rest = t.trim_start_matches('第').trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    rest[digits.len()..].trim_start().starts_with('卷')
}

fn has_h2_starts_with(text: &str, prefix: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        t.strip_prefix("## ")
            .map(|r| r.trim().starts_with(prefix))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        r#"# 第1卷 · 试卷

## 卷定位
- 承诺

## 开卷状态
- 状态

## 冲突升级阶梯
1. a
2. b

## 关键节点
- A

## 人物弧
- 主角

## 伏笔
- 埋

## 卷末终止条件
- 条件一兑现
- 条件二兑现

## 卷末交付
- 钩子
"#
        .into()
    }

    #[test]
    fn accepts_valid() {
        assert!(validate_arc_outline(&sample()).is_ok());
    }

    #[test]
    fn rejects_missing_ending() {
        let t = sample().replace("## 卷末终止条件\n- 条件一兑现\n- 条件二兑现\n\n", "");
        let e = validate_arc_outline(&t).unwrap_err();
        assert!(e.message.contains("卷末终止条件"));
    }

    #[test]
    fn rejects_one_ending() {
        let t = sample().replace(
            "## 卷末终止条件\n- 条件一兑现\n- 条件二兑现\n",
            "## 卷末终止条件\n- 仅一条\n",
        );
        let e = validate_arc_outline(&t).unwrap_err();
        assert!(e.message.contains("至少 2"));
    }
}
