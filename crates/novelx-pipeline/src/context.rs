//! Chapter-scoped CanonContext: lore pack for long-horizon consistency.

use crate::cards::{load_markdown_cards, truncate_chars, truncate_chars_tail, MarkdownCard};
use crate::memory::{load_memory, recall_archived_summaries};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextProfile {
    /// Full pack for consistency / writer / chapter_planner (~6–8k).
    Full,
    /// Short pack for pacing (~2k): act goals + open threads only.
    Pacing,
}

#[derive(Debug, Clone)]
pub struct ChapterContextPack {
    pub markdown: String,
    pub hits: Vec<String>,
}

impl ChapterContextPack {
    pub fn empty() -> Self {
        Self {
            markdown: String::new(),
            hits: vec![],
        }
    }
}

/// Build a budgeted CanonContext for a chapter from on-disk project lore.
pub fn build_chapter_context(
    project_dir: &Path,
    chapter: u32,
    draft: &str,
    outline: &str,
    profile: ContextProfile,
) -> ChapterContextPack {
    let haystack = format!("{draft}\n{outline}");
    let mut hits = Vec::new();
    let mut sections: Vec<(String, String)> = Vec::new();

    // --- 卷幕目标 ---
    let act_block = select_act_goals(project_dir, chapter, &mut hits);
    if !act_block.is_empty() {
        sections.push(("卷幕目标".into(), truncate_chars(&act_block, 800)));
    }

    // --- 滚动记忆（近章优先；可按大纲关键词召回旧章摘要）---
    let memory_block = select_memory(project_dir, chapter, &haystack, profile, &mut hits);
    if !memory_block.is_empty() {
        let budget = if profile == ContextProfile::Pacing {
            1100
        } else {
            2000
        };
        // Prefer keeping the recent tail if over budget.
        sections.push((
            "滚动记忆与未收线".into(),
            truncate_chars_tail(&memory_block, budget),
        ));
    }

    if profile == ContextProfile::Pacing {
        // Pacing: only act + memory/threads.
        return assemble(sections, hits);
    }

    // --- 世界观（bible.md 与 world_architect.md 任一可用）---
    let (world_src, world) = read_world_doc(project_dir);
    if !world.trim().is_empty() {
        hits.push(world_src.into());
        sections.push(("世界观摘要".into(), truncate_chars(world.trim(), 1000)));
    }

    // --- 名词 ---
    let nom = select_nomenclature(project_dir, &haystack, &mut hits);
    if !nom.is_empty() {
        sections.push(("相关名词".into(), truncate_chars(&nom, 800)));
    }

    // --- 设定卡（排除 status=exited/consumed；主角 always-include 例外）---
    let entity_block = select_entities(project_dir, &haystack, &mut hits);
    if !entity_block.is_empty() {
        sections.push(("相关设定卡".into(), entity_block));
    }

    // --- 设定缺口（软提示：章纲优先补全，不阻断写章）---
    let gaps = crate::cards::collect_entity_gaps(project_dir);
    if !gaps.is_empty() {
        hits.push("entity_gaps".into());
        let joined = gaps.iter().take(8).cloned().collect::<Vec<_>>().join("\n");
        sections.push((
            "设定缺口（软提示：优先补全下列；勿另起未列主要实体）".into(),
            truncate_chars(&joined, 600),
        ));
    }

    // --- 剧情卡（生命周期 + 卷 active_main 优先）---
    let (plot_block, plot_hits) =
        crate::plots::select_plots_for_chapter(project_dir, chapter, &haystack);
    hits.extend(plot_hits);
    if !plot_block.is_empty() {
        sections.push(("相关剧情卡".into(), plot_block));
    }
    let index = crate::plots::load_plot_index(project_dir);
    if index.volumes.iter().any(|v| {
        v.plots
            .iter()
            .any(|p| p.status == "bridging")
    }) {
        hits.push("bridge_chapter".into());
        sections.push((
            "衔接章".into(),
            "【衔接章】本章消化上一剧情卡落点余波（人物反应、信息落点、各方态势），\
             **不要开全新主线高潮**；章末钩子导向下一张剧情卡或未竟线索。"
                .into(),
        ));
    }

    assemble(sections, hits)
}

fn assemble(sections: Vec<(String, String)>, hits: Vec<String>) -> ChapterContextPack {
    if sections.is_empty() {
        return ChapterContextPack::empty();
    }
    let mut md = String::from("# CanonContext（只读设定，与正文冲突时标 P0）\n");
    for (title, body) in sections {
        if body.trim().is_empty() {
            continue;
        }
        md.push_str(&format!("\n## {title}\n\n{body}\n"));
    }
    ChapterContextPack { markdown: md, hits }
}

