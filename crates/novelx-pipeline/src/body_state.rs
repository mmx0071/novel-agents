//! Continuity board for injury sites & ability loci — injected into writer CanonContext.
//! Genre-neutral: only structural side/site / host cues, never story-specific names.

use crate::cards::truncate_chars;
use crate::memory::load_memory;
use crate::project::chapter_dir;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// Build a short locked board of injury / ability-locus facts for chapter N
/// (sourced from prior digests + previous chapter summary).
pub fn format_body_state_board(project_dir: &Path, chapter: u32) -> String {
    render_board(&collect_body_state_lines(project_dir, chapter))
}

/// Per-character board for Web / entity cards.
///
/// - Fact mentions any of `self_keys` → include.
/// - Fact mentions some other name in `all_keys` but not self → exclude.
/// - Fact mentions nobody in `all_keys` → include only when `is_default_owner`
///   (always-include / protagonist card).
pub fn format_body_state_board_for_character(
    project_dir: &Path,
    chapter: u32,
    self_keys: &[String],
    all_keys: &[String],
    is_default_owner: bool,
) -> String {
    let lines = collect_body_state_lines(project_dir, chapter);
    let self_keys = normalize_keys(self_keys);
    let all_keys = normalize_keys(all_keys);
    if self_keys.is_empty() && !is_default_owner {
        return String::new();
    }

    let mut owned = Vec::new();
    for (ch, fact) in lines {
        let mentioned: Vec<&str> = all_keys
            .iter()
            .map(|s| s.as_str())
            .filter(|k| fact.contains(k))
            .collect();
        let mine = mentioned.iter().any(|k| self_keys.iter().any(|s| s == k));
        let include = if mentioned.is_empty() {
            is_default_owner
        } else {
            mine
        };
        if include {
            owned.push((ch, fact));
        }
    }
    render_board(&owned)
}

fn normalize_keys(keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = keys
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| s.chars().count() >= 1)
        .collect();
    out.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
    out.dedup();
    out
}

fn collect_body_state_lines(project_dir: &Path, chapter: u32) -> Vec<(u32, String)> {
    let mut lines: Vec<(u32, String)> = Vec::new();
    let mut seen = HashSet::new();

    let mem = load_memory(project_dir);
    let prior_chs: Vec<u32> = {
        let mut cs: Vec<u32> = mem
            .recent_digests
            .iter()
            .map(|d| d.chapter)
            .filter(|c| *c < chapter || chapter == 0)
            .collect();
        if chapter > 1 {
            cs.push(chapter - 1);
        }
        cs.sort_unstable();
        cs.dedup();
        cs.into_iter().rev().take(4).collect()
    };

    for ch in prior_chs.iter().copied().rev() {
        if let Some(d) = mem.recent_digests.iter().find(|d| d.chapter == ch) {
            for fact in &d.key_facts {
                push_fact(&mut lines, &mut seen, ch, fact);
            }
        }
        if let Some(v) = load_chapter_summary_json(project_dir, ch) {
            collect_from_summary_value(&mut lines, &mut seen, ch, &v);
        }
    }

    lines.sort_by(|a, b| b.0.cmp(&a.0));
    lines
}

fn render_board(lines: &[(u32, String)]) -> String {
    let mut injuries = Vec::new();
    let mut abilities = Vec::new();
    for (ch, fact) in lines {
        let tagged = format!("[第{ch}章] {fact}");
        if is_injury_fact(fact) {
            if injuries.len() < 8 {
                injuries.push(tagged);
            }
        } else if is_ability_locus_fact(fact) || is_control_side_fact(fact) {
            if abilities.len() < 8 {
                abilities.push(tagged);
            }
        }
    }

    if injuries.is_empty() && abilities.is_empty() {
        return String::new();
    }

    let mut out = Vec::new();
    out.push(
        "开写前锁定：本章描写伤势侧别/部位、能力寄宿/附着/载体与肢体控制时必须与下表一致；\
若要改变，须写出可见转移过程（不得默默挪位）。"
            .into(),
    );
    if !injuries.is_empty() {
        out.push(String::new());
        out.push("伤势：".into());
        for l in &injuries {
            out.push(format!("- {l}"));
        }
    }
    if !abilities.is_empty() {
        out.push(String::new());
        out.push("能力位置/载体/控制：".into());
        for l in &abilities {
            out.push(format!("- {l}"));
        }
    }
    out.join("\n")
}

