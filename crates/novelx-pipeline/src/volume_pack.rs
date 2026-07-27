//! Layered volume context packing for audit_volume / sync_volume.
//! Avoids flattening every chapter summary into one exploding prompt.

use crate::memory::{
    load_all_volume_rollups, load_foreshadow_archive, load_memory, select_dangling_age_boosted,
};
use crate::volume::{volume_chapter_span, VolumeBound};
use crate::volume_audit_gate::thick_volume_threshold_resolved;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::Path;

/// Soft cap for the full pack (chars). Hard truncate after assembly if exceeded.
pub const VOLUME_PACK_CHAR_CAP: usize = 25_000;
const DIGEST_PER_CHAPTER: usize = 100;
const FOCUS_SUMMARY_CHARS: usize = 600;
const MAX_FOCUS_CHAPTERS: usize = 12;

#[derive(Debug, Clone)]
pub struct LayeredVolumePack {
    pub text: String,
    /// One-line digests covering every chapter in span (layer B).
    pub digest_count: usize,
    /// Chapters that received fuller summaries (layer C).
    pub focus_chapters: Vec<u32>,
}

/// Build layered pack shared by volume audit and volume sync.
pub fn gather_volume_layered_pack(
    project_dir: &Path,
    volume: &VolumeBound,
    upto: u32,
    extra_focus: &[u32],
) -> Result<LayeredVolumePack> {
    let (from, to) = volume_chapter_span(volume, upto);
    let mut parts = Vec::new();

    // Layer A: volume meta + rollup
    let mut layer_a = format!(
        "# 卷范围\nvolume_index={} 章 {}–{}\ngoal: {}\nending_conditions:\n{}",
        volume.volume_index,
        from,
        to,
        volume.goal,
        if volume.ending_conditions.is_empty() {
            "- （无）".into()
        } else {
            volume
                .ending_conditions
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    );
    let mem = load_memory(project_dir);
    let all_rollups = load_all_volume_rollups(project_dir);
    if let Some(r) = all_rollups
        .iter()
        .find(|r| r.volume_index == volume.volume_index)
    {
        layer_a.push_str(&format!(
            "\n\n## 本卷 rollup\n{}",
            truncate_chars(&r.summary, 900)
        ));
    }
    parts.push(layer_a);

    // Layer B: one-line digest per chapter; sample when volume is thick.
    let span = if from <= to {
        to.saturating_sub(from).saturating_add(1)
    } else {
        0
    };
    let thick_threshold = thick_volume_threshold_resolved(project_dir);
    let sample = span > thick_threshold;
    let step = if sample {
        ((span as f64) / 36.0).ceil().max(2.0) as u32
    } else {
        1
    };
    let mut digests = Vec::new();
    let mut digest_count = 0usize;
    if from <= to {
        let mut seen = BTreeSet::new();
        let mut ch = from;
        while ch <= to {
            if seen.insert(ch) {
                let digest = chapter_digest_line(project_dir, ch);
                digests.push(format!("- 第{ch}章：{digest}"));
                digest_count += 1;
            }
            if ch == to {
                break;
            }
            let next = ch.saturating_add(step);
            ch = if next >= to { to } else { next };
        }
    }
    if digests.is_empty() {
        digests.push("（本卷尚无章节）".into());
    }
    let layer_b = if sample {
        format!(
            "# 各章一行摘要（厚卷抽样：共{span}章，步长{step}；建议尽快结卷以恢复全量）\n{}",
            digests.join("\n")
        )
    } else {
        format!("# 各章一行摘要\n{}", digests.join("\n"))
    };
    parts.push(layer_b);

    // Layer C: focus chapters full(er) summaries
    let focus = select_focus_chapters(project_dir, from, to, extra_focus);
    let mut focus_parts = Vec::new();
    for ch in &focus {
        let body = chapter_summary_excerpt(project_dir, *ch, FOCUS_SUMMARY_CHARS);
        focus_parts.push(format!("## 第{ch}章\n{body}"));
    }
    if focus_parts.is_empty() {
        focus_parts.push("（无重点章摘要）".into());
    }
    parts.push(format!("# 重点章摘要\n{}", focus_parts.join("\n\n")));

    // Layer D: arc outline + memory/foreshadow slice
    let arc_vol = volume.volume_index.max(1);
    let arc = crate::volume::read_arc_outline_text(project_dir, arc_vol)
        .or_else(|| std::fs::read_to_string(project_dir.join("artifacts/arc_outline.md")).ok())
        .unwrap_or_default();
    if arc.trim().chars().count() > 20 {
        parts.push(format!(
            "# 卷纲节选\n{}",
            truncate_chars(&arc, 1800)
        ));
    }

    // Oldest-first longform debt: hot + archived + cold archive.
    let mut debt_pool: Vec<_> = mem
        .open_threads
        .iter()
        .chain(mem.archived_threads.iter())
        .cloned()
        .collect();
    for t in load_foreshadow_archive(project_dir) {
        if debt_pool.iter().any(|a| a.id == t.id || a.text == t.text) {
            continue;
        }
        debt_pool.push(t);
    }
    let open_threads: Vec<String> = select_dangling_age_boosted(&debt_pool, 16)
        .into_iter()
        .map(|t| {
            format!(
                "- [{}] {}（埋于第{}章）",
                t.id,
                truncate_chars(&t.text, 80),
                t.planted_chapter
            )
        })
        .collect();
    let mem_slice = format!(
        "## 未收伏笔（旧线优先·含冷档）\n{}\n\n## 滚动记忆节选\n{}",
        if open_threads.is_empty() {
            "（无）".into()
        } else {
            open_threads.join("\n")
        },
        truncate_chars(&mem.rolling_summary, 1600)
    );
    parts.push(format!("# 记忆与伏笔\n{mem_slice}"));

    let mut text = parts.join("\n\n");
    if text.chars().count() > VOLUME_PACK_CHAR_CAP {
        text = truncate_chars(&text, VOLUME_PACK_CHAR_CAP);
        text.push_str("\n\n（已截断至卷打包字数上限）");
    }

    Ok(LayeredVolumePack {
        text,
        digest_count,
        focus_chapters: focus,
    })
}

fn select_focus_chapters(
    project_dir: &Path,
    from: u32,
    to: u32,
    extra: &[u32],
) -> Vec<u32> {
    if from > to {
        return Vec::new();
    }
    let mut set = BTreeSet::new();
    set.insert(from);
    set.insert(to);
    let mid = from + (to.saturating_sub(from) / 2);
    if mid >= from && mid <= to {
        set.insert(mid);
    }
    for ch in extra {
        if *ch >= from && *ch <= to {
            set.insert(*ch);
        }
    }
    for ch in from..=to {
        let dir = project_dir.join("chapters").join(format!("{ch:03}"));
        let audit_path = dir.join("audit.json");
        if let Ok(text) = std::fs::read_to_string(&audit_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if v.get("passed").and_then(|x| x.as_bool()) == Some(false) {
                    set.insert(ch);
                }
            }
        }
        if !dir.join("summary.json").exists() && dir.join("draft.md").exists() {
            set.insert(ch);
        }
    }
    let mut out: Vec<u32> = set.into_iter().collect();
    if out.len() > MAX_FOCUS_CHAPTERS {
        // Keep first, last, mid, then earliest failures / extras.
        let mut keep = BTreeSet::new();
        keep.insert(from);
        keep.insert(to);
        keep.insert(mid);
        for ch in out.iter().copied() {
            if keep.len() >= MAX_FOCUS_CHAPTERS {
                break;
            }
            keep.insert(ch);
        }
        out = keep.into_iter().collect();
    }
    out
}