/// Prefer `bible.md` (Studio/init), fall back to legacy `world_architect.md`.
pub fn read_world_doc(project_dir: &Path) -> (&'static str, String) {
    let bible = std::fs::read_to_string(project_dir.join("artifacts/bible.md")).unwrap_or_default();
    if bible.trim().len() > 20 {
        return ("bible", bible);
    }
    let legacy = std::fs::read_to_string(project_dir.join("artifacts/world_architect.md"))
        .unwrap_or_default();
    if legacy.trim().is_empty() {
        ("bible", String::new())
    } else {
        ("world_architect", legacy)
    }
}

fn select_act_goals(project_dir: &Path, chapter: u32, hits: &mut Vec<String>) -> String {
    let path = project_dir.join("artifacts/story_outline.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            // Fallback: master/arc outline markdown from Studio tools.
            return select_outline_markdown_fallback(project_dir, hits);
        }
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return select_outline_markdown_fallback(project_dir, hits);
    };
    let acts = v.get("acts").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    if acts.is_empty() {
        if let Some(md) = v.get("markdown").and_then(|x| x.as_str()) {
            if md.trim().len() > 20 {
                hits.push("story_outline".into());
                return truncate_chars(md, 800);
            }
        }
        return select_outline_markdown_fallback(project_dir, hits);
    }

    // Prefer act whose progress_notes mention this chapter, else by volume_index
    // heuristic: chapter 1-5 → volume 1, etc. (20 chapters per volume fallback).
    let mut chosen: Option<&Value> = None;
    for act in &acts {
        let notes = act
            .get("progress_notes")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if notes.contains(&format!("第{chapter}章")) || notes.contains(&format!("[{chapter}章]")) {
            chosen = Some(act);
            break;
        }
    }
    if chosen.is_none() {
        let vol = ((chapter.saturating_sub(1)) / 20) + 1;
        chosen = acts.iter().find(|a| {
            a.get("volume_index")
                .and_then(|x| x.as_u64())
                .map(|v| v as u32 == vol)
                .unwrap_or(false)
        });
    }
    let act = chosen.or_else(|| acts.first());
    let Some(act) = act else {
        return String::new();
    };
    hits.push(format!(
        "act:{}",
        act.get("name").and_then(|x| x.as_str()).unwrap_or("?")
    ));
    let mut lines = Vec::new();
    if let Some(name) = act.get("name").and_then(|x| x.as_str()) {
        lines.push(format!("幕：{name}"));
    }
    if let Some(goal) = act.get("goal").and_then(|x| x.as_str()) {
        if !goal.is_empty() {
            lines.push(format!("目标：{goal}"));
        }
    }
    if let Some(stakes) = act.get("stakes").and_then(|x| x.as_str()) {
        if !stakes.is_empty() {
            lines.push(format!("赌注：{stakes}"));
        }
    }
    if let Some(end) = act.get("ending_condition").and_then(|x| x.as_str()) {
        if !end.is_empty() {
            lines.push(format!("收束：{end}"));
        }
    }
    if let Some(theme) = v.get("theme").and_then(|x| x.as_str()) {
        if !theme.is_empty() {
            lines.push(format!("主题：{theme}"));
        }
    }
    lines.join("\n")
}

fn select_outline_markdown_fallback(project_dir: &Path, hits: &mut Vec<String>) -> String {
    crate::volume::migrate_legacy_arc_planner(project_dir);
    for (label, rel) in [
        ("master_outline", "artifacts/master_outline.md"),
        ("arc_outline", "artifacts/arc_outline.md"),
        ("story_outline_md", "artifacts/story_outline.md"),
    ] {
        let md = std::fs::read_to_string(project_dir.join(rel)).unwrap_or_default();
        if md.trim().len() > 20 {
            hits.push(label.into());
            return truncate_chars(md.trim(), 800);
        }
    }
    String::new()
}

