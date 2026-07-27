//! Minimal rule-based agent activation from `config/agents.yaml`.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

use crate::PipelineConfig;

#[derive(Debug, Clone, Default)]
pub struct ActivationSignals {
    pub published_count: u32,
    pub entity_count: usize,
    pub open_foreshadows: usize,
    pub draft_chars: usize,
    pub has_bible: bool,
    pub has_master_outline: bool,
    pub has_arc_outline: bool,
    pub has_nomenclature: bool,
    pub dialogue_ratio: f64,
    pub speaking_characters: usize,
    pub scene_tags: Vec<String>,
    pub has_new_entity_hints: bool,
    pub audit_fail_rate: f64,
    pub bible_stale: bool,
}

#[derive(Debug, Deserialize)]
struct AgentsFile {
    #[serde(default)]
    agents: std::collections::HashMap<String, AgentEntry>,
}

#[derive(Debug, Deserialize)]
struct AgentEntry {
    #[serde(default)]
    activation: Vec<ActivationRule>,
}

#[derive(Debug, Deserialize)]
struct ActivationRule {
    condition: String,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Clone)]
pub struct ActivationSuggestion {
    pub agent: String,
    pub reason: String,
}

/// Evaluate activation rules; any matching condition yields a suggestion.
pub fn evaluate_activation(
    config_root: &Path,
    signals: &ActivationSignals,
) -> Vec<ActivationSuggestion> {
    let path = config_root.join("agents.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(file) = serde_yaml::from_str::<AgentsFile>(&text) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (id, entry) in file.agents {
        for rule in &entry.activation {
            if condition_matches(rule, signals) {
                out.push(ActivationSuggestion {
                    agent: id.clone(),
                    reason: if rule.reason.is_empty() {
                        rule.condition.clone()
                    } else {
                        rule.reason.clone()
                    },
                });
                break;
            }
        }
    }
    out
}

fn condition_matches(rule: &ActivationRule, s: &ActivationSignals) -> bool {
    let thr = rule.threshold.unwrap_or(0.0);
    match rule.condition.as_str() {
        "no_bible" => !s.has_bible,
        "has_bible" => s.has_bible,
        "no_master_outline" => !s.has_master_outline,
        "no_arc_outline" => !s.has_arc_outline,
        "no_nomenclature" => !s.has_nomenclature,
        "entity_count_gt" => (s.entity_count as f64) > thr,
        "chapter_count_gt" => (s.published_count as f64) > thr,
        "chapters_in_arc_gt" => (s.published_count as f64) > thr,
        "active_foreshadows_gt" => (s.open_foreshadows as f64) > thr,
        "word_count_gt" => (s.draft_chars as f64) > thr,
        "dialogue_ratio_gt" => s.dialogue_ratio > thr,
        "character_count_gt" => (s.speaking_characters as f64) > thr,
        "has_scene_tags" => {
            if rule.tags.is_empty() {
                !s.scene_tags.is_empty()
            } else {
                rule.tags
                    .iter()
                    .any(|t| s.scene_tags.iter().any(|x| x.eq_ignore_ascii_case(t)))
            }
        }
        "has_new_entity_hints" => s.has_new_entity_hints,
        "audit_fail_rate_gt" => s.audit_fail_rate > thr,
        "bible_stale" => s.bible_stale,
        "always_after_auditor" => true,
        _ => false,
    }
}

/// Merge project active + MVP + activation suggestions, ordered by pipeline config.
pub fn resolve_pipeline_agents(
    base_active: &[String],
    pipe: &PipelineConfig,
    suggestions: &[ActivationSuggestion],
) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    for a in pipe.mvp() {
        set.insert(a.clone());
    }
    for a in base_active {
        set.insert(a.clone());
    }
    for s in suggestions {
        if pipe.is_pipeline_agent(&s.agent) {
            set.insert(s.agent.clone());
        }
    }
    pipe.order()
        .iter()
        .filter(|a| set.contains(*a))
        .cloned()
        .collect()
}

