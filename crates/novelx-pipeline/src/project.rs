use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Compatible with existing Python `state.json` (extra fields preserved in `extra`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectState {
    pub name: String,
    #[serde(default = "default_genre")]
    pub genre: String,
    #[serde(default = "default_chapters")]
    pub target_chapters: u32,
    #[serde(default)]
    pub published_count: u32,
    #[serde(default = "default_next")]
    pub next_chapter: u32,
    #[serde(default)]
    pub active_agents: Vec<String>,
    #[serde(default)]
    pub meta: HashMap<String, Value>,
    /// Preserve unknown Python fields
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_genre() -> String {
    "未定".into()
}
fn default_chapters() -> u32 {
    100
}
fn default_next() -> u32 {
    1
}

impl ProjectState {
    pub fn new(name: &str, genre: &str, chapters: u32) -> Self {
        Self {
            name: name.to_string(),
            genre: genre.to_string(),
            target_chapters: chapters,
            published_count: 0,
            next_chapter: 1,
            active_agents: novelx_harness::default_pipeline().mvp().to_vec(),
            meta: HashMap::new(),
            extra: HashMap::new(),
        }
    }

    /// Infer next_chapter from chapters array if missing/zero.
    pub fn normalize(&mut self) {
        if self.next_chapter == 0 {
            self.next_chapter = 1;
        }
        if self.published_count == 0 {
            if let Some(chs) = self.extra.get("chapters").and_then(|v| v.as_array()) {
                let max_n = chs
                    .iter()
                    .filter_map(|c| c.get("number").and_then(|n| n.as_u64()))
                    .max()
                    .unwrap_or(0) as u32;
                if max_n > 0 {
                    self.published_count = max_n;
                    self.next_chapter = self.next_chapter.max(max_n + 1);
                }
            }
        }
        if self.active_agents.is_empty() {
            self.active_agents = novelx_harness::default_pipeline().mvp().to_vec();
        }
    }
}

pub fn project_dir(projects_root: &Path, name: &str) -> PathBuf {
    projects_root.join(name)
}

pub fn init_project(
    projects_root: &Path,
    name: &str,
    genre: &str,
    chapters: u32,
) -> Result<PathBuf> {
    let dir = project_dir(projects_root, name);
    fs::create_dir_all(dir.join("chapters"))?;
    fs::create_dir_all(dir.join("entities/characters"))?;
    fs::create_dir_all(dir.join("entities/locations"))?;
    fs::create_dir_all(dir.join("entities/items"))?;
    fs::create_dir_all(dir.join("plots"))?;
    fs::create_dir_all(dir.join("lore"))?;
    fs::create_dir_all(dir.join("artifacts"))?;

    let state = ProjectState::new(name, genre, chapters);
    save_project_state(&dir, &state)?;

    let meta = serde_json::json!({
        "name": name,
        "genre": genre,
        "target_chapters": chapters,
        "brief": "",
        "setup_phase": "collecting",
    });
    fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;

    let bible = format!("# {name}\n\n题材：{genre}\n\n（世界观 Bible 待完善）\n");
    fs::write(dir.join("artifacts/bible.md"), &bible)?;
    Ok(dir)
}

pub fn load_project_state(project_dir: &Path) -> Result<ProjectState> {
    let path = project_dir.join("state.json");
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut state: ProjectState = serde_json::from_str(&text)?;
    state.normalize();
    Ok(state)
}

fn path_nonempty(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|t| t.trim().len() > 20)
        .unwrap_or(false)
}

/// Refresh `meta.json` artifact flags from disk (keeps activation / status honest).
pub fn refresh_meta_flags(project_dir: &Path) -> Result<()> {
    let path = project_dir.join("meta.json");
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|_| "{}".into()))
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let obj = root.as_object_mut().unwrap();
    obj.insert(
        "has_bible".into(),
        Value::Bool(path_nonempty(&project_dir.join("artifacts/bible.md"))),
    );
    obj.insert(
        "has_master_outline".into(),
        Value::Bool(
            path_nonempty(&project_dir.join("artifacts/master_planner.md"))
                || path_nonempty(&project_dir.join("artifacts/story_outline.json")),
        ),
    );
    obj.insert(
        "has_arc_outline".into(),
        Value::Bool(crate::volume::has_any_arc_outline(project_dir)),
    );
    obj.insert(
        "has_nomenclature".into(),
        Value::Bool(
            path_nonempty(&project_dir.join("artifacts/nomenclature.md"))
                || path_nonempty(&project_dir.join("lore/nomenclature.json")),
        ),
    );
    // Keep setup_phase honest without wiping an explicit ready lock.
    let phase = crate::phases::resolve_setup_phase(project_dir);
    obj.insert(
        "setup_phase".into(),
        Value::String(phase.as_str().to_string()),
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

/// Best-effort sync of legacy `records/novel.json` counters (if present).
pub fn sync_records_novel_json(project_dir: &Path, state: &ProjectState) -> Result<()> {
    let path = project_dir.join("records/novel.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    if let Some(obj) = root.as_object_mut() {
        obj.insert("published_count".into(), json_u64(state.published_count));
        obj.insert("next_chapter".into(), json_u64(state.next_chapter));
        obj.insert("latest_chapter".into(), json_u64(state.published_count));
        obj.insert("status".into(), Value::String("active".into()));
        if let Ok(text) = fs::read_to_string(project_dir.join("artifacts/story_outline.json")) {
            if let Ok(outline) = serde_json::from_str::<Value>(&text) {
                if let Some(acts) = outline.get("acts").and_then(|a| a.as_array()) {
                    let current = acts.iter().find(|a| {
                        let start = a.get("start_chapter").and_then(|x| x.as_u64()).unwrap_or(0);
                        let end = a.get("end_chapter").and_then(|x| x.as_u64()).unwrap_or(0);
                        let completed = a.get("completed").and_then(|x| x.as_bool()).unwrap_or(false);
                        let n = state.next_chapter as u64;
                        if start > 0 && n >= start && (end == 0 || n <= end) {
                            return true;
                        }
                        !completed && start > 0 && n >= start
                    });
                    if let Some(act) = current {
                        if let Some(name) = act.get("name").and_then(|x| x.as_str()) {
                            obj.insert("current_volume".into(), Value::String(name.into()));
                        }
                        if let Some(vi) = act.get("volume_index").and_then(|x| x.as_u64()) {
                            obj.insert("current_volume_index".into(), json_u64(vi as u32));
                        }
                    }
                }
            }
        }
    }
    fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

/// Update `target_chapters` in state.json + meta.json when brief/outline implies a new scale.
pub fn update_target_chapters(project_dir: &Path, chapters: u32) -> Result<()> {
    if chapters < 10 {
        return Ok(());
    }
    let mut state = load_project_state(project_dir)?;
    if state.target_chapters == chapters {
        return Ok(());
    }
    state.target_chapters = chapters;
    save_project_state(project_dir, &state)?;
    let meta_path = project_dir.join("meta.json");
    if meta_path.exists() {
        let mut meta: Value = serde_json::from_str(&fs::read_to_string(&meta_path)?)
            .unwrap_or_else(|_| json!({}));
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("target_chapters".into(), json!(chapters));
            fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
        }
    }
    Ok(())
}

pub fn save_project_state(project_dir: &Path, state: &ProjectState) -> Result<()> {
    let path = project_dir.join("state.json");
    // Merge into existing JSON to avoid wiping Python fields when possible
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?)?
    } else {
        Value::Object(serde_json::Map::new())
    };
    if let Some(obj) = root.as_object_mut() {
        obj.insert("name".into(), Value::String(state.name.clone()));
        obj.insert("genre".into(), Value::String(state.genre.clone()));
        obj.insert("target_chapters".into(), json_u64(state.target_chapters));
        obj.insert("published_count".into(), json_u64(state.published_count));
        obj.insert("next_chapter".into(), json_u64(state.next_chapter));
        obj.insert(
            "active_agents".into(),
            Value::Array(
                state
                    .active_agents
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
        let meta_map: serde_json::Map<String, Value> = state
            .meta
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        obj.insert("meta".into(), Value::Object(meta_map));
    }
    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

fn json_u64(n: u32) -> Value {
    Value::Number(n.into())
}

pub fn chapter_dir(project_dir: &Path, chapter: u32) -> PathBuf {
    project_dir.join("chapters").join(format!("{chapter:03}"))
}

pub fn read_chapter_draft(project_dir: &Path, chapter: u32) -> Option<String> {
    let path = chapter_dir(project_dir, chapter).join("draft.md");
    fs::read_to_string(path).ok()
}

pub fn write_chapter_draft(project_dir: &Path, chapter: u32, draft: &str) -> Result<()> {
    let dir = chapter_dir(project_dir, chapter);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("draft.md"), draft)?;
    Ok(())
}

/// Read chapter outline as canonical JSON text (pretty). Migrates legacy `outline.md` once.
pub fn read_chapter_outline(project_dir: &Path, chapter: u32) -> Option<String> {
    let dir = chapter_dir(project_dir, chapter);
    let json_path = dir.join("outline.json");
    if let Ok(text) = fs::read_to_string(&json_path) {
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    let md_path = dir.join("outline.md");
    let md = fs::read_to_string(&md_path).ok()?;
    if md.trim().is_empty() {
        return None;
    }
    match crate::schemas::parse_chapter_outline_text(&md) {
        Ok(o) => {
            let pretty = serde_json::to_string_pretty(&o).ok()?;
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(&json_path, &pretty);
            Some(pretty)
        }
        Err(_) => {
            // Unparseable legacy: still return raw so preview can show something.
            Some(md)
        }
    }
}

/// Validate and write `outline.json` (JSON object or fenced JSON text).
pub fn write_chapter_outline(project_dir: &Path, chapter: u32, outline: &str) -> Result<()> {
    let parsed = crate::schemas::parse_chapter_outline_text(outline)?;
    let dir = chapter_dir(project_dir, chapter);
    fs::create_dir_all(&dir)?;
    let pretty = serde_json::to_string_pretty(&parsed)?;
    fs::write(dir.join("outline.json"), pretty)?;
    Ok(())
}

pub fn list_projects(projects_root: &Path) -> Result<Vec<String>> {
    if !projects_root.exists() {
        return Ok(vec![]);
    }
    let mut names = Vec::new();
    for e in fs::read_dir(projects_root)? {
        let e = e?;
        if e.path().is_dir() {
            if let Some(n) = e.file_name().to_str() {
                if !n.starts_with('.') {
                    names.push(n.to_string());
                }
            }
        }
    }
    names.sort();
    Ok(names)
}

/// List chapter numbers that have a folder under `chapters/`.
pub fn list_chapter_numbers(project_dir: &Path) -> Vec<u32> {
    let root = project_dir.join("chapters");
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(&root) else {
        return out;
    };
    for e in rd.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        if let Some(n) = e
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        {
            out.push(n);
        }
    }
    out.sort_unstable();
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteChapterResult {
    pub deleted: u32,
    pub remaining: Vec<u32>,
    pub published_count: u32,
    pub next_chapter: u32,
}

/// Delete one chapter folder and sync `state.json` counters (+ legacy `chapters` array).
pub fn delete_chapter(project_dir: &Path, chapter: u32) -> Result<DeleteChapterResult> {
    if chapter == 0 {
        anyhow::bail!("无效章号");
    }
    let dir = chapter_dir(project_dir, chapter);
    if !dir.exists() {
        anyhow::bail!("第{chapter}章不存在（无 chapters/{chapter:03}/）");
    }
    fs::remove_dir_all(&dir)
        .with_context(|| format!("删除 {}", dir.display()))?;

    let remaining = list_chapter_numbers(project_dir);
    let max = remaining.iter().copied().max().unwrap_or(0);
    let mut state = load_project_state(project_dir)?;
    state.published_count = max;
    state.next_chapter = max.saturating_add(1).max(1);
    save_project_state(project_dir, &state)?;
    prune_legacy_chapters_array(project_dir, chapter)?;
    let _ = prune_memory_chapter_refs(project_dir, chapter);

    Ok(DeleteChapterResult {
        deleted: chapter,
        remaining,
        published_count: state.published_count,
        next_chapter: state.next_chapter,
    })
}

fn prune_legacy_chapters_array(project_dir: &Path, chapter: u32) -> Result<()> {
    let path = project_dir.join("state.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(());
    };
    if let Some(arr) = obj.get_mut("chapters").and_then(|v| v.as_array_mut()) {
        arr.retain(|c| {
            c.get("number")
                .and_then(|n| n.as_u64())
                .map(|n| n as u32 != chapter)
                .unwrap_or(true)
        });
    }
    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

fn prune_memory_chapter_refs(project_dir: &Path, chapter: u32) -> Result<()> {
    let path = project_dir.join("lore/memory.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(());
    };
    for key in ["digests", "chapter_digests", "summaries"] {
        if let Some(arr) = obj.get_mut(key).and_then(|v| v.as_array_mut()) {
            arr.retain(|c| {
                c.get("chapter")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as u32 != chapter)
                    .unwrap_or(true)
            });
        }
    }
    fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_chapter_removes_dir_and_updates_counts() {
        let proj = std::env::temp_dir().join(format!(
            "novelx-del-ch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&proj);
        fs::create_dir_all(proj.join("chapters/001")).unwrap();
        fs::create_dir_all(proj.join("chapters/002")).unwrap();
        fs::write(proj.join("chapters/001/draft.md"), "# 1\n").unwrap();
        fs::write(proj.join("chapters/002/draft.md"), "# 2\n").unwrap();
        let state = ProjectState {
            name: "book".into(),
            genre: "未定".into(),
            target_chapters: 100,
            published_count: 2,
            next_chapter: 3,
            active_agents: vec![],
            meta: HashMap::new(),
            extra: HashMap::new(),
        };
        save_project_state(&proj, &state).unwrap();
        let mut raw: Value =
            serde_json::from_str(&fs::read_to_string(proj.join("state.json")).unwrap()).unwrap();
        raw.as_object_mut().unwrap().insert(
            "chapters".into(),
            serde_json::json!([{"number": 1}, {"number": 2}]),
        );
        fs::write(
            proj.join("state.json"),
            serde_json::to_string_pretty(&raw).unwrap(),
        )
        .unwrap();

        let r = delete_chapter(&proj, 2).unwrap();
        assert_eq!(r.deleted, 2);
        assert_eq!(r.remaining, vec![1]);
        assert_eq!(r.published_count, 1);
        assert_eq!(r.next_chapter, 2);
        assert!(!proj.join("chapters/002").exists());
        assert!(proj.join("chapters/001").exists());
        let raw: Value =
            serde_json::from_str(&fs::read_to_string(proj.join("state.json")).unwrap()).unwrap();
        assert_eq!(raw["chapters"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&proj);
    }

    #[test]
    fn update_target_chapters_writes_state_and_meta() {
        let root = std::env::temp_dir().join(format!("nx_tgt_{}", uuid_like()));
        let _ = fs::remove_dir_all(&root);
        let proj = init_project(&root, "demo", "悬疑", 120).unwrap();
        update_target_chapters(&proj, 900).unwrap();
        let state = load_project_state(&proj).unwrap();
        assert_eq!(state.target_chapters, 900);
        let meta: Value =
            serde_json::from_str(&fs::read_to_string(proj.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["target_chapters"], 900);
        let _ = fs::remove_dir_all(&root);
    }

    fn uuid_like() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
    }
}
