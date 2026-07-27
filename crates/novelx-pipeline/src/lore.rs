//! Lore librarian: query slices + assert facts into memory.

use crate::cards::{load_markdown_cards, truncate_chars, truncate_chars_tail};
use crate::memory::{load_memory, recall_archived_summaries, save_memory, AssertedFact};
use anyhow::Result;
use std::path::Path;
use uuid::Uuid;

/// Build a short lore slice for writer (query mode).
pub fn lore_query(project_dir: &Path, chapter: u32, outline: &str, draft: &str) -> String {
    let hay = format!("{outline}\n{draft}");
    let mem = load_memory(project_dir);
    let mut parts = Vec::new();

    // Prefer SQLite entity hits when index is warm; needle from roster / lore_queries.
    let mut index_hits: Vec<(String, String, String)> = Vec::new();
    let mut seen_names = std::collections::HashSet::new();
    for needle in lore_index_needles(outline) {
        if let Some(hits) = crate::lore_index::query_entities_from_index(project_dir, &needle, 6) {
            for h in hits {
                if seen_names.insert(h.1.clone()) {
                    index_hits.push(h);
                }
            }
        }
        if index_hits.len() >= 8 {
            break;
        }
    }
    if !index_hits.is_empty() {
        let lines: Vec<String> = index_hits
            .into_iter()
            .take(8)
            .map(|(kind, name, status)| format!("- [{kind}] {name}（{status}）"))
            .collect();
        parts.push(format!("## Lore·实体索引\n{}", lines.join("\n")));
    }

    if !mem.rolling_summary.is_empty() {
        parts.push(format!(
            "## Lore·滚动摘要\n{}",
            truncate_chars_tail(&mem.rolling_summary, 700)
        ));
    }

    let digests: Vec<_> = mem
        .recent_digests
        .iter()
        .filter(|d| d.chapter < chapter)
        .rev()
        .take(3)
        .collect();
    for d in digests.into_iter().rev() {
        parts.push(format!(
            "## Lore·第{}章摘要\n{}\n钩子：{}",
            d.chapter,
            truncate_chars(&d.event_summary, 280),
            truncate_chars(&d.hook, 80)
        ));
    }

    let exclude: Vec<u32> = mem.recent_digests.iter().map(|d| d.chapter).collect();
    for d in recall_archived_summaries(project_dir, &hay, &exclude, 1) {
        parts.push(format!(
            "## Lore·召回第{}章\n{}\n钩子：{}",
            d.chapter,
            truncate_chars(&d.event_summary, 220),
            truncate_chars(&d.hook, 60)
        ));
    }

    let open: Vec<_> = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open")
        .take(8)
        .map(|t| {
            if t.planted_chapter > 0 {
                format!("- {}（埋于第{}章）", t.text, t.planted_chapter)
            } else {
                format!("- {}", t.text)
            }
        })
        .collect();
    if !open.is_empty() {
        parts.push(format!("## Lore·未收线\n{}", open.join("\n")));
    }
    let dangling = crate::foreshadow::format_dangling_for_context(project_dir, 6);
    if !dangling.is_empty() && open.is_empty() {
        parts.push(format!("## Lore·未回收伏笔\n{dangling}"));
    }

    let expected = crate::expected_events::format_expected_for_lore(project_dir, chapter);
    if !expected.is_empty() {
        parts.push(expected);
    }

    // Entity cards: outline roster first, then name hits in outline/draft.
    let roster = crate::schemas::outline_entity_roster(outline);
    let mut cards = Vec::new();
    for group in ["characters", "items", "locations"] {
        cards.extend(load_markdown_cards(
            &project_dir.join("entities").join(group),
            group,
        ));
    }
    let mut hit_slugs: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in roster
        .characters
        .iter()
        .chain(roster.items.iter())
        .chain(roster.locations.iter())
    {
        if let Some(c) = cards
            .iter()
            .find(|c| c.match_keys().iter().any(|k| k == name))
        {
            if seen.insert(c.slug.clone()) {
                hit_slugs.push(c.slug.clone());
            }
        }
    }
    for c in &cards {
        if hit_slugs.len() >= 6 {
            break;
        }
        if c.match_keys()
            .iter()
            .any(|k| k.chars().count() >= 2 && hay.contains(k))
            && seen.insert(c.slug.clone())
        {
            hit_slugs.push(c.slug.clone());
        }
    }
    hit_slugs.truncate(6);
    if !hit_slugs.is_empty() {
        let mut block = String::from("## Lore·相关实体\n");
        for slug in &hit_slugs {
            let Some(c) = cards.iter().find(|c| &c.slug == slug) else {
                continue;
            };
            block.push_str(&format!(
                "### {}\n{}\n",
                c.name,
                truncate_chars(c.markdown.trim(), 280)
            ));
        }
        parts.push(block);
    }

    // Recently asserted facts (name hit or last 6).
    let mut facts: Vec<&AssertedFact> = mem
        .asserted_facts
        .iter()
        .filter(|f| {
            f.text.chars().count() >= 4
                && hay
                    .chars()
                    .collect::<String>()
                    .contains(&f.text.chars().take(6).collect::<String>())
        })
        .collect();
    if facts.is_empty() {
        facts = mem.asserted_facts.iter().rev().take(6).collect();
        facts.reverse();
    }
    if !facts.is_empty() {
        let lines: Vec<_> = facts
            .iter()
            .map(|f| format!("- [第{}章] {}", f.chapter, truncate_chars(&f.text, 100)))
            .collect();
        parts.push(format!("## Lore·已入库事实\n{}", lines.join("\n")));
    }

    if parts.is_empty() {
        return String::new();
    }
    format!("# LoreSlice（query）\n\n{}", parts.join("\n\n"))
}

