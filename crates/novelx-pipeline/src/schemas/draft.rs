//! Chapter draft: `chapters/NNN/draft.md`.

use super::error::SchemaError;
use super::md::normalize_blank_lines;

/// Publish/schema floor for draft body (chars after the title line).
/// This is a minimum shape gate — not the writer target (see `config/chapter.yaml`, typically 5000–6000).
pub const MIN_DRAFT_BODY_CHARS: usize = 800;

/// Strict schema check for Web PUT / publish gates.
pub fn validate_draft(chapter: u32, text: &str) -> Result<String, SchemaError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SchemaError::new("draft", "draft.md", "正文不能为空"));
    }
    if is_whole_json_fence(trimmed) {
        return Err(SchemaError::new(
            "draft",
            "draft.md",
            "正文禁止整篇包在 ```json 代码块中",
        ));
    }
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or("").trim();
    let Some(title_suffix) = title_suffix_for_chapter(first, chapter) else {
        return Err(SchemaError::new(
            "draft",
            "title",
            format!("首行须为「# 第{chapter}章 …」，当前：{first}"),
        ));
    };
    // Canonicalize `# 第一章 标题` → `# 第1章 标题` (models often emit 中文数字).
    let canonical_title = format!("# 第{chapter}章{title_suffix}");
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let body_chars = body.chars().count();
    if body_chars < MIN_DRAFT_BODY_CHARS {
        return Err(SchemaError::new(
            "draft",
            "length",
            format!("正文（标题行之后）至少 {MIN_DRAFT_BODY_CHARS} 字，当前 {body_chars}"),
        ));
    }
    let rebuilt = if body.is_empty() {
        canonical_title
    } else {
        format!("{canonical_title}\n\n{body}")
    };
    Ok(normalize_blank_lines(&rebuilt))
}

/// Pipeline path: never discard model output — coerce shape, report remaining issues.
///
/// Returns `(text, issues)`. Empty `issues` means `text` passes [`validate_draft`].
/// Empty `text` only when input is empty (unrecoverable).
pub fn normalize_draft_best_effort(chapter: u32, text: &str) -> (String, Vec<String>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (String::new(), vec!["正文不能为空".into()]);
    }

    let unfenced = strip_outer_fence(trimmed);
    let coerced = coerce_draft_heading(chapter, &unfenced);
    let normalized = normalize_blank_lines(coerced.trim());

    match validate_draft(chapter, &normalized) {
        Ok(ok) => (ok, Vec::new()),
        Err(e) => (normalized, vec![e.message]),
    }
}

pub fn display_draft(text: &str) -> String {
    normalize_blank_lines(text.trim())
}

/// Char count of body after the first (title) line. Empty/missing title → whole text.
pub fn draft_body_chars(text: &str) -> usize {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let mut lines = trimmed.lines();
    let _first = lines.next();
    lines.collect::<Vec<_>>().join("\n").trim().chars().count()
}

#[cfg(test)]
mod body_chars_tests {
    use super::draft_body_chars;

    #[test]
    fn counts_after_title() {
        let text = "# 第1章 标题\n\n正文一二三四五六七八";
        assert_eq!(draft_body_chars(text), 10);
    }
}

