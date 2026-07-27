//! NovelX app-server — axum HTTP + WebSocket (Codex app-server inspired).

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Json};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use novelx_core::NovelxCore;
use novelx_harness::{ContentRulesConfig, NamingRules, StudioPolicies};
use novelx_llm::{load_llm_config, LlmClient};
use novelx_skills::SkillScope;
use novelx_pipeline::cards::load_markdown_cards;
use novelx_pipeline::plots::{load_plot_cards_by_progress, rebuild_plot_index};
use novelx_pipeline::project::{
    delete_chapter, list_projects, load_project_state, project_dir, read_chapter_draft,
    read_chapter_outline, write_chapter_draft, write_chapter_outline,
};
use novelx_pipeline::{
    format_body_state_board, format_body_state_board_for_character, resolve_setup_phase,
    resolve_volume_phase,
};
use novelx_pipeline::schemas::{
    display_bible, display_chapter_outline, display_draft, display_entity_card,
    display_entity_gaps, display_master_outline, display_plot_card_body, parse_chapter_outline_text,
    validate_arc_outline, validate_bible, validate_draft, validate_entity_body_edit,
    validate_master_outline, validate_plot_card_body_edit, EntityKind,
};
use novelx_protocol::{EventMsg, Op};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub core: Arc<NovelxCore>,
    pub repo_root: PathBuf,
    pub event_bus: broadcast::Sender<EventMsg>,
}

#[derive(Debug, Deserialize)]
pub struct StartThreadReq {
    pub project: Option<String>,
    /// When true, discard persisted dialogue and open a fresh thread.
    #[serde(default)]
    pub fresh: bool,
}

