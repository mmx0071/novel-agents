//! Hard gate: refuse skipping ahead of `next_chapter` / leaving gaps.

use crate::project::{load_project_state, read_chapter_draft};
use std::path::Path;

/// Minimum meaningful draft size shared by order-gap and revise-target checks.
const MIN_DRAFT_CHARS: usize = 20;

#[derive(Debug, Clone)]
pub struct ChapterOrderBlock {
    pub reason: &'static str,
    pub message: String,
    pub next_chapter: u32,
    pub requested: u32,
}

fn draft_ready(project_dir: &Path, chapter: u32) -> bool {
    read_chapter_draft(project_dir, chapter)
        .map(|t| t.trim().chars().count() >= MIN_DRAFT_CHARS)
        .unwrap_or(false)
}

/// Block when writing `chapter` would leave unpublished gaps.
///
/// `next_chapter` only advances on **publish**. An unpublished draft at
/// `next_chapter` is still "done enough" to allow writing the following chapter
/// (draft_exists → `de_next` / 写下一章). Pure skips with no prior draft remain blocked.
pub fn check_chapter_order(project_dir: &Path, chapter: u32) -> Option<ChapterOrderBlock> {
    let state = match load_project_state(project_dir) {
        Ok(s) => s,
        Err(_) => {
            return Some(ChapterOrderBlock {
                reason: "chapter_state_error",
                message: "无法读取项目状态，拒绝写章。请检查项目目录后重试。".into(),
                next_chapter: 1,
                requested: chapter.max(1),
            });
        }
    };
    let next = state.next_chapter.max(1);
    let requested = chapter.max(1);
    let published = state.published_count;
    // Gap: some chapter before requested has neither draft nor published credit.
    // (Do not hard-block on requested > next — that wrongly forbids「写下一章」
    // while the current chapter sits as an unpublished draft.)
    for n in 1..requested {
        if n <= published {
            continue;
        }
        if !draft_ready(project_dir, n) {
            let reason = if requested > next {
                "chapter_skip"
            } else {
                "chapter_gap"
            };
            let message = if requested > next {
                format!(
                    "不能跳写第{requested}章：第{n}章尚无正文（next_chapter={next}，已发布 {published} 章）。\n\
                     请先 continue_writing 创作第{n}章；若要改已有正文请用 revise_chapter。"
                )
            } else {
                format!(
                    "第{n}章尚未创作，不能写第{requested}章。\n\
                     请先 continue_writing 创作第{n}章（当前 next_chapter={next}）。"
                )
            };
            return Some(ChapterOrderBlock {
                reason,
                message,
                next_chapter: n.min(next),
                requested,
            });
        }
    }
    None
}

/// Revise is allowed only for chapters that already exist on disk or are published.
pub fn check_revise_target(project_dir: &Path, chapter: u32) -> Option<String> {
    let Ok(state) = load_project_state(project_dir) else {
        return Some("无法读取项目状态，拒绝修订。".into());
    };
    let ch = chapter.max(1);
    if ch <= state.published_count {
        return None;
    }
    if draft_ready(project_dir, ch) {
        return None;
    }
    Some(format!(
        "第{ch}章尚无正文，不能 revise_chapter。请先 continue_writing 创作第{}章。",
        state.next_chapter.max(1)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{init_project, load_project_state, save_project_state, write_chapter_draft};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "novelx-chgate-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn blocks_skip_ahead_of_next() {
        let root = tmp("skip");
        let dir = init_project(&root, "book", "未定", 100).unwrap();
        let block = check_chapter_order(&dir, 2).expect("must block");
        assert_eq!(block.reason, "chapter_skip");
        assert_eq!(block.next_chapter, 1);
        assert!(check_chapter_order(&dir, 1).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn allows_write_next_when_unpublished_draft_ready() {
        let root = tmp("draft-ahead");
        let dir = init_project(&root, "book", "未定", 100).unwrap();
        // next_chapter stays 1 until publish; draft at ch1 must unlock writing ch2.
        write_chapter_draft(
            &dir,
            1,
            "第一章正文足够长用于测试，凑满二十字门槛。",
        )
        .unwrap();
        assert!(
            check_chapter_order(&dir, 2).is_none(),
            "unpublished draft at next_chapter should allow de_next / 写下一章"
        );
        // Still refuse jumping over a missing middle chapter.
        let block = check_chapter_order(&dir, 3).expect("gap at 2");
        assert_eq!(block.reason, "chapter_skip");
        assert!(block.message.contains("第2章"), "{}", block.message);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn blocks_gap_even_if_next_advanced() {
        let root = tmp("gap");
        let dir = init_project(&root, "book", "未定", 100).unwrap();
        let mut state = load_project_state(&dir).unwrap();
        // Corrupt/advanced pointer without drafts — still refuse ch3 with gap at 1.
        state.next_chapter = 3;
        state.published_count = 0;
        save_project_state(&dir, &state).unwrap();
        let block = check_chapter_order(&dir, 3).expect("gap");
        assert_eq!(block.reason, "chapter_gap");
        assert_eq!(block.next_chapter, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn revise_requires_existing_draft_or_published() {
        let root = tmp("rev");
        let dir = init_project(&root, "book", "未定", 100).unwrap();
        assert!(check_revise_target(&dir, 1).is_some());
        write_chapter_draft(&dir, 1, "第一章正文足够长用于测试，凑满二十字门槛。").unwrap();
        assert!(check_revise_target(&dir, 1).is_none());
        // Below MIN_DRAFT_CHARS — still refuse revise.
        write_chapter_draft(&dir, 3, "太短了").unwrap();
        assert!(check_revise_target(&dir, 3).is_some());
        let mut state = load_project_state(&dir).unwrap();
        state.published_count = 2;
        state.next_chapter = 3;
        save_project_state(&dir, &state).unwrap();
        assert!(check_revise_target(&dir, 2).is_none());
        assert!(check_revise_target(&dir, 5).is_some());
        let _ = fs::remove_dir_all(&root);
    }
}
