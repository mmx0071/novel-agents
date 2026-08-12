//! Episode script: `episodes/NNN/script.md` (project_mode=short_drama).

use super::error::SchemaError;
use super::md::normalize_blank_lines;
use novelx_harness::ScriptShapeConfig;

/// Minimum script body chars after the title line (shape gate; see script.yaml for publish band).
pub const MIN_SCRIPT_BODY_CHARS: usize = 200;

/// Strict schema check for short-drama scripts.
pub fn validate_script(
    episode: u32,
    text: &str,
    shape: &ScriptShapeConfig,
) -> Result<String, SchemaError> {
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
    if let Some(bad) = title_hits_reject(first, shape) {
        return Err(SchemaError::new(
            "script",
            "title",
            format!("标题含禁止内容「{bad}」，请去掉代码围栏或 markdown 字样后重写标题"),
        ));
    }
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
    let scene_heads: Vec<(usize, &str)> = body
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            t.starts_with("## 场") || t.starts_with("##场")
        })
        .collect();
    let scene_count = scene_heads.len();
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

    let picture_total = count_picture_markers(&body);
    let min_total = shape.picture_min_total as usize;
    if picture_total < min_total {
        return Err(SchemaError::new(
            "script",
            "picture",
            format!("全文至少需要 {min_total} 条「【画面】」，当前 {picture_total}"),
        ));
    }
    let min_per = shape.picture_min_per_scene as usize;
    if min_per > 0 {
        let line_vec: Vec<&str> = body.lines().collect();
        for (idx, &(start, _)) in scene_heads.iter().enumerate() {
            let end = scene_heads
                .get(idx + 1)
                .map(|(i, _)| *i)
                .unwrap_or(line_vec.len());
            let section = line_vec[start..end].join("\n");
            let n = count_picture_markers(&section);
            if n < min_per {
                let scene_no = idx + 1;
                return Err(SchemaError::new(
                    "script",
                    "picture",
                    format!("第 {scene_no} 个「## 场」节至少需要 {min_per} 条「【画面】」，当前 {n}"),
                ));
            }
        }
    }

    let rebuilt = if body.is_empty() {
        canonical_title
    } else {
        format!("{canonical_title}\n\n{body}")
    };
    Ok(normalize_blank_lines(&rebuilt))
}

/// Pipeline path: coerce heading / strip title noise; report remaining issues.
pub fn normalize_script_best_effort(
    episode: u32,
    text: &str,
    shape: &ScriptShapeConfig,
) -> (String, Vec<String>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (String::new(), vec!["剧本不能为空".into()]);
    }
    let coerced = coerce_script_heading(episode, trimmed, shape);
    let normalized = normalize_blank_lines(coerced.trim());
    match validate_script(episode, &normalized, shape) {
        Ok(ok) => (ok, Vec::new()),
        Err(e) => (normalized, vec![e.message]),
    }
}

fn count_picture_markers(text: &str) -> usize {
    text.matches("【画面】").count()
}

fn title_hits_reject(first: &str, shape: &ScriptShapeConfig) -> Option<String> {
    let lower = first.to_ascii_lowercase();
    for needle in &shape.reject_title_contains {
        if needle.is_empty() {
            continue;
        }
        let hit = if needle.bytes().all(|b| b.is_ascii()) {
            lower.contains(&needle.to_ascii_lowercase())
        } else {
            first.contains(needle.as_str())
        };
        if hit {
            return Some(needle.clone());
        }
    }
    None
}