#[derive(Debug, Deserialize)]
pub struct StartTurnReq {
    pub thread_id: String,
    pub text: String,
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SteerReq {
    pub thread_id: String,
    pub turn_id: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct SkillsListResp {
    pub skills: Vec<SkillDto>,
}

#[derive(Debug, Serialize)]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: String,
}

pub async fn serve(repo_root: PathBuf, addr: SocketAddr) -> Result<()> {
    let config_root = repo_root.join("config");
    let projects_root = repo_root.join("projects");
    let llm_cfg = load_llm_config(&config_root.join("llm.yaml")).unwrap_or_default();
    let llm = Arc::new(LlmClient::new(llm_cfg));
    let core = NovelxCore::new(projects_root, config_root, llm);
    // Mirror core event bus → app bus so WS sees SubAgent status too.
    // Large enough that audit heartbeats + completion events survive lag.
    let (event_bus, _) = broadcast::channel(8192);
    {
        let mut rx = core.subscribe_events();
        let bus = event_bus.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let _ = bus.send(ev);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    let state = AppState {
        core,
        repo_root: repo_root.clone(),
        event_bus,
    };

    let web_dist = repo_root.join("web/dist");
    let index_html = web_dist.join("index.html");
    let has_ui = index_html.is_file();

    let api = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/thread/start", post(thread_start))
        .route("/api/thread/new", post(thread_new))
        .route("/api/thread/resume", post(thread_resume))
        .route("/api/thread/turns", post(thread_save_turns))
        .route("/api/turn/start", post(turn_start))
        .route("/api/turn/interrupt", post(turn_interrupt))
        .route("/api/turn/steer", post(turn_steer))
        .route("/api/skills/list", get(skills_list))
        .route("/api/skills/{name}", get(skill_get).put(skill_put))
        .route(
            "/api/config/content_rules",
            get(content_rules_get).put(content_rules_put),
        )
        .route(
            "/api/config/naming_rules",
            get(naming_rules_get).put(naming_rules_put),
        )
        .route(
            "/api/config/policies",
            get(policies_get).put(policies_put),
        )
        .route("/api/library", get(library))
        .route("/api/library/{name}", get(library_one).delete(library_delete))
        .route("/api/projects/{name}/preview", get(preview))
        .route("/api/projects/{name}/content", put(project_content_put))
        .route(
            "/api/projects/{name}/chapters/{chapter}",
            delete(chapter_delete),
        )
        .route("/ws/thread/{id}", get(ws_thread))
        .with_state(state);

    let app = if has_ui {
        info!("serving web UI from {}", web_dist.display());
        api.fallback_service(
            ServeDir::new(web_dist).not_found_service(ServeFile::new(index_html)),
        )
        .layer(CorsLayer::permissive())
    } else {
        api.route("/", get(index_fallback))
            .layer(CorsLayer::permissive())
    };

    info!("NovelX listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index_fallback() -> Html<&'static str> {
    Html("<!doctype html><meta charset=utf-8><title>NovelX</title><p>NovelX API is running. Build the web UI with <code>cd web && npm run build</code> or use <code>npm run dev</code>.</p>")
}

async fn thread_start(
    State(state): State<AppState>,
    Json(req): Json<StartThreadReq>,
) -> impl IntoResponse {
    match state.core.spawn_thread(req.project, req.fresh).await {
        Ok((thread_id, project)) => {
            let snap = state.core.thread_snapshot(&thread_id).await.unwrap_or_else(|| {
                serde_json::json!({
                    "threadId": thread_id,
                    "project": project,
                    "messages": [],
                    "turns": [],
                })
            });
            Json(snap)
        }
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

/// Explicit "new task / clear chat": always opens a fresh empty thread.
async fn thread_new(
    State(state): State<AppState>,
    Json(req): Json<StartThreadReq>,
) -> impl IntoResponse {
    match state.core.spawn_thread(req.project, true).await {
        Ok((thread_id, project)) => Json(serde_json::json!({
            "threadId": thread_id,
            "project": project,
            "messages": [],
            "turns": [],
            "ok": true,
        })),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
struct SaveTurnsReq {
    thread_id: String,
    turns: serde_json::Value,
}

async fn thread_save_turns(
    State(state): State<AppState>,
    Json(req): Json<SaveTurnsReq>,
) -> impl IntoResponse {
    match state.core.save_ui_turns(&req.thread_id, req.turns).await {
        Ok(()) => Json(serde_json::json!({"ok": true})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    }
}

#[derive(Debug, Deserialize)]
struct ResumeReq {
    thread_id: String,
}

async fn thread_resume(
    State(state): State<AppState>,
    Json(req): Json<ResumeReq>,
) -> impl IntoResponse {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let core = state.core.clone();
    tokio::spawn(async move {
        let _ = core
            .handle_op(
                Op::ResumeThread {
                    thread_id: req.thread_id,
                },
                tx,
            )
            .await;
    });
    let mut out = serde_json::json!({});
    while let Some(ev) = rx.recv().await {
        let _ = state.event_bus.send(ev.clone());
        if let EventMsg::SessionConfigured {
            thread_id,
            project,
            ..
        } = ev
        {
            out = serde_json::json!({"threadId": thread_id, "project": project});
        }
    }
    Json(out)
}

async fn turn_start(
    State(state): State<AppState>,
    Json(req): Json<StartTurnReq>,
) -> impl IntoResponse {
    let thread_id = req.thread_id.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let bus = state.event_bus.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = bus.send(ev);
        }
    });
    let core = state.core.clone();
    tokio::spawn(async move {
        let _ = core
            .handle_op(
                Op::StartTurn {
                    thread_id: req.thread_id,
                    text: req.text,
                    skills: req.skills,
                },
                tx,
            )
            .await;
    });
    Json(serde_json::json!({"ok": true, "threadId": thread_id}))
}

#[derive(Debug, Deserialize)]
struct InterruptReq {
    thread_id: String,
    turn_id: String,
}

async fn turn_interrupt(
    State(state): State<AppState>,
    Json(req): Json<InterruptReq>,
) -> impl IntoResponse {
    let (tx, _rx) = mpsc::unbounded_channel();
    let _ = state
        .core
        .handle_op(
            Op::InterruptTurn {
                thread_id: req.thread_id,
                turn_id: Some(req.turn_id),
            },
            tx,
        )
        .await;
    Json(serde_json::json!({"ok": true}))
}

async fn turn_steer(State(state): State<AppState>, Json(req): Json<SteerReq>) -> impl IntoResponse {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let bus = state.event_bus.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = bus.send(ev);
        }
    });
    let core = state.core.clone();
    tokio::spawn(async move {
        let _ = core
            .handle_op(
                Op::SteerTurn {
                    thread_id: req.thread_id,
                    turn_id: Some(req.turn_id),
                    text: req.text,
                },
                tx,
            )
            .await;
    });
    Json(serde_json::json!({"ok": true}))
}

async fn skills_list(State(state): State<AppState>) -> impl IntoResponse {
    state.core.reload_skills().await;
    let skills = state.core.list_skills().await;
    Json(SkillsListResp {
        skills: skills
            .into_iter()
            .map(|s| SkillDto {
                name: s.name,
                description: s.description,
                path: s.path.display().to_string(),
                scope: format!("{:?}", s.scope),
            })
            .collect(),
    })
}

#[derive(Debug, Deserialize)]
struct YamlPutReq {
    content: String,
}

#[derive(Debug, Deserialize)]
struct SkillPutReq {
    content: String,
}

/// Resolve a file under `config/` (no `..` / absolute escape).
fn resolve_config_file(config_root: &std::path::Path, relative: &str) -> Result<PathBuf, String> {
    let rel = relative.trim().trim_start_matches('/');
    if rel.is_empty() || rel.contains('\0') {
        return Err("非法配置路径".into());
    }
    if rel.split(['/', '\\']).any(|p| p == ".." || p.is_empty()) {
        return Err("拒绝路径穿越".into());
    }
    let root = config_root
        .canonicalize()
        .unwrap_or_else(|_| config_root.to_path_buf());
    let candidate = root.join(rel);
    // Allow not-yet-existing files: canonicalize parent + append file name.
    let parent = candidate.parent().unwrap_or(&root);
    let parent_canon = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    if !parent_canon.starts_with(&root) {
        return Err("拒绝路径穿越".into());
    }
    let file_name = candidate
        .file_name()
        .ok_or_else(|| "非法配置路径".to_string())?;
    let resolved = parent_canon.join(file_name);
    if !resolved.starts_with(&root) {
        return Err("拒绝路径穿越".into());
    }
    Ok(resolved)
}

fn read_config_text(config_root: &std::path::Path, relative: &str) -> Result<String, String> {
    let path = resolve_config_file(config_root, relative)?;
    std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))
}

