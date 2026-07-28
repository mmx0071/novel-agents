//! Arc / volume outline: `artifacts/arc_outlines/{NN}.md` (one file per volume).

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
    let t = normalize_arc_outline_input(text);
    if t.is_empty() {
        return Err(SchemaError::new(
            "arc_outline",
            "arc_outline.md",
            "卷纲不能为空",
        ));
    }
    let h1 = h1_line(&t).unwrap_or_default();
    if !h1_looks_like_volume(&h1) {
        return Err(SchemaError::new(
            "arc_outline",
            "title",
            format!(
                "首个 H1 须为「# 第X卷 · …」，当前：{}",
                if h1.is_empty() { "（缺失）" } else { &h1 }
            ),
        ));
    }
    let mut missing = Vec::new();
    for (canon, aliases) in REQUIRED {
        if !has_h2(&t, aliases) && !has_h2_starts_with(&t, canon) {
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
    let n = list_items_under_h2(&t, &["卷末终止条件"]);
    if n < 2 {
        return Err(SchemaError::new(
            "arc_outline",
            "卷末终止条件",
            format!("「## 卷末终止条件」下至少 2 条列表项，当前 {n}"),
        ));
    }
    if starts_with_chat_preamble(&t) {
        return Err(SchemaError::new(
            "arc_outline",
            "wrapper",
            "卷纲不得含寒暄/修订说明；请只输出以「# 第X卷 · …」开头的 Markdown 正文",
        ));
    }
    Ok(display_arc_outline(&t))
}

pub fn display_arc_outline(text: &str) -> String {
    rewrite_h2_aliases(normalize_arc_outline_input(text).trim(), ALIAS_MAP)
}

/// Strip LLM chat wrappers / code fences / trailing revision notes before schema checks.
pub fn normalize_arc_outline_input(text: &str) -> String {
    let raw = text.trim();
    if raw.is_empty() {
        return String::new();
    }
    let body = if let Some(inner) = extract_fenced_markdown(raw) {
        inner
    } else {
        slice_from_volume_h1(raw).to_string()
    };
    let body = strip_trailing_meta(&body);
    slice_from_volume_h1(body.trim()).trim().to_string()
}

fn extract_fenced_markdown(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after_ticks = &rest[start + 3..];
        let after_lang = after_ticks
            .strip_prefix("markdown")
            .or_else(|| after_ticks.strip_prefix("md"))
            .unwrap_or(after_ticks);
        let after_lang = after_lang
            .strip_prefix('\r')
            .unwrap_or(after_lang)
            .strip_prefix('\n')
            .unwrap_or(after_lang);
        let end = after_lang.find("```")?;
        let inner = after_lang[..end].trim();
        if arc_has_required_signal(inner) {
            return Some(inner.to_string());
        }
        rest = &after_lang[end + 3..];
    }
    None
}

fn arc_has_required_signal(text: &str) -> bool {
    has_h2(text, &["卷定位"])
        || has_h2(text, &["卷末终止条件"])
        || text.lines().any(|l| {
            let t = l.trim();
            t.starts_with("# ")
                && !t.starts_with("## ")
                && h1_looks_like_volume(t.trim_start_matches("# ").trim())
        })
}

fn slice_from_volume_h1(text: &str) -> &str {
    let mut offset = 0usize;
    for line in text.lines() {
        let tr = line.trim();
        if tr.starts_with("# ")
            && !tr.starts_with("## ")
            && h1_looks_like_volume(tr.trim_start_matches("# ").trim())
        {
            return text[offset..].trim();
        }
        offset += line.len();
        if text[offset..].starts_with('\n') {
            offset += 1;
        } else if text[offset..].starts_with("\r\n") {
            offset += 2;
        }
    }
    text
}

fn strip_trailing_meta(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut cut = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("**修订影响")
            || t.starts_with("**修订说明")
            || t.starts_with("修订影响声明")
            || t.starts_with("修订说明")
            || (t.starts_with("**") && (t.contains("影响声明") || t.contains("修订声明")))
        {
            cut = i;
            break;
        }
        // Drop leftover fence closers / chat after the outline body.
        if t == "```" && i + 1 < lines.len() {
            let rest = lines[i + 1..].join("\n");
            if rest.contains("修订") || rest.contains("影响声明") || rest.trim_start().starts_with("**")
            {
                cut = i;
                break;
            }
        }
    }
    // Also drop auto volume-end stubs if they ever land in the planner body.
    for (i, line) in lines.iter().enumerate().take(cut) {
        let t = line.trim();
        if t.starts_with("## 卷末进度（自动）") || t.starts_with("## 卷末修订（自动）") {
            cut = i;
            break;
        }
    }
    lines[..cut].join("\n").trim().to_string()
}

fn starts_with_chat_preamble(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("").trim();
    !first.starts_with('#')
        && (first.contains("这是根据")
            || first.contains("根据你的")
            || first.contains("我将")
            || first.contains("好的")
            || first.contains("修订要求"))
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

    #[test]
    fn strips_chat_wrapper_and_fence() {
        let raw = format!(
            "这是根据你的修订要求，对第1卷卷纲进行精准更新后的结果。\n\n```markdown\n{}\n```\n\n**修订影响声明**：\n- 改了一步\n",
            sample().trim()
        );
        let out = validate_arc_outline(&raw).expect("normalize+validate");
        assert!(out.starts_with("# 第1卷 · 试卷"), "{out}");
        assert!(!out.contains("这是根据"));
        assert!(!out.contains("```"));
        assert!(!out.contains("修订影响"));
        assert!(out.contains("## 卷末终止条件"));
    }
}