/// Collect filesystem + draft heuristics for activation.
pub fn collect_signals(
    project_dir: &Path,
    published_count: u32,
    draft: &str,
) -> ActivationSignals {
    let outline = find_latest_outline(project_dir, published_count);
    let entity_names = list_entity_names(project_dir);
    let entity_count = entity_names.len();
    let has_bible = path_nonempty(&project_dir.join("artifacts/bible.md"))
        || path_nonempty(&project_dir.join("lore/bible.md"))
        || path_nonempty(&project_dir.join("artifacts/world_architect.md"));
    let has_master = path_nonempty(&project_dir.join("artifacts/master_outline.md"))
        || path_nonempty(&project_dir.join("artifacts/story_outline.md"))
        || path_nonempty(&project_dir.join("artifacts/story_outline.json"));
    // Canonical 卷纲: artifacts/arc_outlines/{NN}.md (+ legacy flat file).
    let has_arc = path_nonempty(&project_dir.join("artifacts/arc_outline.md"))
        || path_nonempty(&project_dir.join("artifacts/arc_planner.md"))
        || project_dir.join("artifacts").join("arcs").is_dir()
        || std::fs::read_dir(project_dir.join("artifacts/arc_outlines"))
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.path().extension().and_then(|s| s.to_str()) == Some("md")
                        && std::fs::read_to_string(e.path())
                            .map(|t| t.trim().chars().count() > 20)
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false);
    let has_nom = path_nonempty(&project_dir.join("lore/nomenclature.md"))
        || path_nonempty(&project_dir.join("artifacts/nomenclature.md"))
        || path_nonempty(&project_dir.join("lore/nomenclature.json"));

    let (dialogue_ratio, speaking_characters) = dialogue_stats(draft);
    let scene_tags = extract_scene_tags(&outline);
    let has_new_entity_hints = detect_new_entity_hints(draft, &entity_names);
    let audit_fail_rate = recent_audit_fail_rate(project_dir, published_count);
    let bible_stale = is_bible_stale(project_dir);

    ActivationSignals {
        published_count,
        entity_count,
        open_foreshadows: count_open_threads(project_dir),
        draft_chars: draft.chars().count(),
        has_bible,
        has_master_outline: has_master,
        has_arc_outline: has_arc,
        has_nomenclature: has_nom,
        dialogue_ratio,
        speaking_characters,
        scene_tags,
        has_new_entity_hints,
        audit_fail_rate,
        bible_stale,
    }
}

fn find_latest_outline(project_dir: &Path, published_count: u32) -> String {
    let ch = published_count.max(1);
    for n in (1..=ch).rev() {
        let base = project_dir.join("chapters").join(format!("{n:03}"));
        for name in ["outline.json", "outline.md"] {
            let p = base.join(name);
            if let Ok(t) = std::fs::read_to_string(p) {
                if !t.trim().is_empty() {
                    return t;
                }
            }
        }
    }
    String::new()
}

fn dialogue_stats(draft: &str) -> (f64, usize) {
    if draft.is_empty() {
        return (0.0, 0);
    }
    let total = draft.chars().count().max(1);
    let mut dialogue_chars = 0usize;
    let mut speakers = HashSet::new();
    // Count chars inside 「」 "" “”
    let mut depth = 0i32;
    let mut buf = String::new();
    for ch in draft.chars() {
        match ch {
            '「' | '“' | '"' if depth == 0 => {
                depth = 1;
                buf.clear();
            }
            '」' | '”' | '"' if depth > 0 => {
                dialogue_chars += buf.chars().count() + 2;
                // crude speaker: look for Name说 before quote — skip; count unique short names in quote tags later
                depth = 0;
            }
            _ if depth > 0 => buf.push(ch),
            _ => {}
        }
    }
    // Speakers: lines like 张三：「 or 张三道：
    for line in draft.lines() {
        if let Some(idx) = line.find('「').or_else(|| line.find('“')) {
            let prefix: String = line[..idx]
                .chars()
                .rev()
                .take(4)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            let name: String = prefix
                .chars()
                .filter(|c| !matches!(c, '：' | ':' | '道' | '说' | '问' | '喊' | '，' | ',' | ' '))
                .collect();
            if (2..=4).contains(&name.chars().count()) {
                speakers.insert(name);
            }
        }
    }
    ((dialogue_chars as f64) / (total as f64), speakers.len())
}

