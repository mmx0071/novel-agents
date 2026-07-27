//! Minimal rule-based agent activation from `config/agents.yaml`.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

use crate::longform::QualityTier;
use crate::PipelineConfig;

#[derive(Debug, Clone, Default)]
pub struct ActivationSignals {
    pub published_count: u32,
    /// Chapters written in the current volume/arc (not whole-book published_count).
    pub chapters_in_current_arc: u32,
    /// Chapter being planned/written (for lean foreshadow cadence).
    pub chapter: u32,
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
    /// Volume phase is handoff / awaiting sync (long-range QA hint).
    pub volume_handoff: bool,
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
        "chapters_in_arc_gt" => (s.chapters_in_current_arc as f64) > thr,
        "chapter_mod_eq" => {
            let m = thr as u32;
            m > 0 && s.chapter > 0 && s.chapter % m == 0
        }
        "volume_handoff" => s.volume_handoff,
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
    resolve_pipeline_agents_filtered(base_active, pipe, suggestions, None)
}

/// Like [`resolve_pipeline_agents`], with optional longform lean filtering.
///
/// Lean may drop activation **suggestions** (and foreshadow cadence), but never
/// removes agents that are already in `base_active` (user/Studio persistence).
///
/// `tier` refines lean aggressiveness (`economy` > `balanced` > `quality`=no lean).
pub fn resolve_pipeline_agents_filtered(
    base_active: &[String],
    pipe: &PipelineConfig,
    suggestions: &[ActivationSuggestion],
    lean: Option<&ActivationSignals>,
) -> Vec<String> {
    resolve_pipeline_agents_with_tier(base_active, pipe, suggestions, lean, QualityTier::Balanced)
}