/// Remove standalone ` · 章 · ` style noise tokens from title suffix.
fn strip_title_noise_suffix(suffix: &str, noise: &[String]) -> String {
    if noise.is_empty() || suffix.is_empty() {
        return suffix.to_string();
    }
    let mut parts: Vec<String> = suffix
        .split('·')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    parts.retain(|p| !noise.iter().any(|n| n == p));
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn coerce_script_heading(episode: u32, text: &str, shape: &ScriptShapeConfig) -> String {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or("").trim();
    let rest: Vec<&str> = lines.collect();
    let mut suffix = title_suffix_for_episode(first, episode).unwrap_or_else(|| {
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
    suffix = strip_title_noise_suffix(&suffix, &shape.strip_title_noise);
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

    fn shape() -> ScriptShapeConfig {
        ScriptShapeConfig::default()
    }

    fn sample(ep: u32) -> String {
        let pad = "对白填充。".repeat(40);
        format!(
            "# 第{ep}集 · 试写\n\n## 场1 · 咖啡馆 / 日\n【画面】两人隔桌对坐\n甲：你来了。\n乙：有事说。\n甲：{pad}\n\n## 场2 · 街口 / 夜\n【画面】雨中回头\n甲：别跟着我。\n乙：今晚必须说清楚。\n\n【钩子】乙口袋里的芯片亮了。\n"
        )
    }

    fn pad_body(extra: &str) -> String {
        format!("{extra}{}", "对白填充。".repeat(40))
    }

    #[test]
    fn validate_ok() {
        let text = sample(1);
        let ok = validate_script(1, &text, &shape()).unwrap();
        assert!(ok.starts_with("# 第1集"));
        assert!(ok.contains("【钩子】"));
        assert!(ok.contains("【画面】"));
    }

    #[test]
    fn reject_chapter_title() {
        let text = "# 第1章 · 错\n\n## 场1 · a\n【画面】x\n甲：嗨\n\n## 场2 · b\n【画面】y\n乙：嗨\n\n【钩子】z\n";
        assert!(validate_script(1, text, &shape()).is_err());
    }

    #[test]
    fn reject_prose_without_pictures() {
        let text = format!(
            "# 第1集 · 散文\n\n## 场1 · a\n{}\n\n## 场2 · b\n{}\n\n【钩子】悬念。\n",
            pad_body("甲走进房间，没有画面行。"),
            pad_body("乙站在门口。")
        );
        let err = validate_script(1, &text, &shape()).unwrap_err();
        assert!(err.message.contains("【画面】"), "{}", err.message);
    }

    #[test]
    fn reject_scene_missing_picture() {
        let text = format!(
            "# 第1集 · 缺场画面\n\n## 场1 · a\n【画面】可见\n{}\n\n## 场2 · b\n{}\n\n【钩子】悬念。\n",
            pad_body("甲：有画面。"),
            pad_body("乙：这场没有画面标记。")
        );
        let err = validate_script(1, &text, &shape()).unwrap_err();
        assert!(
            err.message.contains("第 2 个") || err.message.contains("【画面】"),
            "{}",
            err.message
        );
    }

    #[test]
    fn reject_title_with_markdown_fence() {
        let text = format!(
            "# 第1集 · ```markdown\n\n## 场1 · a\n【画面】x\n{}\n\n## 场2 · b\n【画面】y\n{}\n\n【钩子】z\n",
            pad_body("甲：一"),
            pad_body("乙：二")
        );
        let err = validate_script(1, &text, &shape()).unwrap_err();
        assert!(err.message.contains("禁止") || err.message.contains("```"), "{}", err.message);
    }

    #[test]
    fn reject_title_with_markdown_word() {
        let text = format!(
            "# 第1集 · markdown 噪音\n\n## 场1 · a\n【画面】x\n{}\n\n## 场2 · b\n【画面】y\n{}\n\n【钩子】z\n",
            pad_body("甲：一"),
            pad_body("乙：二")
        );
        let err = validate_script(1, &text, &shape()).unwrap_err();
        assert!(err.message.contains("markdown") || err.message.contains("禁止"), "{}", err.message);
    }

    #[test]
    fn normalize_strips_chapter_noise_then_passes() {
        let text = format!(
            "# 第1集 · 章 · 干净标题\n\n## 场1 · a\n【画面】x\n{}\n\n## 场2 · b\n【画面】y\n{}\n\n【钩子】z\n",
            pad_body("甲：一"),
            pad_body("乙：二")
        );
        let (t, issues) = normalize_script_best_effort(1, &text, &shape());
        assert!(issues.is_empty(), "{issues:?}");
        assert!(t.starts_with("# 第1集 · 干净标题"), "{t}");
        assert!(!t.lines().next().unwrap_or("").contains('章'));
    }

    #[test]
    fn normalize_coerces_heading() {
        let (t, issues) = normalize_script_best_effort(
            2,
            &format!(
                "## 场1 · a\n【画面】x\n{}\n\n## 场2 · b\n【画面】y\n{}\n\n【钩子】h\n",
                pad_body("甲：1"),
                pad_body("乙：2")
            ),
            &shape(),
        );
        assert!(t.starts_with("# 第2集"), "{t} issues={issues:?}");
        assert!(issues.is_empty() || t.contains("场1"));
    }
}