fn load_chapter_summary_json(project_dir: &Path, chapter: u32) -> Option<Value> {
    let path = chapter_dir(project_dir, chapter).join("summary.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn collect_from_summary_value(
    lines: &mut Vec<(u32, String)>,
    seen: &mut HashSet<String>,
    chapter: u32,
    v: &Value,
) {
    if let Some(bs) = v.get("body_state") {
        if let Some(arr) = bs.get("injuries").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    push_fact(lines, seen, chapter, &format!("伤势：{s}"));
                }
            }
        }
        if let Some(arr) = bs.get("ability_loci").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    push_fact(lines, seen, chapter, &format!("能力位置：{s}"));
                }
            }
        }
    }
    if let Some(arr) = v.get("new_facts").and_then(|x| x.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                push_fact(lines, seen, chapter, s);
            }
        }
    }
}

fn push_fact(lines: &mut Vec<(u32, String)>, seen: &mut HashSet<String>, chapter: u32, raw: &str) {
    let fact = truncate_chars(raw.trim(), 160);
    if fact.is_empty() || !is_body_state_fact(&fact) {
        return;
    }
    let key = fact.chars().take(48).collect::<String>();
    if !seen.insert(key) {
        return;
    }
    lines.push((chapter, fact));
}

fn is_body_state_fact(s: &str) -> bool {
    is_injury_fact(s) || is_ability_locus_fact(s) || is_control_side_fact(s)
}

fn has_side_or_site(s: &str) -> bool {
    const SITES: &[&str] = &[
        "左", "右", "肩", "臂", "手", "掌", "腕", "腿", "膝", "踝", "腹", "胸", "肋", "背", "颈",
        "头", "额", "眼", "指",
    ];
    SITES.iter().any(|k| s.contains(k))
}

fn is_injury_fact(s: &str) -> bool {
    if s.starts_with("伤势：") || s.contains("伤势：") {
        return has_side_or_site(s) || s.contains("伤");
    }
    let injury_kw = s.contains("伤")
        || s.contains("伤口")
        || s.contains("骨折")
        || s.contains("淤血")
        || s.contains("血痕")
        || s.contains("撕裂")
        || s.contains("挫伤")
        || s.contains("扭伤")
        || s.contains("灼伤");
    injury_kw && has_side_or_site(s)
}

fn is_ability_locus_fact(s: &str) -> bool {
    if s.starts_with("能力位置：") || s.contains("能力位置：") {
        return true;
    }
    let mark_on_site = (s.contains("掌心") || s.contains("手腕") || s.contains("腕内侧") || s.contains("手背"))
        && (s.contains("符号") || s.contains("印记") || s.contains("纹") || s.contains("图案"));
    let locus_kw = s.contains("寄宿")
        || s.contains("附着")
        || s.contains("载体")
        || s.contains("印记")
        || s.contains("操控权")
        || s.contains("控制权")
        || s.contains("所在位置")
        || s.contains("附着点")
        || mark_on_site
        || (s.contains("能力") && (s.contains("位于") || s.contains("在其") || has_side_or_site(s)))
        || (s.contains("权能") && has_side_or_site(s))
        || (s.contains("异能") && has_side_or_site(s));
    locus_kw
}