fn extract_scene_tags(outline: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let lower = outline.to_lowercase();
    for (tag, keys) in [
        ("battle", &["战斗", "交手", "厮杀", "battle"][..]),
        ("chase", &["追逐", "逃", "chase"][..]),
        ("action", &["动作", "交锋", "action"][..]),
        ("climax", &["高潮", "决战", "climax"][..]),
    ] {
        if keys.iter().any(|k| lower.contains(k) || outline.contains(k)) {
            tags.push(tag.into());
        }
    }
    // YAML-ish tags: tags: [battle, ...]
    if let Some(start) = outline.find("tags:") {
        let rest = &outline[start..];
        for t in ["battle", "chase", "action", "climax"] {
            if rest.to_lowercase().contains(t) && !tags.iter().any(|x| x == t) {
                tags.push(t.into());
            }
        }
    }
    tags
}

fn detect_new_entity_hints(draft: &str, known: &[String]) -> bool {
    // 《专名》 not in entity list
    let mut inside = false;
    let mut buf = String::new();
    for ch in draft.chars() {
        if ch == '《' {
            inside = true;
            buf.clear();
        } else if ch == '》' && inside {
            inside = false;
            if buf.chars().count() >= 2
                && !known.iter().any(|k| k == &buf || k.contains(&buf))
            {
                return true;
            }
        } else if inside {
            buf.push(ch);
        }
    }
    false
}

fn recent_audit_fail_rate(project_dir: &Path, published_count: u32) -> f64 {
    if published_count == 0 {
        return 0.0;
    }
    let start = published_count.saturating_sub(4).max(1);
    let mut total = 0u32;
    let mut fails = 0u32;
    for n in start..=published_count {
        let path = project_dir
            .join("chapters")
            .join(format!("{n:03}"))
            .join("audit.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        total += 1;
        if v.get("passed").and_then(|x| x.as_bool()) == Some(false) {
            fails += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        fails as f64 / total as f64
    }
}

fn is_bible_stale(project_dir: &Path) -> bool {
    let bible = newest_mtime(&[
        project_dir.join("artifacts/bible.md"),
        project_dir.join("artifacts/world_architect.md"),
    ]);
    let entities = newest_mtime_under(&project_dir.join("entities"));
    match (bible, entities) {
        (Some(b), Some(e)) => e > b,
        _ => false,
    }
}

fn newest_mtime(paths: &[std::path::PathBuf]) -> Option<SystemTime> {
    paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
}

fn newest_mtime_under(root: &Path) -> Option<SystemTime> {
    let mut best: Option<SystemTime> = None;
    let Ok(walker) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in walker.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(t) = newest_mtime_under(&p) {
                best = Some(best.map_or(t, |b| b.max(t)));
            }
        } else if let Ok(m) = std::fs::metadata(&p).and_then(|m| m.modified()) {
            best = Some(best.map_or(m, |b| b.max(m)));
        }
    }
    best
}

fn list_entity_names(project_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for group in ["characters", "items", "locations"] {
        let dir = project_dir.join("entities").join(group);
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names
}

fn path_nonempty(p: &Path) -> bool {
    std::fs::read_to_string(p)
        .map(|t| t.trim().len() > 20)
        .unwrap_or(false)
}

fn count_open_threads(project_dir: &Path) -> usize {
    let path = project_dir.join("lore/memory.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    v.get("open_threads")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter(|t| t.get("status").and_then(|s| s.as_str()) != Some("resolved"))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_orders_by_pipeline() {
        let suggestions = vec![ActivationSuggestion {
            agent: "literary_editor".into(),
            reason: "test".into(),
        }];
        let pipe = PipelineConfig::defaults();
        let got = resolve_pipeline_agents(&["writer".into()], &pipe, &suggestions);
        assert!(got.iter().position(|a| a == "writer") < got.iter().position(|a| a == "summarizer"));
        assert!(got.contains(&"literary_editor".to_string()));
    }

    #[test]
    fn dialogue_ratio_detects_quotes() {
        let (r, n) = dialogue_stats("张三：「你好。」李四站着。");
        assert!(r > 0.0);
        assert!(n >= 1);
    }
}
