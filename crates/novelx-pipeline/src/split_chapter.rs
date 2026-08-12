//! Split an overlong unpublished chapter draft into two consecutive chapters.

use crate::project::{
    chapter_dir, load_project_state, read_chapter_draft, write_chapter_draft,
};
use crate::schemas::{draft_body_chars, normalize_draft_best_effort, MIN_DRAFT_BODY_CHARS};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SplitChapterResult {
    pub chapter_a: u32,
    pub chapter_b: u32,
    pub chars_a: usize,
    pub chars_b: usize,
    pub title_a: String,
    pub title_b: String,
}

/// Split draft at `chapter` into `chapter` + `chapter+1` at a paragraph boundary.
///
/// Constraints (unattended-safe):
/// - Source must be an unpublished draft at/near `next_chapter` tip (or explicit force).
/// - `chapter+1` must not already have a substantial draft.
/// - Both halves must meet [`MIN_DRAFT_BODY_CHARS`].
pub fn split_chapter_draft(
    project_dir: &Path,
    chapter: u32,
    cut_ratio: f32,
    title_b_override: Option<&str>,
    force: bool,
) -> Result<SplitChapterResult> {
    if chapter == 0 {
        bail!("chapter 须 ≥ 1");
    }
    let state = load_project_state(project_dir)?;
    let next = state.next_chapter.max(1);
    if !force && chapter > next {
        bail!("不能拆第{chapter}章：当前 next_chapter={next}（勿拆未写到的章）");
    }
    if !force && chapter < next && chapter <= state.published_count {
        bail!("第{chapter}章已发布，禁止拆分已发布章（force=true 可强制，慎用）");
    }

    let draft = read_chapter_draft(project_dir, chapter)
        .filter(|t| !t.trim().is_empty())
        .with_context(|| format!("第{chapter}章无正文可拆"))?;
    let body_n = draft_body_chars(&draft);
    if body_n < MIN_DRAFT_BODY_CHARS.saturating_mul(2) {
        bail!(
            "第{chapter}章正文过短（{body_n} 字），不足以拆成两章（合计至少 {} 字）",
            MIN_DRAFT_BODY_CHARS * 2
        );
    }

    let next_ch = chapter + 1;
    if let Some(existing) = read_chapter_draft(project_dir, next_ch) {
        let n = draft_body_chars(&existing);
        if n >= 400 && !force {
            bail!(
                "第{next_ch}章已有正文（约 {n} 字），拒绝覆盖。请先处理该章或 force=true"
            );
        }
    }

    let ratio = cut_ratio.clamp(0.35, 0.65);
    let (title_line, body) = split_title_body(&draft);
    let (part_a, part_b, _cut_at) = cut_body_at_paragraph(&body, ratio)?;
    let chars_a = part_a.chars().count();
    let chars_b = part_b.chars().count();
    if chars_a < MIN_DRAFT_BODY_CHARS || chars_b < MIN_DRAFT_BODY_CHARS {
        bail!(
            "拆分后半段过短（前 {chars_a} / 后 {chars_b}，各须 ≥ {MIN_DRAFT_BODY_CHARS}）。请调整 cut_ratio"
        );
    }

    let suffix_a = title_suffix_from_line(&title_line, chapter);
    let title_a_name = if suffix_a.is_empty() {
        "上".into()
    } else {
        suffix_a.trim().to_string()
    };
    let title_b_name = title_b_override
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if title_a_name.ends_with('续') {
                format!("{title_a_name}·下")
            } else {
                format!("{title_a_name}·续")
            }
        });

    let draft_a = format!("# 第{chapter}章 {title_a_name}\n\n{part_a}\n");
    let draft_b = format!("# 第{next_ch}章 {title_b_name}\n\n{part_b}\n");
    let (draft_a, issues_a) = normalize_draft_best_effort(chapter, &draft_a);
    let (draft_b, issues_b) = normalize_draft_best_effort(next_ch, &draft_b);
    if draft_a.trim().is_empty() || draft_b.trim().is_empty() {
        bail!("拆分结果为空");
    }
    if !issues_a.is_empty() {
        tracing::warn!(chapter, ?issues_a, "split part A shape issues");
    }
    if !issues_b.is_empty() {
        tracing::warn!(chapter = next_ch, ?issues_b, "split part B shape issues");
    }

    clear_chapter_memory_artifacts(project_dir, chapter);
    clear_chapter_memory_artifacts(project_dir, next_ch);
    write_chapter_draft(project_dir, chapter, &draft_a)?;
    write_chapter_draft(project_dir, next_ch, &draft_b)?;
    // Keep next_chapter at the first half so batch/audit resume publishes A then B.
    if state.next_chapter > chapter {
        // tip already past — leave state; caller may have force-split a middle draft.
    }

    tracing::info!(
        chapter_a = chapter,
        chapter_b = next_ch,
        chars_a,
        chars_b,
        "split overlong chapter draft"
    );

    Ok(SplitChapterResult {
        chapter_a: chapter,
        chapter_b: next_ch,
        chars_a: draft_body_chars(&draft_a),
        chars_b: draft_body_chars(&draft_b),
        title_a: title_a_name,
        title_b: title_b_name,
    })
}