fn write_config_text(config_root: &std::path::Path, relative: &str, content: &str) -> Result<(), String> {
    let path = resolve_config_file(config_root, relative)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("写入失败：{e}"))
}

async fn content_rules_get(State(state): State<AppState>) -> impl IntoResponse {
    let cfg_root = state.core.config_root();
    let cfg = ContentRulesConfig::load_from_config_root(cfg_root);
    let yaml = if cfg.raw_yaml.trim().is_empty() {
        read_config_text(cfg_root, "content_rules.yaml").unwrap_or_else(|_| cfg.raw_yaml.clone())
    } else {
        cfg.raw_yaml.clone()
    };
    Json(serde_json::json!({
        "ok": true,
        "path": "config/content_rules.yaml",
        "yaml": yaml,
        "rules": cfg.catalog(),
    }))
}

async fn content_rules_put(
    State(state): State<AppState>,
    Json(req): Json<YamlPutReq>,
) -> impl IntoResponse {
    match ContentRulesConfig::parse_yaml(&req.content) {
        Ok(cfg) => match write_config_text(state.core.config_root(), "content_rules.yaml", &req.content)
        {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "rules": cfg.catalog(),
                })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok": false, "error": e})),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": e})),
        )
            .into_response(),
    }
}

async fn naming_rules_get(State(state): State<AppState>) -> impl IntoResponse {
    let cfg_root = state.core.config_root();
    let yaml = read_config_text(cfg_root, "naming_rules.yaml").unwrap_or_default();
    let rules = NamingRules::load_from_config_root(cfg_root);
    Json(serde_json::json!({
        "ok": true,
        "path": "config/naming_rules.yaml",
        "yaml": yaml,
        "forbidden_names": rules.forbidden_names,
        "naming_principles": rules.naming_principles,
        "categories": rules.categories,
    }))
}

async fn naming_rules_put(
    State(state): State<AppState>,
    Json(req): Json<YamlPutReq>,
) -> impl IntoResponse {
    match NamingRules::parse_yaml(&req.content) {
        Ok(rules) => {
            match write_config_text(state.core.config_root(), "naming_rules.yaml", &req.content) {
                Ok(()) => (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "ok": true,
                        "forbidden_names": rules.forbidden_names,
                        "naming_principles": rules.naming_principles,
                    })),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"ok": false, "error": e})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": e})),
        )
            .into_response(),
    }
}

async fn policies_get(State(state): State<AppState>) -> impl IntoResponse {
    let yaml = read_config_text(state.core.config_root(), "policies.yaml").unwrap_or_default();
    Json(serde_json::json!({
        "ok": true,
        "path": "config/policies.yaml",
        "yaml": yaml,
    }))
}

async fn policies_put(
    State(state): State<AppState>,
    Json(req): Json<YamlPutReq>,
) -> impl IntoResponse {
    match StudioPolicies::parse_yaml(&req.content) {
        Ok(_) => match write_config_text(state.core.config_root(), "policies.yaml", &req.content) {
            Ok(()) => {
                state.core.reload_policies();
                (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
            }
            Err(e) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok": false, "error": e})),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": e})),
        )
            .into_response(),
    }
}

fn is_framework_skill_path(config_root: &std::path::Path, path: &std::path::Path) -> bool {
    let Ok(skills_root) = config_root.join("skills").canonicalize() else {
        let skills_root = config_root.join("skills");
        return path.starts_with(&skills_root);
    };
    let Ok(canon) = path.canonicalize() else {
        return path.starts_with(&skills_root);
    };
    canon.starts_with(&skills_root)
}