fn is_control_side_fact(s: &str) -> bool {
    let control = s.contains("操控")
        || s.contains("控制")
        || s.contains("夺取")
        || s.contains("接管")
        || s.contains("不归")
        || s.contains("自主");
    control && has_side_or_site(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{save_memory, ChapterDigest, ProjectMemory};
    use crate::project::init_project;
    use std::path::PathBuf;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "novelx-body-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn board_pulls_injury_and_ability_from_summary() {
        let projects = tmp_root("sum");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 1);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"对峙。",
              "ending_hook":"门开了。",
              "new_facts":[
                "主角左肩贯穿伤，暂不能抬弓",
                "异能印记寄宿在右手腕内侧"
              ],
              "body_state":{
                "injuries":["左肩贯穿伤"],
                "ability_loci":["异能印记在右手腕内侧"]
              }
            }"#,
        )
        .unwrap();
        let board = format_body_state_board(&root, 2);
        assert!(board.contains("伤势"), "{board}");
        assert!(board.contains("左肩"), "{board}");
        assert!(board.contains("能力位置") || board.contains("右手腕"), "{board}");
        assert!(board.contains("不得默默挪位") || board.contains("可见转移"), "{board}");
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn board_uses_digest_facts_when_no_summary() {
        let projects = tmp_root("dig");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let mut mem = ProjectMemory {
            version: 1,
            ..Default::default()
        };
        mem.recent_digests.push(ChapterDigest {
            chapter: 3,
            event_summary: "夜袭".into(),
            hook: "追兵将近".into(),
            key_facts: vec![
                "主角右膝旧伤复发，奔跑跛行".into(),
                "权能附着于左掌心铜环".into(),
            ],
            ..Default::default()
        });
        save_memory(&root, &mem).unwrap();
        let board = format_body_state_board(&root, 4);
        assert!(board.contains("右膝"), "{board}");
        assert!(board.contains("左掌") || board.contains("铜环"), "{board}");
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn board_keeps_limb_control_beside_ability_locus() {
        let projects = tmp_root("ctrl");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 5);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"对峙。",
              "ending_hook":"门开了。",
              "new_facts":[
                "异能印记寄宿在右手腕内侧",
                "主角左臂约七成操控权被夺取，暂不归他控制"
              ]
            }"#,
        )
        .unwrap();
        let board = format_body_state_board(&root, 6);
        assert!(board.contains("右手腕") || board.contains("印记"), "{board}");
        assert!(board.contains("左臂") && board.contains("控制"), "{board}");
        assert!(board.contains("能力位置/载体/控制"), "{board}");
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn per_character_board_filters_by_name() {
        let projects = tmp_root("per");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 1);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"对峙。",
              "ending_hook":"门开了。",
              "new_facts":[
                "张甲左肩贯穿伤，暂不能抬弓",
                "李乙右膝旧伤复发",
                "异能印记寄宿在张甲右手腕内侧",
                "左踝擦伤未点名"
              ],
              "body_state":{
                "injuries":["张甲左肩贯穿伤","李乙右膝旧伤","左踝擦伤"],
                "ability_loci":["张甲：异能印记在右手腕内侧"]
              }
            }"#,
        )
        .unwrap();
        let all = vec!["张甲".into(), "李乙".into()];
        let a = format_body_state_board_for_character(
            &root,
            2,
            &["张甲".into()],
            &all,
            true,
        );
        let b = format_body_state_board_for_character(
            &root,
            2,
            &["李乙".into()],
            &all,
            false,
        );
        assert!(a.contains("左肩"), "{a}");
        assert!(a.contains("左踝") || a.contains("擦伤"), "default owner gets unscoped: {a}");
        assert!(!a.contains("右膝"), "{a}");
        assert!(b.contains("右膝"), "{b}");
        assert!(!b.contains("左肩"), "{b}");
        assert!(!b.contains("右手腕"), "{b}");
        assert!(!b.contains("左踝"), "{b}");
        let _ = std::fs::remove_dir_all(&projects);
    }
}