fn clear_chapter_memory_artifacts(project_dir: &Path, chapter: u32) {
    let dir = chapter_dir(project_dir, chapter);
    for name in [
        "audit.json",
        "summary.json",
        "summary.json.draft_fp",
        "foreshadow.json",
        "foreshadow.json.draft_fp",
        "plot_accept.json",
    ] {
        let _ = fs::remove_file(dir.join(name));
    }
}

fn split_title_body(draft: &str) -> (String, String) {
    let trimmed = draft.trim();
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or("").to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    if first.trim_start().starts_with("# ") {
        (first, body)
    } else {
        (String::new(), trimmed.to_string())
    }
}

fn title_suffix_from_line(title_line: &str, chapter: u32) -> String {
    let t = title_line.trim();
    let prefixes = [
        format!("# 第{chapter}章"),
        format!("#第{chapter}章"),
    ];
    for p in &prefixes {
        if let Some(rest) = t.strip_prefix(p) {
            return rest.trim().to_string();
        }
    }
    if let Some(rest) = t.strip_prefix('#') {
        return rest.trim().to_string();
    }
    String::new()
}

/// Cut near `ratio` of character length, snapping to a paragraph break.
fn cut_body_at_paragraph(body: &str, ratio: f32) -> Result<(String, String, usize)> {
    let chars: Vec<char> = body.chars().collect();
    let n = chars.len();
    if n < MIN_DRAFT_BODY_CHARS * 2 {
        bail!("正文过短");
    }
    let target = ((n as f32) * ratio).round() as usize;
    let target = target
        .clamp(MIN_DRAFT_BODY_CHARS, n.saturating_sub(MIN_DRAFT_BODY_CHARS));

    // Search outward for `\n\n`
    let mut best = None::<usize>;
    let mut dist = usize::MAX;
    let mut i = 0usize;
    while i + 1 < n {
        if chars[i] == '\n' && chars[i + 1] == '\n' {
            let cut = i; // end of first part exclusive of the blank line pair start
            if cut >= MIN_DRAFT_BODY_CHARS && n - cut >= MIN_DRAFT_BODY_CHARS {
                let d = cut.abs_diff(target);
                if d < dist {
                    dist = d;
                    best = Some(cut);
                }
            }
        }
        i += 1;
    }
    let cut = best.unwrap_or(target);
    let left: String = chars[..cut].iter().collect::<String>().trim().to_string();
    let right: String = chars[cut..].iter().collect::<String>().trim().to_string();
    if left.is_empty() || right.is_empty() {
        bail!("未能在段落边界切开");
    }
    Ok((left, right, cut))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{init_project, save_project_state};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "novelx-split-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::create_dir_all(&p);
        p
    }

    fn filler(n: usize) -> String {
        "甲乙丙丁戊己庚辛。".chars().cycle().take(n).collect()
    }

    #[test]
    fn splits_tip_draft_into_two() {
        let root = tmp();
        let proj = init_project(&root, "demo", "测试", 20).unwrap();
        let mut state = load_project_state(&proj).unwrap();
        state.next_chapter = 3;
        state.published_count = 2;
        save_project_state(&proj, &state).unwrap();
        let body = format!("{}\n\n转场。\n\n{}", filler(1200), filler(1200));
        write_chapter_draft(&proj, 3, &format!("# 第3章 长夜\n\n{body}\n")).unwrap();
        let r = split_chapter_draft(&proj, 3, 0.5, None, false).unwrap();
        assert_eq!(r.chapter_a, 3);
        assert_eq!(r.chapter_b, 4);
        assert!(r.chars_a >= MIN_DRAFT_BODY_CHARS);
        assert!(r.chars_b >= MIN_DRAFT_BODY_CHARS);
        let a = read_chapter_draft(&proj, 3).unwrap();
        let b = read_chapter_draft(&proj, 4).unwrap();
        assert!(a.contains("# 第3章"));
        assert!(b.contains("# 第4章"));
        assert!(b.contains("续") || b.contains("下"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_when_next_has_draft() {
        let root = tmp();
        let proj = init_project(&root, "demo", "测试", 20).unwrap();
        let mut state = load_project_state(&proj).unwrap();
        state.next_chapter = 1;
        save_project_state(&proj, &state).unwrap();
        let body = format!("{}\n\n转。\n\n{}", filler(1200), filler(1200));
        write_chapter_draft(&proj, 1, &format!("# 第1章 甲\n\n{body}\n")).unwrap();
        write_chapter_draft(&proj, 2, &format!("# 第2章 乙\n\n{}\n", filler(900))).unwrap();
        assert!(split_chapter_draft(&proj, 1, 0.5, None, false).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