async fn skill_get(State(state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    state.core.reload_skills().await;
    let skills = state.core.list_skills().await;
    let Some(meta) = skills.iter().find(|s| s.name == name || s.name.replace('_', "-") == name.replace('_', "-"))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"ok": false, "error": format!("未找到 skill「{name}」")})),
        )
            .into_response();
    };
    let content = match std::fs::read_to_string(&meta.path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok": false, "error": format!("读取失败：{e}")})),
            )
                .into_response();
        }
    };
    let editable = matches!(meta.scope, SkillScope::System | SkillScope::Agent | SkillScope::Studio)
        && is_framework_skill_path(state.core.config_root(), &meta.path);
    Json(serde_json::json!({
        "ok": true,
        "name": meta.name,
        "description": meta.description,
        "path": meta.path.display().to_string(),
        "scope": format!("{:?}", meta.scope),
        "editable": editable,
        "content": content,
    }))
    .into_response()
}

async fn skill_put(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<SkillPutReq>,
) -> impl IntoResponse {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": "非法 skill 名称"})),
        )
            .into_response();
    }
    state.core.reload_skills().await;
    let skills = state.core.list_skills().await;
    let Some(meta) = skills.iter().find(|s| s.name == name || s.name.replace('_', "-") == name.replace('_', "-"))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"ok": false, "error": format!("未找到 skill「{name}」")})),
        )
            .into_response();
    };
    if matches!(meta.scope, SkillScope::Project)
        || !is_framework_skill_path(state.core.config_root(), &meta.path)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "ok": false,
                "error": "一期仅允许编辑 config/skills 下的框架 Skill，不能写 projects/ 项目 Skill",
            })),
        )
            .into_response();
    }
    // Ensure write target stays under config/skills even if path was swapped.
    let skills_root = state.core.config_root().join("skills");
    let Ok(skills_canon) = skills_root.canonicalize() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": "config/skills 不存在"})),
        )
            .into_response();
    };
    let target = meta.path.clone();
    let parent = target.parent().unwrap_or(&skills_canon);
    let parent_canon = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    if !parent_canon.starts_with(&skills_canon) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"ok": false, "error": "拒绝写入 config/skills 以外的路径"})),
        )
            .into_response();
    }
    if let Err(e) = std::fs::write(&target, &req.content) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": format!("写入失败：{e}")})),
        )
            .into_response();
    }
    state.core.reload_skills().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "name": meta.name,
            "path": target.display().to_string(),
        })),
    )
        .into_response()
}