/// Force first line to `# 第{chapter}章 …`, preserving title suffix / body when possible.
fn coerce_draft_heading(chapter: u32, text: &str) -> String {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or("").trim();
    let body = lines.collect::<Vec<_>>().join("\n");

    let title_line = if let Some(suffix) = title_suffix_for_chapter(first, chapter) {
        format!("# 第{chapter}章{suffix}")
    } else if let Some(suffix) = any_chapter_title_suffix(first) {
        // Wrong chapter number (or 中文数字 already handled above) → rewrite N only.
        format!("# 第{chapter}章{suffix}")
    } else if first.starts_with('#') {
        let rest = first.trim_start_matches('#').trim_start();
        let rest = rest
            .strip_prefix('第')
            .map(|r| {
                // Drop a leading numeral + 章 if present, keep the rest as title.
                if let Some((_, after)) = parse_leading_chapter_number(r.trim_start()) {
                    after.trim_start().strip_prefix('章').unwrap_or(after).trim_start()
                } else {
                    rest
                }
            })
            .unwrap_or(rest);
        if rest.is_empty() {
            format!("# 第{chapter}章")
        } else {
            format!("# 第{chapter}章 {rest}")
        }
    } else {
        // No H1 — keep entire text as body under a synthetic title.
        let title = format!("# 第{chapter}章");
        if trimmed.is_empty() {
            return title;
        }
        return format!("{title}\n\n{trimmed}");
    };

    let body = body.trim();
    if body.is_empty() {
        // First line was not a title; entire content may have been consumed as "first"
        // when there was only one line of prose without `#`.
        if !first.starts_with('#') && !first.is_empty() {
            format!("{title_line}\n\n{first}")
        } else {
            title_line
        }
    } else {
        format!("{title_line}\n\n{body}")
    }
}

/// Strip a single outer ```…``` fence (any language tag).
fn strip_outer_fence(text: &str) -> String {
    let t = text.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let rest = t.strip_prefix("```").unwrap_or(t);
    let mut lines = rest.lines();
    let _lang = lines.next();
    let body = lines.collect::<Vec<_>>().join("\n");
    let body = body.trim();
    let body = body.strip_suffix("```").unwrap_or(body).trim();
    body.to_string()
}

/// If `line` is a chapter H1 for `chapter`, return the text after `章` (may be empty / leading space).
fn title_suffix_for_chapter(line: &str, chapter: u32) -> Option<String> {
    let (n, suffix) = parse_chapter_heading(line)?;
    if n != chapter {
        return None;
    }
    Some(suffix)
}

/// Any `# 第N章 …` heading → suffix after 章 (used to rewrite wrong N).
fn any_chapter_title_suffix(line: &str) -> Option<String> {
    parse_chapter_heading(line).map(|(_, suffix)| suffix)
}

fn parse_chapter_heading(line: &str) -> Option<(u32, String)> {
    let rest = line.strip_prefix('#')?;
    let rest = rest.trim_start();
    if rest.starts_with('#') {
        return None;
    }
    let rest = rest.strip_prefix('第')?.trim_start();
    let (n, after_num) = parse_leading_chapter_number(rest)?;
    let after = after_num.trim_start();
    let after_zhang = after.strip_prefix('章')?;
    Some((n, after_zhang.to_string()))
}

fn parse_leading_chapter_number(s: &str) -> Option<(u32, &str)> {
    let ascii: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !ascii.is_empty() {
        let n = ascii.parse::<u32>().ok()?;
        return Some((n, &s[ascii.len()..]));
    }
    let cn_len = s
        .chars()
        .take_while(|c| "零〇一二三四五六七八九十百千两壹贰叁肆伍陆柒捌玖拾佰仟".contains(*c))
        .map(|c| c.len_utf8())
        .sum::<usize>();
    if cn_len == 0 {
        return None;
    }
    let cn = &s[..cn_len];
    let n = parse_chinese_uint(cn)?;
    Some((n, &s[cn_len..]))
}

/// Parse common Chinese integers used in 章号 (1..9999).
fn parse_chinese_uint(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut total = 0u32;
    let mut current = 0u32;
    for c in s.chars() {
        if let Some(d) = chinese_digit(c) {
            current = d;
            continue;
        }
        let unit = match c {
            '十' | '拾' => 10u32,
            '百' | '佰' => 100,
            '千' | '仟' => 1000,
            _ => return None,
        };
        if current == 0 {
            current = 1;
        }
        total += current * unit;
        current = 0;
    }
    total += current;
    if total == 0 {
        None
    } else {
        Some(total)
    }
}

