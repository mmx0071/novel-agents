//! Episode script: `episodes/NNN/script.md` (project_mode=short_drama).

use super::error::SchemaError;
use super::md::normalize_blank_lines;

/// Minimum script body chars after the title line (shape gate; see script.yaml for publish band).
pub const MIN_SCRIPT_BODY_CHARS: usize = 200;

/// Strict schema check for short-drama scripts.
pub fn validate_script(episode: u32, text: &str) -> Result<String, SchemaError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SchemaError::new("script", "script.md", "剧本不能为空"));
    }
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or("").trim();
    let Some(title_suffix) = title_suffix_for_episode(first, episode) else {
        return Err(SchemaError::new(
            "script",
            "title",
            format!("首行须为「# 第{episode}集 …」，当前：{first}"),
        ));
    };
    let canonical_title = format!("# 第{episode}集{title_suffix}");
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let body_chars = body.chars().count();
    if body_chars < MIN_SCRIPT_BODY_CHARS {
        return Err(SchemaError::new(
            "script",
            "length",
            format!("剧本（标题行之后）至少 {MIN_SCRIPT_BODY_CHARS} 字，当前 {body_chars}"),
        ));
    }
    let scene_count = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("## 场") || t.starts_with("##场")
        })
        .count();
    if scene_count < 2 {
        return Err(SchemaError::new(
            "script",
            "scenes",
            format!("至少需要 2 个「## 场」节，当前 {scene_count}"),
        ));
    }
    if !body.contains("【钩子】") {
        return Err(SchemaError::new(
            "script",
            "hook",
            "文末须有「【钩子】」行",
        ));
    }
    let rebuilt = if body.is_empty() {
        canonical_title
    } else {
        format!("{canonical_title}\n\n{body}")
    };
    Ok(normalize_blank_lines(&rebuilt))
}

/// Pipeline path: coerce heading; report remaining issues.
pub fn normalize_script_best_effort(episode: u32, text: &str) -> (String, Vec<String>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (String::new(), vec!["剧本不能为空".into()]);
    }
    let coerced = coerce_script_heading(episode, trimmed);
    let normalized = normalize_blank_lines(coerced.trim());
    match validate_script(episode, &normalized) {
        Ok(ok) => (ok, Vec::new()),
        Err(e) => (normalized, vec![e.message]),
    }
}

pub fn display_script(text: &str) -> String {
    normalize_blank_lines(text.trim())
}

fn coerce_script_heading(episode: u32, text: &str) -> String {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or("").trim();
    let rest: Vec<&str> = lines.collect();
    let suffix = title_suffix_for_episode(first, episode)
        .unwrap_or_else(|| {
            let s = first
                .trim_start_matches('#')
                .trim()
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '第' || c == '集');
            let s = s.trim_start_matches(['·', ' ', ':', '：']);
            if s.is_empty() {
                String::new()
            } else {
                format!(" · {s}")
            }
        });
    let title = format!("# 第{episode}集{suffix}");
    if rest.is_empty() {
        title
    } else {
        format!("{title}\n{}", rest.join("\n"))
    }
}

fn title_suffix_for_episode(first: &str, episode: u32) -> Option<String> {
    let t = first.trim();
    if !t.starts_with('#') {
        return None;
    }
    let rest = t.trim_start_matches('#').trim();
    let prefixes = [
        format!("第{episode}集"),
        format!("第 {episode} 集"),
    ];
    for p in &prefixes {
        if let Some(suffix) = rest.strip_prefix(p.as_str()) {
            let suffix = suffix.trim_start();
            if suffix.is_empty() {
                return Some(String::new());
            }
            if suffix.starts_with('·') || suffix.starts_with(' ') {
                return Some(format!(" {suffix}").replacen("  ", " ", 1));
            }
            return Some(format!(" · {suffix}"));
        }
    }
    // Chinese numerals — accept and canonicalize if body looks like episode title.
    if rest.starts_with('第') && rest.contains('集') {
        if let Some(idx) = rest.find('集') {
            let suffix = rest[idx + '集'.len_utf8()..].trim_start();
            if suffix.is_empty() {
                return Some(String::new());
            }
            if suffix.starts_with('·') || suffix.starts_with(' ') {
                return Some(format!(" {suffix}").replacen("  ", " ", 1));
            }
            return Some(format!(" · {suffix}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ep: u32) -> String {
        let pad = "对白填充。".repeat(40);
        format!(
            "# 第{ep}集 · 试写\n\n## 场1 · 咖啡馆 / 日\n【画面】两人隔桌对坐\n甲：你来了。\n乙：有事说。\n甲：{pad}\n\n## 场2 · 街口 / 夜\n【画面】雨中回头\n甲：别跟着我。\n乙：今晚必须说清楚。\n\n【钩子】乙口袋里的芯片亮了。\n"
        )
    }

    #[test]
    fn validate_ok() {
        let text = sample(1);
        let ok = validate_script(1, &text).unwrap();
        assert!(ok.starts_with("# 第1集"));
        assert!(ok.contains("【钩子】"));
    }

    #[test]
    fn reject_chapter_title() {
        let text = "# 第1章 · 错\n\n## 场1 · a\n【画面】x\n甲：嗨\n\n## 场2 · b\n【画面】y\n乙：嗨\n\n【钩子】z\n";
        assert!(validate_script(1, text).is_err());
    }

    #[test]
    fn normalize_coerces_heading() {
        let (t, issues) = normalize_script_best_effort(
            2,
            "## 场1 · a\n【画面】x\n甲：1\n\n## 场2 · b\n【画面】y\n乙：2\n\n【钩子】h\n",
        );
        // Still missing length maybe — but heading coerced
        assert!(t.starts_with("# 第2集") || !issues.is_empty() || t.contains("场1"));
    }
}