fn build_preview(repo_root: &std::path::Path, name: &str) -> serde_json::Value {
    let dir = repo_root.join("projects").join(name);
    let st = load_project_state(&dir).ok();
    let published = st.as_ref().map(|s| s.published_count).unwrap_or(0);
    let next = st.as_ref().map(|s| s.next_chapter).unwrap_or(1);
    let mut chapters = Vec::new();
    let chapters_dir = dir.join("chapters");
    if chapters_dir.exists() {
        if let Ok(rd) = std::fs::read_dir(&chapters_dir) {
            let mut nums: Vec<u32> = rd
                .flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|s| s.parse::<u32>().ok())
                })
                .collect();
            nums.sort_unstable();
            for n in nums {
                let draft_raw = read_chapter_draft(&dir, n).unwrap_or_default();
                let draft = if draft_raw.trim().is_empty() {
                    String::new()
                } else {
                    display_draft(&draft_raw)
                };
                let outline_raw = read_chapter_outline(&dir, n).unwrap_or_default();
                let outline = match parse_chapter_outline_text(&outline_raw) {
                    Ok(o) => display_chapter_outline(&o),
                    Err(_) => outline_raw,
                };
                chapters.push(serde_json::json!({
                    "number": n,
                    "title": format!("第{n}章"),
                    "draft": draft,
                    "outline": outline,
                }));
            }
        }
    }

    let mut entities = serde_json::json!({
        "characters": [],
        "items": [],
        "locations": [],
    });
    // Character name keys for attributing body_state facts per card.
    let char_cards = load_markdown_cards(&dir.join("entities").join("characters"), "characters");
    let all_char_keys: Vec<String> = {
        let mut keys = Vec::new();
        for c in &char_cards {
            keys.extend(entity_match_keys(c));
        }
        keys.sort();
        keys.dedup();
        keys
    };
    let default_owner_names = load_default_body_owner_names(&dir);
    for group in ["characters", "items", "locations"] {
        let kind = EntityKind::from_group(group).unwrap_or(EntityKind::Character);
        let cards = if group == "characters" {
            char_cards.clone()
        } else {
            load_markdown_cards(&dir.join("entities").join(group), group)
        };
        let list: Vec<serde_json::Value> = cards
            .into_iter()
            .map(|mut c| {
                // Prefer full-file display when available; cards store body in markdown.
                let path = dir.join("entities").join(group).join(format!("{}.md", c.slug));
                let full = std::fs::read_to_string(&path).unwrap_or_else(|_| c.markdown.clone());
                c.markdown = display_entity_card(kind, &full);
                let mut v = c.to_preview_json();
                if group == "characters" {
                    let self_keys = entity_match_keys(&c);
                    let is_default = self_keys.iter().any(|k| {
                        default_owner_names
                            .iter()
                            .any(|d| d == k || k.contains(d) || d.contains(k))
                    }) || c
                        .meta
                        .get("role")
                        .map(|r| {
                            matches!(
                                r.trim().to_ascii_lowercase().as_str(),
                                "protagonist" | "main" | "lead" | "主角"
                            )
                        })
                        .unwrap_or(false);
                    let board = format_body_state_board_for_character(
                        &dir,
                        next.max(1),
                        &self_keys,
                        &all_char_keys,
                        is_default,
                    );
                    if !board.trim().is_empty() {
                        v["body_state"] = serde_json::Value::String(board);
                    }
                }
                v
            })
            .collect();
        entities[group] = serde_json::Value::Array(list);
    }
    let plots: Vec<serde_json::Value> = load_plot_cards_by_progress(&dir)
        .into_iter()
        .map(|mut c| {
            let path = dir.join("plots").join(format!("{}.md", c.slug));
            let full = std::fs::read_to_string(&path).unwrap_or_else(|_| c.markdown.clone());
            c.markdown = display_plot_card_body(&full);
            c.to_preview_json()
        })
        .collect();

    let mut artifacts = serde_json::Map::new();
    let art_dir = dir.join("artifacts");
    if art_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&art_dir) {
            for ent in rd.flatten() {
                let path = ent.path();
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Per-volume 卷纲 live in artifacts/arc_outlines/ — not as a single flat file.
                if stem == "arc_outline" || stem == "arc_planner" {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if ext == "md" || ext == "txt" {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        let display = match stem {
                            "master_outline" | "master_planner" => display_master_outline(&text),
                            "bible" => display_bible(&text),
                            _ => text,
                        };
                        artifacts.insert(stem.to_string(), serde_json::Value::String(display));
                    }
                }
            }
        }
    }
    let arc_outlines = novelx_pipeline::list_arc_outlines_for_preview(&dir);

    let story_outline = std::fs::read_to_string(dir.join("artifacts/story_outline.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .unwrap_or_else(|| {
            let md = artifacts
                .get("master_planner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            serde_json::json!({ "markdown": md, "acts": [] })
        });

    let entity_gaps_list = novelx_pipeline::collect_entity_gaps(&dir);
    let entity_gaps_display = display_entity_gaps(&entity_gaps_list);
    // Same chapter index the next writer run would lock against.
    let body_state_board = format_body_state_board(&dir, next.max(1));
    // Progress authority: published_count + on-disk chapters — ignore stale state.extra.chapters.
    let mut state_json = st
        .as_ref()
        .and_then(|s| serde_json::to_value(s).ok())
        .unwrap_or(serde_json::json!({}));
    if let Some(obj) = state_json.as_object_mut() {
        obj.remove("chapters");
        obj.insert("published_count".into(), serde_json::json!(published));
        obj.insert("next_chapter".into(), serde_json::json!(next));
    }

    let setup_phase = resolve_setup_phase(&dir);
    let volume_phase = resolve_volume_phase(&dir);
    let has_arc = !arc_outlines.is_empty() || novelx_pipeline::has_any_arc_outline(&dir);
    let has_master = artifacts
        .get("master_outline")
        .or_else(|| artifacts.get("master_planner"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().len() > 20)
        .unwrap_or(false)
        || story_outline
            .get("markdown")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().len() > 20)
            .unwrap_or(false);

    serde_json::json!({
        "project": name,
        "name": name,
        "chapters": chapters,
        "story_outline": story_outline,
        "plots": plots,
        "entities": entities,
        "artifacts": artifacts,
        "arc_outlines": arc_outlines,
        "entity_gaps": entity_gaps_list,
        "entity_gaps_display": entity_gaps_display,
        "body_state_board": body_state_board,
        "published_count": published,
        "next_chapter": next,
        "state": state_json,
        "setup_phase": setup_phase.as_str(),
        "volume_phase": volume_phase.as_str(),
        "has_master_outline": has_master,
        "has_arc_outline": has_arc,
    })
}

/// Names / aliases used to attribute body_state facts to a character card.
fn entity_match_keys(card: &novelx_pipeline::cards::MarkdownCard) -> Vec<String> {
    let mut keys = card.match_keys();
    // 「老七(预言者)」also matches facts that only say「老七」.
    for base in [card.name.as_str(), card.title.as_str(), card.slug.as_str()] {
        for sep in ['(', '（', '·', '/'] {
            if let Some((head, _)) = base.split_once(sep) {
                let t = head.trim();
                if t.chars().count() >= 2 {
                    keys.push(t.to_string());
                }
            }
        }
    }
    keys.retain(|k| !k.trim().is_empty());
    keys.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
    keys.dedup();
    keys
}

