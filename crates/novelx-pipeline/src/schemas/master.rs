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
    let t = normalize_master_outline_input(text);
    if t.is_empty() {
        return Err(SchemaError::new(
            "master_outline",
            "master_outline.md",
            "总纲不能为空",
        ));
    }
    let mut missing = Vec::new();
    if !has_h2(&t, &["一句话卖点", "Logline", "logline", "卖点"]) {
        missing.push("## 一句话卖点（或 ## Logline）");
    }
    if !has_h2(&t, &["三幕结构", "分卷", "分卷结构"])
        && !has_h2_prefix(&t, "三幕结构")
        && !has_h2_prefix(&t, "分卷")
    {
        missing.push("## 三幕结构 或 ## 分卷");
    }
    if !has_h2(&t, &["主角弧", "主角弧光"]) && !has_h2_prefix(&t, "主角弧") {
        missing.push("## 主角弧");
    }
    if !has_h2(&t, &["主线冲突", "核心冲突", "主冲突"]) && !has_h2_prefix(&t, "主线冲突") {
        missing.push("## 主线冲突");
    }
    if !missing.is_empty() {
        return Err(SchemaError::new(
            "master_outline",
            "sections",
            format!("缺少必填节：{}", missing.join("、")),
        ));
    }
    // Reject leftover chat/meta wrappers that still contain required H2s inside fences.
    if looks_like_chat_wrapper(text.trim()) && extract_fenced_markdown(text.trim()).is_none() {
        // normalize already tried; if we still start with chatty lines, fail hard.
        if starts_with_chat_preamble(&t) {
            return Err(SchemaError::new(
                "master_outline",
                "wrapper",
                "总纲不得含寒暄/路径说明；请只输出以「# 总纲」开头的 Markdown 正文",
            ));
        }
    }
    Ok(display_master_outline(&t))
}

pub fn display_master_outline(text: &str) -> String {
    rewrite_h2_aliases(normalize_master_outline_input(text).trim(), ALIAS_MAP)
}

/// Strip LLM chat wrappers / code fences / trailing sync notes before schema checks.
pub fn normalize_master_outline_input(text: &str) -> String {
    let raw = text.trim();
    if raw.is_empty() {
        return String::new();
    }
    let body = if let Some(inner) = extract_fenced_markdown(raw) {
        inner
    } else {
        slice_from_outline_start(raw).to_string()
    };
    let body = strip_trailing_meta(&body);
    let body = slice_from_outline_start(body.trim()).trim().to_string();
    ensure_h1_total(&body)
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
        if outline_has_required_signal(inner) {
            return Some(inner.to_string());
        }
        rest = &after_lang[end + 3..];
    }
    None
}

fn outline_has_required_signal(text: &str) -> bool {
    has_h2(text, &["一句话卖点", "Logline", "logline", "卖点"])
        || has_h2(text, &["三幕结构", "分卷", "分卷结构"])
        || text.lines().any(|l| {
            let t = l.trim();
            t.starts_with("# 总纲") || (t.starts_with("# ") && t.contains("总纲") && !t.starts_with("## "))
        })
}

fn slice_from_outline_start(text: &str) -> &str {
    let mut offset = 0usize;
    for line in text.lines() {
        let tr = line.trim();
        let is_h1_total = tr.starts_with("# 总纲")
            || (tr.starts_with("# ") && !tr.starts_with("## ") && tr.contains("总纲"));
        let is_required_h2 = tr.starts_with("## 一句话卖点")
            || tr.starts_with("## Logline")
            || tr.starts_with("## logline")
            || tr.starts_with("## 卖点")
            || tr.starts_with("## 三幕结构")
            || tr.starts_with("## 分卷");
        if is_h1_total || is_required_h2 {
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
        if t == "---" {
            let rest = lines[i + 1..].join("\n");
            if rest.contains("story_outline.json")
                || rest.contains("文件路径")
                || rest.contains("同步 JSON")
                || rest.contains("同步JSON")
                || rest.contains("当您确认")
                || rest.contains("```json")
            {
                cut = i;
                break;
            }
        }
        if t.contains("story_outline.json")
            && (t.contains("文件路径") || t.contains("同步") || t.starts_with("**"))
        {
            cut = i;
            break;
        }
    }
    lines[..cut].join("\n").trim().to_string()
}

fn ensure_h1_total(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.lines().any(|l| {
        let tr = l.trim();
        tr.starts_with("# 总纲") || (tr.starts_with("# ") && !tr.starts_with("## ") && tr.contains("总纲"))
    }) {
        return t.to_string();
    }
    // Required H2 present but missing H1 — add a neutral title for reader consistency.
    if has_h2(t, &["一句话卖点", "Logline", "logline", "卖点"]) {
        return format!("# 总纲\n\n{t}");
    }
    t.to_string()
}

fn looks_like_chat_wrapper(text: &str) -> bool {
    let head: String = text.chars().take(80).collect();
    head.contains("总纲架构师")
        || head.contains("我将为您")
        || head.contains("文件路径")
        || text.contains("```markdown")
}

fn starts_with_chat_preamble(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("").trim();
    !first.starts_with('#')
        && (first.contains("好的")
            || first.contains("架构师")
            || first.contains("我将")
            || first.contains("根据您"))
}

fn has_h2_prefix(text: &str, prefix: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let r = rest.trim();
            r == prefix
                || r.starts_with(&format!("{prefix}（"))
                || r.starts_with(&format!("{prefix}("))
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

    #[test]
    fn strips_chat_fence_and_json_meta() {
        let dirty = r#"好的，总纲架构师。我将撰写总纲。

**文件路径:** `artifacts/master_outline.md`

```markdown
# 总纲：《样例》

## 一句话卖点
卖点。

## 三幕结构
三幕。

## 主角弧
弧。

## 主线冲突
冲突。

---

**文件路径（同步JSON）：** `story_outline.json`
此 JSON 文件仅由系统更新。
```

同步 story_outline.json：
```json
{"acts":[]}
```
"#;
        let out = validate_master_outline(dirty).unwrap();
        assert!(out.starts_with("# 总纲"));
        assert!(!out.contains("总纲架构师"));
        assert!(!out.contains("```"));
        assert!(!out.contains("story_outline.json"));
        assert!(out.contains("## 一句话卖点"));
    }
}