fn select_memory(
    project_dir: &Path,
    chapter: u32,
    haystack: &str,
    profile: ContextProfile,
    hits: &mut Vec<String>,
) -> String {
    let mem = load_memory(project_dir);
    if mem.rolling_summary.is_empty()
        && mem.recent_digests.is_empty()
        && mem.open_threads.is_empty()
    {
        return String::new();
    }
    hits.push("memory".into());
    let mut parts = Vec::new();

    if !mem.rolling_summary.is_empty() {
        // Rolling is already recent-biased; keep its tail if still long.
        parts.push(format!(
            "滚动摘要：{}",
            truncate_chars_tail(&mem.rolling_summary, 900)
        ));
    }

    let digest_n = if profile == ContextProfile::Pacing {
        2
    } else {
        4
    };
    let prior: Vec<_> = mem
        .recent_digests
        .iter()
        .filter(|d| d.chapter < chapter || chapter == 0)
        .cloned()
        .collect();
    let recent: Vec<_> = prior.iter().rev().take(digest_n).collect();
    for d in recent.into_iter().rev() {
        if !d.event_summary.is_empty() {
            parts.push(format!(
                "近章摘要[第{}章]：{}",
                d.chapter,
                truncate_chars(&d.event_summary, 360)
            ));
        }
        if !d.plot_progress.is_empty() {
            parts.push(format!(
                "剧情推进[第{}章]：{}",
                d.chapter,
                truncate_chars(&d.plot_progress, 160)
            ));
        }
        if !d.hook.is_empty() {
            parts.push(format!(
                "钩子[第{}章]：{}",
                d.chapter,
                truncate_chars(&d.hook, 120)
            ));
        }
        // Surface relationship deltas briefly.
        let rel_notes: Vec<String> = d
            .relationship_deltas
            .iter()
            .filter_map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| x.get("note").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .or_else(|| {
                        let a = x.get("from").and_then(|v| v.as_str()).unwrap_or("");
                        let b = x.get("to").and_then(|v| v.as_str()).unwrap_or("");
                        let r = x
                            .get("relation")
                            .or_else(|| x.get("change"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if a.is_empty() && b.is_empty() && r.is_empty() {
                            None
                        } else {
                            Some(format!("{a}->{b}:{r}"))
                        }
                    })
            })
            .take(3)
            .collect();
        if !rel_notes.is_empty() {
            parts.push(format!(
                "关系变化[第{}章]：{}",
                d.chapter,
                rel_notes.join("；")
            ));
        }
    }

    // Keyword recall of archived chapter summaries outside the hot digest window.
    if profile == ContextProfile::Full {
        let exclude: Vec<u32> = mem.recent_digests.iter().map(|d| d.chapter).collect();
        let recalled = recall_archived_summaries(project_dir, haystack, &exclude, 2);
        for d in recalled {
            hits.push(format!("recall:ch{}", d.chapter));
            parts.push(format!(
                "召回摘要[第{}章]：{}{}",
                d.chapter,
                truncate_chars(&d.event_summary, 280),
                if d.hook.is_empty() {
                    String::new()
                } else {
                    format!("（钩子：{}）", truncate_chars(&d.hook, 80))
                }
            ));
        }
    }

    let mut open: Vec<_> = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .cloned()
        .collect();
    // Prefer recently planted threads.
    open.sort_by_key(|t| std::cmp::Reverse(t.planted_chapter));
    let open_texts: Vec<String> = open
        .into_iter()
        .take(8)
        .map(|t| t.text)
        .filter(|t| !t.is_empty())
        .collect();
    if !open_texts.is_empty() {
        parts.push(format!("未收线：\n- {}", open_texts.join("\n- ")));
    }

    parts.join("\n\n")
}