/// Always-include / protagonist names — receive unscoped body_state facts.
fn load_default_body_owner_names(project_dir: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();
    let push = |names: &mut Vec<String>, v: &serde_json::Value| {
        if let Some(s) = v.as_str() {
            let t = s.trim();
            if !t.is_empty() {
                names.push(t.to_string());
            }
        } else if let Some(arr) = v.as_array() {
            for x in arr {
                if let Some(s) = x.as_str() {
                    let t = s.trim();
                    if !t.is_empty() {
                        names.push(t.to_string());
                    }
                }
            }
        }
    };
    if let Ok(raw) = std::fs::read_to_string(project_dir.join("meta.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(p) = v.get("protagonist") {
                push(&mut names, p);
            }
            if let Some(p) = v.get("protagonists") {
                push(&mut names, p);
            }
        }
    }
    if let Ok(st) = load_project_state(project_dir) {
        if let Some(p) = st.meta.get("protagonist") {
            push(&mut names, p);
        }
        if let Some(p) = st.meta.get("protagonists") {
            push(&mut names, p);
        }
    }
    names.sort();
    names.dedup();
    names
}

async fn library(State(state): State<AppState>) -> impl IntoResponse {
    let names = list_projects(&state.repo_root.join("projects")).unwrap_or_default();
    let mut items = Vec::new();
    for name in names {
        let dir = state.repo_root.join("projects").join(&name);
        let st = load_project_state(&dir).ok();
        items.push(serde_json::json!({
            "id": name,
            "name": name,
            "display_title": st.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| name.clone()),
            "title": st.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| name.clone()),
            "genre": st.as_ref().map(|s| s.genre.clone()).unwrap_or_default(),
            "next_chapter": st.as_ref().map(|s| s.next_chapter).unwrap_or(1),
            "published_count": st.as_ref().map(|s| s.published_count).unwrap_or(0),
            "status": "active",
        }));
    }
    Json(serde_json::json!({"novels": items}))
}

async fn library_one(State(state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    let preview = build_preview(&state.repo_root, &name);
    let st = preview.get("state").cloned().unwrap_or(serde_json::Value::Null);
    Json(serde_json::json!({
        "novel": {
            "id": name,
            "display_title": st.get("name").and_then(|v| v.as_str()).unwrap_or(&name),
            "genre": st.get("genre").and_then(|v| v.as_str()).unwrap_or(""),
            "published_count": preview.get("published_count"),
            "next_chapter": preview.get("next_chapter"),
            "status": "active",
        },
        "preview": preview,
    }))
}

async fn preview(State(state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    Json(build_preview(&state.repo_root, &name))
}

#[derive(Debug, Deserialize)]
struct ContentPutReq {
    tab: String,
    content: String,
    #[serde(default)]
    chapter: u32,
    #[serde(default)]
    card_key: String,
}

async fn project_content_put(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<ContentPutReq>,
) -> impl IntoResponse {
    let dir = state.repo_root.join("projects").join(&name);
    if !dir.is_dir() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"ok": false, "error": format!("项目「{name}」不存在")})),
        )
            .into_response();
    }
    match save_project_content(&dir, &req) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "preview": build_preview(&state.repo_root, &name),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