fn chinese_digit(c: char) -> Option<u32> {
    match c {
        '零' | '〇' => Some(0),
        '一' | '壹' => Some(1),
        '二' | '两' | '贰' => Some(2),
        '三' | '叁' => Some(3),
        '四' | '肆' => Some(4),
        '五' | '伍' => Some(5),
        '六' | '陆' => Some(6),
        '七' | '柒' => Some(7),
        '八' | '捌' => Some(8),
        '九' | '玖' => Some(9),
        _ => None,
    }
}

fn is_whole_json_fence(text: &str) -> bool {
    let t = text.trim();
    if !t.starts_with("```") {
        return false;
    }
    let rest = t.strip_prefix("```").unwrap_or(t);
    let mut lines = rest.lines();
    let lang = lines.next().unwrap_or("").trim().to_ascii_lowercase();
    if lang != "json" && lang != "jsonc" {
        return false;
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    let body = body.trim().strip_suffix("```").unwrap_or(body.trim());
    let body = body.trim();
    body.starts_with('{') || body.starts_with('[')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_body() -> String {
        "甲".repeat(MIN_DRAFT_BODY_CHARS)
    }

    #[test]
    fn accepts_valid() {
        let text = format!("# 第2章 夜行\n\n{}\n", long_body());
        assert!(validate_draft(2, &text).is_ok());
    }

    #[test]
    fn accepts_colon_title() {
        let text = format!("# 第3章：门开\n\n{}\n", long_body());
        assert!(validate_draft(3, &text).is_ok());
    }

    #[test]
    fn normalizes_chinese_chapter_numeral() {
        let text = format!("# 第一章 已死之人的签名\n\n{}\n", long_body());
        let out = validate_draft(1, &text).unwrap();
        assert!(out.starts_with("# 第1章 已死之人的签名"));
        let (best, issues) = normalize_draft_best_effort(1, &text);
        assert!(issues.is_empty());
        assert!(best.starts_with("# 第1章 已死之人的签名"));
    }

    #[test]
    fn accepts_chinese_twelve() {
        let text = format!("# 第十二章 雨\n\n{}\n", long_body());
        let out = validate_draft(12, &text).unwrap();
        assert!(out.starts_with("# 第12章 雨"));
    }

    #[test]
    fn rejects_wrong_chapter() {
        let text = format!("# 第1章 错\n\n{}\n", long_body());
        let e = validate_draft(2, &text).unwrap_err();
        assert!(e.message.contains("第2章"));
    }

    #[test]
    fn best_effort_rewrites_wrong_chapter() {
        let text = format!("# 第1章 夜行\n\n{}\n", long_body());
        let (out, issues) = normalize_draft_best_effort(2, &text);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(out.starts_with("# 第2章 夜行"));
    }

    #[test]
    fn best_effort_strips_markdown_fence() {
        let text = format!("```markdown\n# 第1章 门\n\n{}\n```", long_body());
        let (out, issues) = normalize_draft_best_effort(1, &text);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(out.starts_with("# 第1章 门"));
    }

    #[test]
    fn best_effort_keeps_short_body() {
        let text = "# 第1章 短\n\n太短了\n";
        let (out, issues) = normalize_draft_best_effort(1, text);
        assert!(!out.is_empty());
        assert!(out.contains("太短了"));
        assert!(!issues.is_empty());
        assert!(issues[0].contains("至少"));
    }

    #[test]
    fn rejects_short() {
        let text = "# 第1章 短\n\n太短了\n";
        assert!(validate_draft(1, text).is_err());
    }

    #[test]
    fn rejects_json_fence() {
        let text = format!("```json\n{{\"a\":1}}\n```");
        let e = validate_draft(1, &text).unwrap_err();
        assert!(e.message.contains("json"));
    }

    #[test]
    fn parse_chinese_basics() {
        assert_eq!(parse_chinese_uint("一"), Some(1));
        assert_eq!(parse_chinese_uint("十"), Some(10));
        assert_eq!(parse_chinese_uint("十一"), Some(11));
        assert_eq!(parse_chinese_uint("二十"), Some(20));
        assert_eq!(parse_chinese_uint("二十一"), Some(21));
        assert_eq!(parse_chinese_uint("一百零一"), Some(101));
    }
}