/// Resolve pipeline agents with an explicit [`QualityTier`].
pub fn resolve_pipeline_agents_with_tier(
    base_active: &[String],
    pipe: &PipelineConfig,
    suggestions: &[ActivationSuggestion],
    lean: Option<&ActivationSignals>,
    tier: QualityTier,
) -> Vec<String> {
    let pinned: HashSet<String> = base_active.iter().cloned().collect();
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
    // quality: keep all suggestions; no lean stripping.
    if matches!(tier, QualityTier::Quality) {
        return pipe
            .order()
            .iter()
            .filter(|a| set.contains(*a))
            .cloned()
            .collect();
    }
    if let Some(sig) = lean {
        let economy = matches!(tier, QualityTier::Economy);
        // economy: foreshadow only on even chapters when no open threads.
        // balanced: never drop foreshadow when open threads remain; otherwise even cadence.
        let drop_foreshadow = if economy {
            sig.published_count >= 3
                && sig.open_foreshadows == 0
                && sig.chapter > 0
                && sig.chapter % 2 != 0
                && !pinned.contains("foreshadow_tracker")
        } else {
            // balanced
            sig.published_count >= 3
                && sig.open_foreshadows == 0
                && sig.chapter > 0
                && sig.chapter % 2 != 0
                && !pinned.contains("foreshadow_tracker")
        };
        if drop_foreshadow {
            set.remove("foreshadow_tracker");
        }
        // Dialogue/scene: only drop suggestion-only agents when we have evidence.
        // Empty draft at plan-time must NOT strip specialists (continue_writing path).
        let dialogue_heavy = sig.dialogue_ratio > 0.22 || sig.speaking_characters >= 3;
        let has_draft_signal = sig.draft_chars >= 200;
        if has_draft_signal && !dialogue_heavy && !pinned.contains("dialogue_specialist") {
            set.remove("dialogue_specialist");
        }
        let scene_heavy = sig.scene_tags.iter().any(|t| {
            let t = t.to_lowercase();
            t.contains("战斗")
                || t.contains("动作")
                || t.contains("battle")
                || t.contains("action")
                || t.contains("chase")
                || t.contains("高潮")
        });
        // Only skip scene when tags are present and clearly non-action; unknown → keep.
        // economy: also drop literary_editor suggestion-only.
        if !sig.scene_tags.is_empty() && !scene_heavy && !pinned.contains("scene_specialist") {
            set.remove("scene_specialist");
        }
        if economy && !pinned.contains("literary_editor") {
            set.remove("literary_editor");
        }
        if economy && !pinned.contains("nomenclature_curator") && sig.published_count >= 5 {
            // After early chapters, skip nomenclature unless pinned.
            set.remove("nomenclature_curator");
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
        chapters_in_current_arc: 0,
        chapter: 0,
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
        volume_handoff: false,
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

    #[test]
    fn chapters_in_arc_uses_arc_field_not_published() {
        let rule = ActivationRule {
            condition: "chapters_in_arc_gt".into(),
            threshold: Some(8.0),
            tags: vec![],
            reason: "arc".into(),
        };
        let mut s = ActivationSignals {
            published_count: 100,
            chapters_in_current_arc: 3,
            ..Default::default()
        };
        assert!(!condition_matches(&rule, &s));
        s.chapters_in_current_arc = 9;
        assert!(condition_matches(&rule, &s));
    }

    #[test]
    fn lean_drops_foreshadow_on_odd_without_open() {
        let pipe = PipelineConfig::defaults();
        let suggestions = vec![ActivationSuggestion {
            agent: "foreshadow_tracker".into(),
            reason: "ch3+".into(),
        }];
        let sig = ActivationSignals {
            published_count: 5,
            chapter: 5,
            open_foreshadows: 0,
            ..Default::default()
        };
        let got = resolve_pipeline_agents_filtered(&[], &pipe, &suggestions, Some(&sig));
        assert!(!got.iter().any(|a| a == "foreshadow_tracker"));
        let sig2 = ActivationSignals {
            published_count: 5,
            chapter: 6,
            open_foreshadows: 0,
            ..Default::default()
        };
        let got2 = resolve_pipeline_agents_filtered(&[], &pipe, &suggestions, Some(&sig2));
        assert!(got2.iter().any(|a| a == "foreshadow_tracker"));
        let sig3 = ActivationSignals {
            published_count: 5,
            chapter: 5,
            open_foreshadows: 2,
            ..Default::default()
        };
        let got3 = resolve_pipeline_agents_filtered(&[], &pipe, &suggestions, Some(&sig3));
        assert!(got3.iter().any(|a| a == "foreshadow_tracker"));
    }

    #[test]
    fn lean_skips_dialogue_and_scene_when_not_tagged() {
        let pipe = PipelineConfig::defaults();
        let suggestions = vec![
            ActivationSuggestion {
                agent: "dialogue_specialist".into(),
                reason: "dialog".into(),
            },
            ActivationSuggestion {
                agent: "scene_specialist".into(),
                reason: "scene".into(),
            },
        ];
        // Empty draft at plan-time: keep dialogue suggestion (unknown until written).
        let empty_draft = ActivationSignals {
            published_count: 10,
            chapter: 10,
            draft_chars: 0,
            dialogue_ratio: 0.0,
            speaking_characters: 0,
            scene_tags: vec![],
            ..Default::default()
        };
        let got0 = resolve_pipeline_agents_filtered(&[], &pipe, &suggestions, Some(&empty_draft));
        assert!(got0.iter().any(|a| a == "dialogue_specialist"));
        // Quiet chapter with real draft + non-action tags → drop both.
        let quiet = ActivationSignals {
            published_count: 10,
            chapter: 10,
            draft_chars: 800,
            dialogue_ratio: 0.05,
            speaking_characters: 0,
            scene_tags: vec!["日常".into()],
            ..Default::default()
        };
        let got = resolve_pipeline_agents_filtered(&[], &pipe, &suggestions, Some(&quiet));
        assert!(!got.iter().any(|a| a == "dialogue_specialist"));
        assert!(!got.iter().any(|a| a == "scene_specialist"));
        let heavy = ActivationSignals {
            published_count: 10,
            chapter: 10,
            draft_chars: 800,
            dialogue_ratio: 0.4,
            speaking_characters: 4,
            scene_tags: vec!["战斗".into()],
            ..Default::default()
        };
        let got2 = resolve_pipeline_agents_filtered(&[], &pipe, &suggestions, Some(&heavy));
        assert!(got2.iter().any(|a| a == "dialogue_specialist"));
        assert!(got2.iter().any(|a| a == "scene_specialist"));
        // Pinned active_agents survive lean even when quiet.
        let pinned = vec![
            "dialogue_specialist".into(),
            "scene_specialist".into(),
        ];
        let got3 = resolve_pipeline_agents_filtered(&pinned, &pipe, &[], Some(&quiet));
        assert!(got3.iter().any(|a| a == "dialogue_specialist"));
        assert!(got3.iter().any(|a| a == "scene_specialist"));
    }
}