fn save_project_content(dir: &std::path::Path, req: &ContentPutReq) -> anyhow::Result<()> {
    let tab = req.tab.trim();
    let content = &req.content;
    match tab {
        "draft" => {
            if req.chapter == 0 {
                anyhow::bail!("保存正文需要有效章号");
            }
            let normalized = validate_draft(req.chapter, content)?;
            write_chapter_draft(dir, req.chapter, &normalized)?;
        }
        "outline" => {
            if req.chapter == 0 {
                anyhow::bail!("保存章纲需要有效章号");
            }
            // write_chapter_outline hard-validates JSON → outline.json
            write_chapter_outline(dir, req.chapter, content)?;
        }
        "master" => {
            let normalized = validate_master_outline(content)?;
            write_artifact_md(dir, "master_outline", &normalized)?;
            let outline_path = dir.join("artifacts/story_outline.json");
            let mut v = std::fs::read_to_string(&outline_path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .unwrap_or_else(|| serde_json::json!({"acts": []}));
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "markdown".into(),
                    serde_json::Value::String(normalized.clone()),
                );
                obj.entry("acts").or_insert_with(|| serde_json::json!([]));
            }
            std::fs::create_dir_all(dir.join("artifacts"))?;
            std::fs::write(&outline_path, serde_json::to_string_pretty(&v)?)?;
        }
        "plots" => {
            let key = req.card_key.trim();
            if key.is_empty() {
                anyhow::bail!("保存剧情卡需要 card_key");
            }
            let path = find_card_path(&dir.join("plots"), key)
                .ok_or_else(|| anyhow::anyhow!("未找到剧情卡「{key}」"))?;
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            let norm_body = validate_plot_card_body_edit(&existing, content)?;
            write_card_preserving_frontmatter(&path, &norm_body)?;
            let _ = rebuild_plot_index(dir);
        }
        "arcs" => {
            let key = req.card_key.trim();
            if key.is_empty() {
                anyhow::bail!("保存卷纲需要 card_key（如 v1）");
            }
            let vol_str = key.strip_prefix('v').unwrap_or(key);
            let vol: u32 = vol_str
                .parse()
                .map_err(|_| anyhow::anyhow!("卷纲 card_key 须为 vN 或卷第数字"))?;
            if vol < 1 {
                anyhow::bail!("卷纲卷第须 ≥ 1");
            }
            let normalized = validate_arc_outline(content)?;
            novelx_pipeline::write_arc_outline_text(dir, vol, &normalized)?;
        }
        t if t.starts_with("ent:") => {
            let group = &t[4..];
            let kind = EntityKind::from_group(group)
                .ok_or_else(|| anyhow::anyhow!("未知设定卡分组：{group}"))?;
            let key = req.card_key.trim();
            if key.is_empty() {
                anyhow::bail!("保存设定卡需要 card_key");
            }
            let path = find_card_path(&dir.join("entities").join(group), key)
                .ok_or_else(|| anyhow::anyhow!("未找到设定卡「{key}」"))?;
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            let norm_body = validate_entity_body_edit(kind, &existing, content)?;
            write_card_preserving_frontmatter(&path, &norm_body)?;
        }
        t if t.starts_with("art:arc_v") => {
            let rest = &t["art:arc_v".len()..];
            let vol: u32 = rest
                .parse()
                .map_err(|_| anyhow::anyhow!("卷纲标签须为 art:arc_vN（N 为卷第）"))?;
            if vol < 1 {
                anyhow::bail!("卷纲卷第须 ≥ 1");
            }
            let normalized = validate_arc_outline(content)?;
            novelx_pipeline::write_arc_outline_text(dir, vol, &normalized)?;
        }
        t if t.starts_with("art:") => {
            let stem = &t[4..];
            if stem.is_empty() || stem.contains('/') || stem.contains("..") {
                anyhow::bail!("非法 artifact 名");
            }
            let normalized = match stem {
                "master_outline" | "master_planner" => validate_master_outline(content)?,
                "arc_outline" | "arc_planner" => {
                    let normalized = validate_arc_outline(content)?;
                    let vol = novelx_pipeline::resolve_arc_outline_volume(dir, None);
                    novelx_pipeline::write_arc_outline_text(dir, vol, &normalized)?;
                    return Ok(());
                }
                "bible" => validate_bible(content)?,
                _ => content.clone(),
            };
            write_artifact_md(dir, stem, &normalized)?;
        }
        other => anyhow::bail!("不支持保存的标签页：{other}"),
    }
    Ok(())
}

fn write_artifact_md(dir: &std::path::Path, stem: &str, content: &str) -> anyhow::Result<()> {
    let art = dir.join("artifacts");
    std::fs::create_dir_all(&art)?;
    // Prefer existing extension if present.
    let md = art.join(format!("{stem}.md"));
    let txt = art.join(format!("{stem}.txt"));
    let path = if md.is_file() {
        md
    } else if txt.is_file() {
        txt
    } else {
        md
    };
    std::fs::write(path, content)?;
    Ok(())
}

fn find_card_path(folder: &std::path::Path, key: &str) -> Option<std::path::PathBuf> {
    if key.is_empty() || !folder.is_dir() {
        return None;
    }
    let direct = folder.join(format!("{key}.md"));
    if direct.is_file() {
        return Some(direct);
    }
    for card in load_markdown_cards(folder, "card") {
        if card.slug == key || card.id == key || card.name == key || card.title == key {
            let p = folder.join(format!("{}.md", card.slug));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn write_card_preserving_frontmatter(
    path: &std::path::Path,
    new_body: &str,
) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let merged = replace_markdown_body(&existing, new_body);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, merged)?;
    Ok(())
}

fn replace_markdown_body(existing: &str, new_body: &str) -> String {
    let trimmed = existing.trim_start_matches('\u{feff}');
    if let Some(rest) = trimmed.strip_prefix("---") {
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(end) = rest.find("\n---") {
            let yaml = &rest[..end];
            let body = new_body.trim_start_matches('\n');
            return format!("---\n{yaml}\n---\n\n{body}");
        }
    }
    new_body.to_string()
}

async fn library_delete(State(state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    let dir = state.repo_root.join("projects").join(&name);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Json(serde_json::json!({"ok": true, "deleted": name}))
}

async fn chapter_delete(
    State(state): State<AppState>,
    Path((name, chapter)): Path<(String, u32)>,
) -> impl IntoResponse {
    let projects_root = state.repo_root.join("projects");
    let dir = project_dir(&projects_root, &name);
    if !dir.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"ok": false, "error": format!("项目「{name}」不存在")})),
        )
            .into_response();
    }
    match delete_chapter(&dir, chapter) {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "deleted": result.deleted,
                "remaining": result.remaining,
                "published_count": result.published_count,
                "next_chapter": result.next_chapter,
                "preview": build_preview(&state.repo_root, &name),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn ws_thread(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, id))
}