fn select_nomenclature(project_dir: &Path, haystack: &str, hits: &mut Vec<String>) -> String {
    let path = project_dir.join("lore/nomenclature.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        // Fallback to markdown nomenclature artifact.
        let md = std::fs::read_to_string(project_dir.join("artifacts/nomenclature.md"))
            .unwrap_or_default();
        if md.trim().is_empty() {
            return String::new();
        }
        hits.push("nomenclature.md".into());
        return truncate_chars(md.trim(), 800);
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    let entities = v
        .get("entities")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let mut matched = Vec::new();
    let mut fallback = Vec::new();
    for e in &entities {
        let name = e
            .get("canonical_name")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let mut aliases: Vec<&str> = e
            .get("aliases")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        aliases.push(name);
        let hit = aliases.iter().any(|a| !a.is_empty() && haystack.contains(a));
        let trait_ = e
            .get("ability_or_trait")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let line = if trait_.is_empty() {
            format!("- {name}")
        } else {
            format!("- {name}：{}", truncate_chars(trait_, 80))
        };
        if hit {
            matched.push(line);
            hits.push(format!("nom:{name}"));
        } else if fallback.len() < 6 {
            fallback.push(line);
        }
    }
    if matched.is_empty() {
        if !fallback.is_empty() {
            hits.push("nomenclature:core".into());
        }
        return fallback.join("\n");
    }
    matched.truncate(16);
    matched.join("\n")
}

fn select_entities(project_dir: &Path, haystack: &str, hits: &mut Vec<String>) -> String {
    let mut cards: Vec<MarkdownCard> = Vec::new();
    for group in ["characters", "items", "locations"] {
        cards.extend(load_markdown_cards(
            &project_dir.join("entities").join(group),
            group,
        ));
    }
    // Project-declared always-include names (meta.json / state), never hardcode titles.
    let always_names = load_always_include_names(project_dir);
    let mut selected: Vec<&MarkdownCard> = Vec::new();
    for c in &cards {
        let always = card_always_include(c, &always_names);
        // Exited/consumed cards are excluded unless they are the declared protagonist.
        if !c.is_active_for_canon() && !always {
            continue;
        }
        let keys = c.match_keys();
        let hit = keys
            .iter()
            .any(|k| k.chars().count() >= 2 && haystack.contains(k));
        if always || hit {
            selected.push(c);
        }
    }
    // Dedup by slug, cap 6
    let mut seen = std::collections::HashSet::new();
    selected.retain(|c| seen.insert(c.slug.clone()));
    selected.truncate(6);

    if selected.is_empty() {
        // Fallback: first 2 active character cards
        for c in cards
            .iter()
            .filter(|c| c.category.contains("character") && c.is_active_for_canon())
            .take(2)
        {
            selected.push(c);
        }
    }

    let mut blocks = Vec::new();
    for c in selected {
        hits.push(format!("entity:{}", c.name));
        let status = c.meta.get("status").map(|s| s.as_str()).unwrap_or("active");
        let holdings = c
            .meta
            .get("holdings")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let meta_line = match holdings {
            Some(h) => format!("status={status} · holdings={h}"),
            None => format!("status={status}"),
        };
        let body = truncate_chars(c.markdown.trim(), 500);
        blocks.push(format!(
            "### {}（{} · {}）\n{body}",
            c.name, c.category, meta_line
        ));
    }
    blocks.join("\n\n")
}

/// Names that should always enter CanonContext for this project.
/// Sources (generic, project-owned data — not skill/code hardcodes):
/// - `meta.json`: `protagonist` (string) or `protagonists` (array)
/// - `state.json` → `meta.protagonists` / `meta.protagonist`
fn load_always_include_names(project_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let push = |names: &mut Vec<String>, s: &str| {
        let t = s.trim();
        if !t.is_empty() && !names.iter().any(|n| n == t) {
            names.push(t.to_string());
        }
    };
    let absorb_value = |names: &mut Vec<String>, v: &Value| {
        if let Some(s) = v.as_str() {
            push(names, s);
        } else if let Some(arr) = v.as_array() {
            for x in arr {
                if let Some(s) = x.as_str() {
                    push(names, s);
                }
            }
        }
    };

    if let Ok(text) = std::fs::read_to_string(project_dir.join("meta.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(p) = v.get("protagonist") {
                absorb_value(&mut names, p);
            }
            if let Some(p) = v.get("protagonists") {
                absorb_value(&mut names, p);
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(project_dir.join("state.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(meta) = v.get("meta") {
                if let Some(p) = meta.get("protagonist") {
                    absorb_value(&mut names, p);
                }
                if let Some(p) = meta.get("protagonists") {
                    absorb_value(&mut names, p);
                }
            }
        }
    }
    names
}

fn card_always_include(card: &MarkdownCard, always_names: &[String]) -> bool {
    if card
        .meta
        .get("canon_always")
        .map(|v| matches!(v.as_str(), "true" | "yes" | "1"))
        .unwrap_or(false)
    {
        return true;
    }
    if card
        .meta
        .get("role")
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "protagonist" | "main" | "lead"
            )
        })
        .unwrap_or(false)
    {
        return true;
    }
    let keys = card.match_keys();
    always_names
        .iter()
        .any(|n| keys.iter().any(|k| k == n))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn fixture_project(root: &Path) {
        write(
            &root.join("meta.json"),
            r#"{"name":"demo","protagonists":["林舟"]}"#,
        );
        write(
            &root.join("artifacts/story_outline.json"),
            r#"{"acts":[{"name":"第一幕","volume_index":1,"goal":"觉醒与追查","progress_notes":"[第1章] 开端"}]}"#,
        );
        write(
            &root.join("artifacts/world_architect.md"),
            "# 世界观\n\n这是一个通用测试世界。\n",
        );
        write(
            &root.join("lore/nomenclature.json"),
            r#"{"entities":[{"canonical_name":"潮纹","ability_or_trait":"水系共鸣","aliases":[]}]}"#,
        );
        write(
            &root.join("entities/characters/林舟.md"),
            "---\nid: char-001\nname: 林舟\ncategory: character\nrole: protagonist\n---\n\n# 林舟\n\n主角，冷静观察者。\n",
        );
        write(
            &root.join("entities/characters/客串甲.md"),
            "---\nid: char-002\nname: 客串甲\ncategory: character\n---\n\n# 客串甲\n\n路人。\n",
        );
        write(
            &root.join("plots/第一幕.md"),
            "---\nid: plot-1\ntitle: 第一幕·觉醒\ncategory: plot\nvolume_index: 1\nplot_type: main\nstatus: in_progress\nchapter_from: 1\nchapter_to: 5\nbridge_chapter: 6\n---\n\n# 第一幕\n\n主角卷入异变。\n",
        );
    }

    #[test]
    fn hits_from_haystack_and_meta_protagonist() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-canon-fixture");
        let _ = fs::remove_dir_all(&dir);
        fixture_project(&dir);

        let draft = "林舟触碰潮纹，异变开始。";
        let pack = build_chapter_context(&dir, 1, draft, "", ContextProfile::Full);
        assert!(pack.markdown.contains("CanonContext"));
        assert!(
            pack.hits.iter().any(|h| h.contains("林舟")),
            "hits={:?}",
            pack.hits
        );
        assert!(
            pack.hits.iter().any(|h| h.starts_with("nom:潮纹") || h.contains("潮纹")),
            "hits={:?}",
            pack.hits
        );
        assert!(
            pack.hits.iter().any(|h| h.starts_with("plot:") || h.starts_with("act:")),
            "hits={:?}",
            pack.hits
        );
        assert!(pack.markdown.chars().count() < 12_000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefers_bible_md_over_legacy_world_architect() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-canon-bible");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("artifacts/bible.md"),
            "# Bible\n\n这是正式 Bible，应被优先读取。\n",
        );
        write(
            &dir.join("artifacts/world_architect.md"),
            "# Legacy\n\n旧路径。\n",
        );
        write(
            &dir.join("artifacts/master_outline.md"),
            "# 总纲\n\n主线是追查异变源头。\n",
        );
        let (src, body) = read_world_doc(&dir);
        assert_eq!(src, "bible");
        assert!(body.contains("正式 Bible"));
        let pack = build_chapter_context(&dir, 1, "开场", "", ContextProfile::Full);
        assert!(pack.hits.iter().any(|h| h == "bible"));
        assert!(pack.markdown.contains("正式 Bible"));
        // No story_outline.json → fall back to master_outline.md
        assert!(pack.hits.iter().any(|h| h == "master_outline"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn role_protagonist_always_included_without_haystack_hit() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-canon-always");
        let _ = fs::remove_dir_all(&dir);
        fixture_project(&dir);
        // Draft mentions neither name nor tide mark — protagonist still via role/meta.
        let pack = build_chapter_context(&dir, 1, "天亮了。", "", ContextProfile::Full);
        assert!(
            pack.hits.iter().any(|h| h.contains("林舟")),
            "hits={:?}",
            pack.hits
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pacing_profile_is_shorter() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-canon-pacing");
        let _ = fs::remove_dir_all(&dir);
        fixture_project(&dir);
        let full = build_chapter_context(&dir, 2, "林舟与潮纹", "", ContextProfile::Full);
        let pacing = build_chapter_context(&dir, 2, "林舟与潮纹", "", ContextProfile::Pacing);
        assert!(pacing.markdown.chars().count() <= full.markdown.chars().count());
        assert!(!pacing.markdown.contains("相关设定卡"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn exited_entities_excluded_and_gaps_injected() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-canon-exited");
        let _ = fs::remove_dir_all(&dir);
        fixture_project(&dir);
        write(
            &dir.join("entities/characters/客串甲.md"),
            "---\nname: 客串甲\nstatus: exited\ncomplete: false\n---\n\n# 客串甲\n\n已退场。\n",
        );
        let pack = build_chapter_context(&dir, 1, "客串甲与林舟会面", "", ContextProfile::Full);
        assert!(
            !pack.hits.iter().any(|h| h == "entity:客串甲"),
            "exited card must not enter canon: {:?}",
            pack.hits
        );
        assert!(pack.hits.iter().any(|h| h == "entity_gaps"));
        assert!(pack.markdown.contains("设定缺口"));
        let pacing = build_chapter_context(&dir, 1, "客串甲与林舟会面", "", ContextProfile::Pacing);
        assert!(!pacing.hits.iter().any(|h| h == "entity_gaps"));
        let _ = fs::remove_dir_all(&dir);
    }
}