fn chapter_digest_line(project_dir: &Path, ch: u32) -> String {
    let path = project_dir
        .join("chapters")
        .join(format!("{ch:03}"))
        .join("summary.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let event = v
                .get("event_summary")
                .or_else(|| v.get("summary"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let hook = v
                .get("ending_hook")
                .or_else(|| v.get("cliffhanger"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let line = if hook.is_empty() {
                event.to_string()
            } else if event.is_empty() {
                hook.to_string()
            } else {
                format!("{event}｜{hook}")
            };
            let t = truncate_chars(&line, DIGEST_PER_CHAPTER);
            if !t.trim().is_empty() {
                return t;
            }
        }
        return truncate_chars(text.trim(), DIGEST_PER_CHAPTER);
    }
    let draft = project_dir
        .join("chapters")
        .join(format!("{ch:03}"))
        .join("draft.md");
    if draft.exists() {
        return "（有正文但无 summary.json）".into();
    }
    "（无）".into()
}

fn chapter_summary_excerpt(project_dir: &Path, ch: u32, max_chars: usize) -> String {
    let path = project_dir
        .join("chapters")
        .join(format!("{ch:03}"))
        .join("summary.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        return truncate_chars(&text, max_chars);
    }
    if project_dir
        .join("chapters")
        .join(format!("{ch:03}"))
        .join("draft.md")
        .exists()
    {
        return "（有正文但无 summary.json）".into();
    }
    "（无摘要）".into()
}

fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::VolumeBound;
    use std::fs;

    #[test]
    fn layered_pack_covers_all_digests_and_caps() {
        let root = std::env::temp_dir().join(format!(
            "novelx_layered_pack_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        // 40 chapters → would explode under old 900-char flat pack.
        for ch in 1..=40 {
            let dir = root.join("chapters").join(format!("{ch:03}"));
            fs::create_dir_all(&dir).unwrap();
            let filler = "甲".repeat(200);
            fs::write(
                dir.join("summary.json"),
                format!(
                    r#"{{"event_summary":"第{ch}章事件{filler}","ending_hook":"钩子{ch}"}}"#
                ),
            )
            .unwrap();
        }
        fs::write(
            root.join("chapters/010/audit.json"),
            r#"{"passed":false}"#,
        )
        .unwrap();
        let vol = VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 40,
            name: "卷一".into(),
            ending_conditions: vec!["抵达落点".into()],
            goal: "推进".into(),
            completed: false,
        };
        let pack = gather_volume_layered_pack(&root, &vol, 40, &[10]).unwrap();
        assert_eq!(pack.digest_count, 40);
        assert!(pack.text.contains("各章一行摘要"));
        assert!(pack.text.contains("第10章"));
        assert!(pack.focus_chapters.contains(&10));
        assert!(pack.focus_chapters.contains(&1));
        assert!(pack.focus_chapters.contains(&40));
        assert!(pack.text.chars().count() <= VOLUME_PACK_CHAR_CAP + 40);
        let _ = fs::remove_dir_all(&root);
    }
}