async fn handle_ws(socket: WebSocket, state: AppState, thread_id: String) {
    let (mut sink, mut stream) = socket.split();
    // Unbounded outbound queue — bounded queues + LLM token floods previously
    // dropped ItemCompleted / RequestUserInput (UI stuck on「仍在等待首包」).
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    let send_task = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // HTTP-initiated turns still publish on the bus; forward those too.
    let mut bus_rx = state.event_bus.subscribe();
    let out_bus = out_tx.clone();
    let tid_bus = thread_id.clone();
    let bus_task = tokio::spawn(async move {
        loop {
            match bus_rx.recv().await {
                Ok(ev) => {
                    if !event_matches_thread(&ev, &tid_bus) {
                        continue;
                    }
                    if let Ok(text) = serde_json::to_string(&ev) {
                        if out_bus.send(text).is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "ws event bus lagged; some events skipped");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        if let Message::Text(text) = msg {
            if let Ok(op) = serde_json::from_str::<Op>(&text) {
                let (tx, mut ev_rx) = mpsc::unbounded_channel::<EventMsg>();
                let out_tx = out_tx.clone();
                tokio::spawn(async move {
                    while let Some(ev) = ev_rx.recv().await {
                        if let Ok(text) = serde_json::to_string(&ev) {
                            if out_tx.send(text).is_err() {
                                break;
                            }
                        }
                    }
                });
                let core = state.core.clone();
                // Run turn concurrently so the socket can keep flushing deltas.
                tokio::spawn(async move {
                    let _ = core.handle_op(op, tx).await;
                });
            }
        }
    }
    send_task.abort();
    bus_task.abort();
}

fn event_matches_thread(ev: &EventMsg, thread_id: &str) -> bool {
    match ev {
        EventMsg::SessionConfigured { thread_id: tid, .. }
        | EventMsg::TurnStarted { thread_id: tid, .. }
        | EventMsg::TurnComplete { thread_id: tid, .. }
        | EventMsg::TurnAborted { thread_id: tid, .. }
        | EventMsg::ItemStarted { thread_id: tid, .. }
        | EventMsg::ItemCompleted { thread_id: tid, .. }
        | EventMsg::AgentMessageContentDelta { thread_id: tid, .. }
        | EventMsg::ReasoningContentDelta { thread_id: tid, .. }
        | EventMsg::ToolCallOutputDelta { thread_id: tid, .. }
        | EventMsg::RequestUserInput { thread_id: tid, .. }
        | EventMsg::TodoUpdated { thread_id: tid, .. }
        | EventMsg::ChatHistoryReset { thread_id: tid, .. } => tid == thread_id,
        EventMsg::AgentStatusChanged { status } => {
            status.thread_id == thread_id
                || status.parent_thread_id.as_deref() == Some(thread_id)
        }
        EventMsg::Error { .. } | EventMsg::Warning { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_config_file_rejects_traversal() {
        let dir = std::env::temp_dir().join("novelx-config-sandbox-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(resolve_config_file(&dir, "../secret.yaml").is_err());
        assert!(resolve_config_file(&dir, "a/../../b.yaml").is_err());
        let ok = resolve_config_file(&dir, "content_rules.yaml").unwrap();
        assert!(ok.ends_with("content_rules.yaml"));
        let root = dir.canonicalize().unwrap();
        assert!(ok.starts_with(&root));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn framework_skill_path_gate() {
        let dir = std::env::temp_dir().join("novelx-skill-path-test");
        let _ = fs::remove_dir_all(&dir);
        let skills = dir.join("skills/agents/writer");
        fs::create_dir_all(&skills).unwrap();
        let skill_file = skills.join("SKILL.md");
        fs::write(&skill_file, "---\nname: writer\n---\nbody").unwrap();
        let outside = dir.join("../outside.md");
        assert!(is_framework_skill_path(&dir, &skill_file));
        // project-style path outside config/skills
        let projects = dir.parent().unwrap().join("projects-sample/x/SKILL.md");
        let _ = fs::create_dir_all(projects.parent().unwrap());
        let _ = fs::write(&projects, "x");
        assert!(!is_framework_skill_path(&dir, &projects));
        let _ = outside;
        let _ = fs::remove_dir_all(&dir);
    }
}
