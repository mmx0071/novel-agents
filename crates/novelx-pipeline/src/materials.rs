//! Reference material cards (non-Canon) for plot drought / inspiration.
//! Path: `projects/<name>/materials/<id>.json` + index `.novelx/materials_index.json`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::project::project_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialCard {
    pub id: String,
    #[serde(default)]
    pub motif_tags: Vec<String>,
    #[serde(default)]
    pub sources_style: String,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default = "default_true")]
    pub do_not_canonize: bool,
    #[serde(default)]
    pub usable_in: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default = "default_provenance")]
    pub provenance: String,
    /// Chapter when this card was researched (cooldown tracking).
    #[serde(default)]
    pub created_at_chapter: u32,
}

fn default_true() -> bool {
    true
}
fn default_provenance() -> String {
    "model_grounded".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialsIndex {
    #[serde(default)]
    pub cards: Vec<MaterialIndexEntry>,
    /// Last chapter when material_researcher ran.
    #[serde(default)]
    pub last_research_chapter: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialIndexEntry {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub motif_tags: Vec<String>,
    #[serde(default)]
    pub created_at_chapter: u32,
}

pub fn materials_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("materials")
}

pub fn materials_index_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".novelx").join("materials_index.json")
}

pub fn load_materials_index(project_dir: &Path) -> MaterialsIndex {
    let path = materials_index_path(project_dir);
    let Ok(raw) = fs::read_to_string(&path) else {
        return MaterialsIndex::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_materials_index(project_dir: &Path, index: &MaterialsIndex) -> Result<()> {
    let path = materials_index_path(project_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(index)?;
    fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Whether cooldown allows another research call at `chapter`.
pub fn material_cooldown_allows(
    project_dir: &Path,
    chapter: u32,
    cooldown_chapters: u32,
) -> bool {
    if cooldown_chapters == 0 {
        return true;
    }
    let idx = load_materials_index(project_dir);
    if idx.last_research_chapter == 0 {
        return true;
    }
    chapter.saturating_sub(idx.last_research_chapter) >= cooldown_chapters
}

pub fn save_material_cards(
    projects_root: &Path,
    project: &str,
    chapter: u32,
    cards: &[MaterialCard],
) -> Result<Vec<String>> {
    let dir = project_dir(projects_root, project);
    let mat_dir = materials_dir(&dir);
    fs::create_dir_all(&mat_dir)?;
    let mut index = load_materials_index(&dir);
    let mut ids = Vec::new();
    for card in cards {
        let mut c = card.clone();
        c.do_not_canonize = true;
        if c.provenance.trim().is_empty() {
            c.provenance = default_provenance();
        }
        if c.created_at_chapter == 0 {
            c.created_at_chapter = chapter;
        }
        if c.id.trim().is_empty() {
            c.id = format!("mat_{}_{}", chapter, ids.len() + 1);
        }
        let path = mat_dir.join(format!("{}.json", sanitize_id(&c.id)));
        fs::write(&path, serde_json::to_string_pretty(&c)?)?;
        index.cards.retain(|e| e.id != c.id);
        index.cards.push(MaterialIndexEntry {
            id: c.id.clone(),
            summary: c.summary.clone(),
            motif_tags: c.motif_tags.clone(),
            created_at_chapter: c.created_at_chapter,
        });
        ids.push(c.id);
    }
    index.last_research_chapter = chapter.max(index.last_research_chapter);
    save_materials_index(&dir, &index)?;
    Ok(ids)
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Short hooks block for chapter_planner / design_plot (not writer).
pub fn format_material_hooks_for_context(project_dir: &Path, max_chars: usize) -> String {
    let index = load_materials_index(project_dir);
    if index.cards.is_empty() {
        return String::new();
    }
    let mat_dir = materials_dir(project_dir);
    let mut lines = vec!["【参考素材钩子（非 Canon，勿直接写进设定）】".to_string()];
    let mut used = lines[0].chars().count();
    for entry in index.cards.iter().rev().take(6) {
        let path = mat_dir.join(format!("{}.json", sanitize_id(&entry.id)));
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(card) = serde_json::from_str::<MaterialCard>(&raw) else {
            continue;
        };
        for hook in card.hooks.iter().take(2) {
            let line = format!("- {}", hook.trim());
            let n = line.chars().count() + 1;
            if used + n > max_chars {
                return lines.join("\n");
            }
            lines.push(line);
            used += n;
        }
    }
    if lines.len() <= 1 {
        String::new()
    } else {
        lines.join("\n")
    }
}

/// Parse agent JSON output into cards (tolerant of wrapper objects).
pub fn parse_material_cards_json(value: &Value, max_cards: usize) -> Vec<MaterialCard> {
    let arr = value
        .get("cards")
        .and_then(|v| v.as_array())
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default();
    arr.into_iter()
        .take(max_cards.max(1))
        .filter_map(|v| serde_json::from_value::<MaterialCard>(v).ok())
        .map(|mut c| {
            c.do_not_canonize = true;
            if c.provenance.is_empty() {
                c.provenance = default_provenance();
            }
            c
        })
        .collect()
}

pub fn material_research_prompt(
    brief_excerpt: &str,
    bible_themes: &str,
    plot_gaps: &str,
    drought_reason: &str,
    max_cards: u32,
) -> String {
    format!(
        "请根据以下项目母题产出最多 {max_cards} 张参考素材卡（纯 JSON，见 Skill 契约）。\n\
         枯竭原因标签：{drought_reason}\n\n\
         ## Brief\n{}\n\n## Bible 题材/母题切片\n{}\n\n## 当前缺口\n{}\n",
        truncate(brief_excerpt, 800),
        truncate(bible_themes, 1200),
        truncate(plot_gaps, 600),
    )
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        t.chars().take(max).collect::<String>() + "…"
    }
}

/// Mark inspiration/drought signals on disk for activation heuristics.
pub fn set_drought_flag(project_dir: &Path, reason: &str, chapter: u32) -> Result<()> {
    let path = project_dir.join(".novelx").join("plot_drought.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let v = json!({
        "active": true,
        "reason": reason,
        "chapter": chapter,
    });
    fs::write(&path, serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

pub fn clear_drought_flag(project_dir: &Path) -> Result<()> {
    let path = project_dir.join(".novelx").join("plot_drought.json");
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn drought_flag_active(project_dir: &Path) -> bool {
    let path = project_dir.join(".novelx").join("plot_drought.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("active").and_then(|x| x.as_bool()))
        .unwrap_or(false)
}

pub fn inspiration_flag_active(project_dir: &Path) -> bool {
    let path = project_dir.join(".novelx").join("inspiration_needed.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("active").and_then(|x| x.as_bool()))
        .unwrap_or(false)
}

pub fn set_inspiration_flag(project_dir: &Path, chapter: u32) -> Result<()> {
    let path = project_dir.join(".novelx").join("inspiration_needed.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "active": true,
            "chapter": chapter,
        }))?,
    )?;
    Ok(())
}

pub fn clear_inspiration_flag(project_dir: &Path) -> Result<()> {
    let path = project_dir.join(".novelx").join("inspiration_needed.json");
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct MaterialRuntimeConfig {
    pub enabled: bool,
    pub cooldown_chapters: u32,
    pub max_cards_per_call: u32,
    pub inject_chars: usize,
}

impl Default for MaterialRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cooldown_chapters: 5,
            max_cards_per_call: 3,
            inject_chars: 900,
        }
    }
}

/// Load material_researcher section from `decision_council.yaml`.
pub fn load_material_runtime_config(config_root: &Path) -> MaterialRuntimeConfig {
    let path = config_root.join("decision_council.yaml");
    let Ok(raw) = fs::read_to_string(path) else {
        return MaterialRuntimeConfig::default();
    };
    let Ok(v) = serde_yaml::from_str::<Value>(&raw) else {
        return MaterialRuntimeConfig::default();
    };
    let Some(m) = v.get("material_researcher") else {
        return MaterialRuntimeConfig::default();
    };
    MaterialRuntimeConfig {
        enabled: m
            .get("enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
        cooldown_chapters: m
            .get("cooldown_chapters")
            .and_then(|x| x.as_u64())
            .unwrap_or(5) as u32,
        max_cards_per_call: m
            .get("max_cards_per_call")
            .and_then(|x| x.as_u64())
            .unwrap_or(3) as u32,
        inject_chars: m
            .get("inject_chars")
            .and_then(|x| x.as_u64())
            .unwrap_or(900) as usize,
    }
}

/// Brief text from project meta / artifacts (genre-neutral).
pub fn read_project_brief(project_dir: &Path) -> String {
    if let Ok(state) = crate::project::load_project_state(project_dir) {
        if let Some(b) = state.meta.get("brief").and_then(|v| v.as_str()) {
            if !b.trim().is_empty() {
                return b.to_string();
            }
        }
    }
    for rel in ["artifacts/brief.md", "brief.md"] {
        if let Ok(t) = fs::read_to_string(project_dir.join(rel)) {
            if t.trim().len() > 5 {
                return t;
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_and_format_hooks() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("novelx_mat_{stamp}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let proj = root.join("sample-novel");
        fs::create_dir_all(proj.join(".novelx")).unwrap();
        let cards = vec![MaterialCard {
            id: "mat_demo".into(),
            motif_tags: vec!["folklore".into()],
            sources_style: "folklore".into(),
            hooks: vec!["用地方节庆作冲突由头".into()],
            do_not_canonize: true,
            usable_in: vec!["plot".into()],
            summary: "节庆冲突".into(),
            provenance: "model_grounded".into(),
            created_at_chapter: 3,
        }];
        let ids = save_material_cards(&root, "sample-novel", 3, &cards).unwrap();
        assert_eq!(ids, vec!["mat_demo".to_string()]);
        let block = format_material_hooks_for_context(&proj, 900);
        assert!(block.contains("节庆"));
        assert!(block.contains("非 Canon"));
        assert!(!material_cooldown_allows(&proj, 4, 5));
        assert!(material_cooldown_allows(&proj, 8, 5));
        let _ = fs::remove_dir_all(&root);
    }
}