/// Assert new_facts from summarizer JSON into memory (skip duplicates / soft conflict).
pub fn lore_assert_from_summary(
    project_dir: &Path,
    chapter: u32,
    summary_raw: &str,
) -> Result<(usize, Vec<String>)> {
    let v = serde_json::from_str::<serde_json::Value>(summary_raw).unwrap_or_else(|_| {
        if let Some(start) = summary_raw.find('{') {
            if let Some(end) = summary_raw.rfind('}') {
                return serde_json::from_str(&summary_raw[start..=end]).unwrap_or_default();
            }
        }
        serde_json::json!({})
    });
    let facts: Vec<String> = v
        .get("new_facts")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| clean_fact(s)))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut mem = load_memory(project_dir);
    let mut added = 0usize;
    let mut conflicts = Vec::new();

    for text in facts {
        if text.chars().count() < 4 {
            continue;
        }
        if mem.asserted_facts.iter().any(|f| f.text == text) {
            continue;
        }
        // Soft conflict: opposite keywords on same subject prefix.
        if let Some(prev) = mem.asserted_facts.iter().rev().find(|f| {
            let a: String = text.chars().take(8).collect();
            !a.is_empty() && f.text.contains(&a)
        }) {
            if looks_conflicting(&prev.text, &text) {
                conflicts.push(format!("与已有事实冲突，未覆盖：{} ↔ {}", prev.text, text));
                continue;
            }
        }
        let entity = text
            .split('：')
            .next()
            .map(str::trim)
            .filter(|s| (2..=12).contains(&s.chars().count()))
            .unwrap_or("")
            .to_string();
        mem.asserted_facts.push(AssertedFact {
            id: format!("fact_{}", &Uuid::new_v4().simple().to_string()[..10]),
            text: text.clone(),
            chapter,
            source: "summarizer".into(),
            entity,
            confidence: 0.9,
        });
        added += 1;
    }

    crate::memory::prune_asserted_facts_to_archive(project_dir, &mut mem)?;
    save_memory(project_dir, &mem)?;
    Ok((added, conflicts))
}

fn clean_fact(s: &str) -> String {
    s.trim()
        .trim_start_matches("[NEW_FACT]")
        .trim_start_matches("[NEW_FACT]:")
        .trim()
        .to_string()
}

fn looks_conflicting(a: &str, b: &str) -> bool {
    let pairs = [("死亡", "存活"), ("已死", "未死"), ("破碎", "完好"), ("失去", "持有")];
    for (x, y) in pairs {
        if (a.contains(x) && b.contains(y)) || (a.contains(y) && b.contains(x)) {
            return true;
        }
    }
    false
}

/// Needles for SQLite entity lookup: outline roster names + lore_queries keywords.
fn lore_index_needles(outline: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        let t = s.trim();
        if t.chars().count() >= 2 && !out.iter().any(|x: &String| x == t) {
            out.push(t.to_string());
        }
    };
    let roster = crate::schemas::outline_entity_roster(outline);
    for n in roster
        .characters
        .iter()
        .chain(roster.items.iter())
        .chain(roster.locations.iter())
    {
        push(n);
    }
    if let Ok(parsed) = crate::schemas::parse_chapter_outline_text(outline) {
        for q in &parsed.lore_queries {
            // Prefer short keyword from query (first 8 chars / first token-ish).
            let key: String = q.chars().take(8).collect();
            push(&key);
            push(q);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::apply_summary_json;
    use std::fs;

    #[test]
    fn query_and_assert_roundtrip() {
        let dir = std::env::temp_dir().join("novelx-lore-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lore")).unwrap();
        fs::create_dir_all(dir.join("entities/characters")).unwrap();
        fs::write(
            dir.join("entities/characters/林舟.md"),
            "---\nname: 林舟\n---\n# 林舟\n主角。\n",
        )
        .unwrap();
        let summary = r#"{"event_summary":"林舟进入矿井。","ending_hook":"深处有光。","new_facts":["林舟左臂受伤"]}"#;
        apply_summary_json(&dir, 1, summary).unwrap();
        let (n, _) = lore_assert_from_summary(&dir, 1, summary).unwrap();
        assert_eq!(n, 1);
        let slice = lore_query(&dir, 2, "林舟继续下探", "");
        assert!(slice.contains("LoreSlice") || slice.contains("林舟"));
        let _ = fs::remove_dir_all(&dir);
    }
}
