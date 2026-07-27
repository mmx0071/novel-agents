//! Tool handlers for NovelX agent loop (Codex-style handler + spec).

mod audit_queue;
mod expected_tools;
mod multi_agent;
mod mutation;
mod mutation_gate;
mod tool_ui;

pub use audit_queue::{
    clear_audit_queue, load_audit_queue, save_audit_queue, AuditQueue, AuditQueueItem,
    AuditQueueStatus,
};
pub use mutation::{
    apply_without_mutation_id, confirm_skipped, maybe_preview, mutation_confirm_enabled,
    preview_mutation, reject_apply_without_id, wants_apply, with_confirm_skip,
};
pub use tool_ui::{agent_label_zh, tool_output_for_ui};

use anyhow::Result;
use async_trait::async_trait;
use novelx_llm::LlmClient;
use novelx_harness::NamingRules;
use novelx_pipeline::project::{
    init_project, load_project_state, project_dir, read_chapter_outline, save_project_state,
    write_chapter_outline,
};
use novelx_pipeline::{
    active_volume_for_chapter, bound_for_volume, build_chapter_context, build_setting_audit_pack,
    build_structure_audit_candidate, check_chapter_order, check_plot_write_gate_with,
    check_revise_target, confirm_setup_approve, confirm_setup_revise, design_plot_force_allowed,
    display_chapter_outline, ensure_bridge_plot_active, ensure_plot_card_lifecycle_frontmatter,
    execute_pipeline, format_plot_progress_report_for, impact_source_arc, impact_source_bible,
    impact_source_draft, impact_source_entity, impact_source_master, impact_source_outline,
    list_plots_summary, list_projects, load_memory, load_meta_json, load_volume_bounds, lock_brief,
    lore_query, mark_volume_sync_skipped, materialize_plot_card_markdown,
    maybe_advance_setup_after_outlines, normalize_plot_card_best_effort,
    parse_chapter_outline_text, plot_design_blocked_reason, read_arc_outline_excerpt,
    read_chapter_draft, rebuild_plot_index, resolve_setup_next_step, resolve_setup_phase,
    resolve_volume_phase, run_setting_audit, run_volume_audit, run_volume_sync, set_volume_phase,
    steer_revision_options, update_plot_card, validate_arc_outline, validate_bible,
    validate_entity_card, validate_master_outline, volume_chapter_span, ContextProfile, EntityKind,
    PhaseEnforceFlags, PlotWriteGate, PlotWriteMode, RevisionOptions, RunMode, SettingAuditPackOpts,
    SetupPhase, VolumePhase,
};
use novelx_protocol::{AgentPath, ThreadId};
use novelx_skills::{build_skill_injections, load_skills, SkillScope};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub use multi_agent::{
    FollowupTask, InterruptAgent, ListAgents, SendAgentMessage, SpawnAgent, WaitAgent,
};

/// Runtime bridge so tools can spawn/wait Codex-style subagents without depending on novelx-core.
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn spawn_agent(&self, req: SpawnAgentRequest) -> Result<SpawnAgentResponse>;
    async fn send_message(&self, req: SendMessageRequest) -> Result<()>;
    async fn wait_agent(&self, thread_id: &str, timeout: Duration) -> Result<WaitAgentResponse>;
    async fn interrupt_agent(&self, thread_id: &str) -> Result<()>;
    async fn list_agents(&self, parent_thread_id: &str) -> Result<Vec<AgentListEntry>>;
}

#[derive(Debug, Clone)]
pub struct SpawnAgentRequest {
    pub parent_thread_id: ThreadId,
    pub role: String,
    pub task: String,
    pub project: Option<String>,
    pub chapter: Option<u32>,
    pub mode: Option<String>,
    pub revision: Option<RevisionOptions>,
}

#[derive(Debug, Clone)]
pub struct SpawnAgentResponse {
    pub thread_id: ThreadId,
    pub agent_path: AgentPath,
}

#[derive(Debug, Clone)]
pub struct SendMessageRequest {
    pub author_thread_id: ThreadId,
    pub recipient_thread_id: ThreadId,
    pub content: String,
    pub trigger_turn: bool,
}

#[derive(Debug, Clone)]
pub struct WaitAgentResponse {
    pub thread_id: ThreadId,
    pub summary: String,
    pub data: Value,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListEntry {
    pub thread_id: ThreadId,
    pub role: String,
    pub agent_path: String,
    pub lifecycle: String,
}

#[derive(Clone)]
pub struct ToolContext {
    pub projects_root: PathBuf,
    pub config_root: PathBuf,
    pub llm: Arc<LlmClient>,
    /// Optional sink for streaming tool progress (LLM chunks / step markers).
    pub progress: Option<mpsc::UnboundedSender<String>>,
    /// Codex-style multi-agent control (set by novelx-core).
    pub agent_runtime: Option<Arc<dyn AgentRuntime>>,
    /// Calling thread (root or subagent) for spawn parent attribution.
    pub caller_thread_id: Option<ThreadId>,
    /// Expected-event ids skipped for this turn (skip_once).
    pub skipped_expected_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub data: Value,
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult>;
}

pub struct ListProjects;
pub struct ContinueWriting;
pub struct ReviseChapter;
pub struct AuditChapter;
pub struct AuditChapters;
pub struct AuditVolume;
pub struct ApplyDraftPatch;
pub struct InitNovel;
pub struct QueryLore;
pub struct DesignEntity;
pub struct DesignPlot;
pub struct UpsertSetting;
pub struct DesignMasterOutline;
pub struct DesignArcOutline;
pub struct SyncVolume;
pub struct CreateNovel;
pub struct SupplementSetting;
pub struct AuditSetting;
pub struct GetProjectStatus;
pub struct LockBrief;
pub struct ConfirmSetup;
pub struct SteerRun;
pub struct OfferDecisions;
pub struct ActivateAgents;
pub struct QueryMemory;
pub struct ListEntities;
pub struct DeleteEntity;

#[async_trait]
impl ToolHandler for ListProjects {
    fn name(&self) -> &'static str {
        "list_projects"
    }
    fn description(&self) -> &'static str {
        "列出 projects/ 下的小说项目"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    async fn call(&self, ctx: &ToolContext, _args: Value) -> Result<ToolResult> {
        let names = list_projects(&ctx.projects_root)?;
        Ok(ToolResult {
            output: format!("项目：{}", names.join(", ")),
            data: json!({"projects": names}),
        })
    }
}

#[async_trait]
impl ToolHandler for ContinueWriting {
    fn name(&self) -> &'static str {
        "continue_writing"
    }
    fn description(&self) -> &'static str {
        "续写下一章或指定章节。用户说了第N章时必须传 chapter=N；省略则写 state.next_chapter。\
         若 next_chapter 已有未发布草稿，省略 chapter 不会静默重写——请 revise_chapter 或显式传下一章号。\
         前序剧情结束后：若需衔接则自动写至多 1 章过渡；否则须先 design_plot。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer","description":"目标章号。用户指定第N章时必填；省略则用 next_chapter"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("").to_string();
        let dir = project_dir(&ctx.projects_root, &project);
        let state = load_project_state(&dir)?;
        let explicit = args.get("chapter").and_then(|v| v.as_u64()).map(|c| c as u32);
        let chapter = explicit.unwrap_or(state.next_chapter).max(1);
        // Omitting chapter while next_chapter already has a draft used to silently rewrite
        // that chapter (e.g. model said「第7章」but left chapter unset → rewrote ch6).
        if explicit.is_none() {
            let existing = read_chapter_draft(&dir, chapter).unwrap_or_default();
            let chars = existing.chars().count();
            let min_chars = novelx_harness::StudioPolicies::load(&ctx.config_root).draft_min_chars();
            if chars >= min_chars {
                let suggest = chapter.saturating_add(1);
                return Ok(ToolResult {
                    output: format!(
                        "⛔ 未指定 chapter，且第{chapter}章已有正文（约 {chars} 字，next_chapter={chapter}，可能尚未审校通过）。\n\
                         - 修订第{chapter}章：revise_chapter(chapter={chapter}, …)\n\
                         - 新写第{suggest}章：continue_writing(chapter={suggest})\n\
                         - 先审校第{chapter}章：audit_chapter(chapter={chapter})"
                    ),
                    data: json!({
                        "blocked": true,
                        "reason": "draft_exists_without_chapter",
                        "project": project,
                        "chapter": chapter,
                        "suggest_chapter": suggest,
                        "draft_chars": chars,
                        "next_chapter": state.next_chapter,
                        "published_count": state.published_count,
                    }),
                });
            }
        }
        let _ = rebuild_plot_index(&dir);
        let skip_expected = args
            .get("confirm_skip_expected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let Some(block) = expected_tools::continue_writing_expected_block(
            &dir,
            &project,
            chapter,
            &ctx.skipped_expected_ids,
            skip_expected,
        ) {
            return Ok(block);
        }
        let enforce = PhaseEnforceFlags::load(&ctx.config_root);
        if enforce.chapter_order {
            if let Some(block) = check_chapter_order(&dir, chapter) {
                return Ok(ToolResult {
                    output: format!("⛔ 写章已拦截\n\n{}", block.message),
                    data: json!({
                        "blocked": true,
                        "reason": block.reason,
                        "handoff": false,
                        "project": project,
                        "chapter": chapter,
                        "next_chapter": block.next_chapter,
                        "requested_chapter": block.requested,
                    }),
                });
            }
        }
        let as_bridge = match check_plot_write_gate_with(&dir, enforce) {
            PlotWriteGate::Block {
                message,
                reason: block_reason,
                plot_title,
            } => {
                // Setup hard-block must keep reason=setup (do not remap via volume_phase).
                if block_reason == "setup" {
                    let setup_phase = resolve_setup_phase(&dir);
                    let setup_next = resolve_setup_next_step(&dir)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    return Ok(ToolResult {
                        output: format!("⛔ 写章已拦截\n\n{message}"),
                        data: json!({
                            "blocked": true,
                            "reason": "setup",
                            "setup_phase": setup_phase.as_str(),
                            "setup_next": setup_next,
                            "handoff": false,
                            "volume_phase": resolve_volume_phase(&dir).as_str(),
                            "project": project,
                            "chapter": chapter,
                        }),
                    });
                }
                let volume_phase = resolve_volume_phase(&dir);
                let reason = match volume_phase {
                    VolumePhase::AwaitingSync => "awaiting_sync",
                    VolumePhase::AwaitingNextArc => "awaiting_next_arc",
                    VolumePhase::AwaitingNextPlot => "awaiting_next_plot",
                    VolumePhase::DraftingVolume => block_reason,
                };
                let handoff = !matches!(volume_phase, VolumePhase::DraftingVolume)
                    || message.contains("卷间交接")
                    || matches!(
                        reason,
                        "need_design_plot" | "awaiting_next_arc" | "awaiting_next_plot"
                    );
                return Ok(ToolResult {
                    output: format!("⛔ 写章已拦截\n\n{message}"),
                    data: json!({
                        "blocked": true,
                        "reason": reason,
                        "plot_title": plot_title,
                        "handoff": handoff,
                        "volume_phase": volume_phase.as_str(),
                        "project": project,
                        "chapter": chapter,
                    }),
                });
            }
            PlotWriteGate::Allow { mode, detail } => {
                let bridge = matches!(mode, PlotWriteMode::Bridge);
                if bridge {
                    let _ = ensure_bridge_plot_active(&dir);
                }
                tracing::info!(chapter, %detail, ?mode, "plot write gate allow");
                bridge
            }
        };
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "continue_writing",
            &format!(
                "将创作第{chapter}章{}",
                if as_bridge { "（衔接章）" } else { "" }
            ),
            json!({
                "kind": "continue_writing",
                "chapter": chapter,
                "bridge": as_bridge,
                "markdown": format!("确认后开始写第{chapter}章流水线（章纲→正文→审校…）。"),
            }),
            "continue_writing",
            json!({
                "project": project,
                "chapter": chapter,
            }),
        ) {
            return Ok(prev);
        }
        let run = run_pipeline_streaming(
            ctx,
            &project,
            chapter,
            RunMode::Continue,
            RevisionOptions::default(),
        )
        .await?;
        let mut output = if ctx.progress.is_some() {
            if !run.message.is_empty() {
                format!("\n——\n{}", run.message)
            } else {
                format!("\n——\n第{chapter}章流水线完成")
            }
        } else {
            run.message.clone()
        };
        if as_bridge {
            output = format!("〔衔接章 · Agent 自动安排，最多 1 章〕\n{output}");
        }
        Ok(ToolResult {
            output,
            data: serde_json::to_value(&run)?,
        })
    }
}

#[async_trait]
impl ToolHandler for ReviseChapter {
    fn name(&self) -> &'static str {
        "revise_chapter"
    }
    fn description(&self) -> &'static str {
        "修订/扩写/重写已有章节正文。字数太少、扩写、重写会走 Writer 全文重写；仅改某几段时才局部补丁。不要用 audit_chapter 代替扩写。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "instructions":{"type":"string","description":"修订要求，如：扩写到3000字、重写本章、改第3段"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_patches":{"description":"局部修订确认时携带的 before/after 补丁数组"}
            },
            "required":["project","chapter","instructions"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("").to_string();
        let chapter = args["chapter"].as_u64().unwrap_or(1) as u32;
        let instructions = args["instructions"].as_str().unwrap_or("").to_string();
        let dir = project_dir(&ctx.projects_root, &project);
        if let Some(msg) = check_revise_target(&dir, chapter) {
            return Ok(ToolResult {
                output: format!("⛔ 修订已拦截\n\n{msg}"),
                data: json!({
                    "blocked": true,
                    "reason": "revise_no_draft",
                    "project": project,
                    "chapter": chapter,
                }),
            });
        }
        // Expansion / rewrite → full Writer; segment edits → local patch.
        let mut rev = steer_revision_options(&instructions);
        rev.revision_mode = true;
        let prefer_local = rev.prefer_local_patch;

        if apply_without_mutation_id(&ctx.config_root, &args) {
            return Ok(reject_apply_without_id());
        }

        // Local path: plan diffs without writing, then confirm-apply cached patches.
        if prefer_local && mutation_confirm_enabled(&ctx.config_root) && !wants_apply(&args) {
            let planned = novelx_pipeline::plan_local_revision_preview(
                &ctx.projects_root,
                &ctx.config_root,
                &project,
                chapter,
                rev.clone(),
                ctx.llm.clone(),
            )
            .await;
            match planned {
                Ok(preview) if !preview.patches.is_empty() => {
                    let diffs: Vec<Value> = preview
                        .patches
                        .iter()
                        .filter(|p| p.instruction != "__full_draft__")
                        .map(|p| {
                            json!({
                                "start_para": p.start_para,
                                "end_para": p.end_para,
                                "before": p.before,
                                "after": p.after,
                            })
                        })
                        .collect();
                    let patch_count = diffs.len().max(1);
                    return Ok(preview_mutation(
                        "revise_local",
                        &format!(
                            "第{chapter}章局部修订（{patch_count} 处补丁）：{}",
                            instructions.chars().take(80).collect::<String>()
                        ),
                        json!({
                            "kind": "revise_local",
                            "project": project,
                            "chapter": chapter,
                            "diffs": diffs,
                            "markdown": preview.summary_markdown,
                        }),
                        "revise_chapter",
                        json!({
                            "project": project,
                            "chapter": chapter,
                            "instructions": instructions,
                            "cached_patches": preview.patches,
                        }),
                    ));
                }
                Ok(_) => {
                    // No local patches planned — fall through to confirm full revise intent.
                }
                Err(e) => {
                    tracing::warn!(error = %e, "local revise preview failed; fall back to intent confirm");
                }
            }
        }

        if prefer_local && wants_apply(&args) {
            if let Some(patches) = args.get("cached_patches").cloned() {
                let applied = novelx_pipeline::apply_cached_local_patches(
                    &ctx.projects_root,
                    &project,
                    chapter,
                    &patches,
                )?;
                return Ok(ToolResult {
                    output: format!("已应用第{chapter}章局部修订（{} 处）。", applied),
                    data: json!({
                        "project": project,
                        "chapter": chapter,
                        "applied_patches": applied,
                        "published": false,
                        "impact_source": impact_source_draft(chapter),
                    }),
                });
            }
        }

        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "revise_chapter",
            &format!(
                "将修订第{chapter}章：{}",
                instructions.chars().take(100).collect::<String>()
            ),
            json!({
                "kind": "revise_chapter",
                "chapter": chapter,
                "instructions": instructions,
                "markdown": format!("确认后按指令修订第{chapter}章（可能全文重写）。"),
            }),
            "revise_chapter",
            json!({
                "project": project,
                "chapter": chapter,
                "instructions": instructions,
            }),
        ) {
            return Ok(prev);
        }

        let run = run_pipeline_streaming(ctx, &project, chapter, RunMode::Revise, rev).await?;
        let output = if ctx.progress.is_some() {
            if !run.message.is_empty() {
                format!("\n——\n{}", run.message)
            } else {
                format!("\n——\n第{chapter}章修订完成")
            }
        } else {
            run.message.clone()
        };
        let mut data = serde_json::to_value(&run)?;
        if let Some(obj) = data.as_object_mut() {
            obj.insert("impact_source".into(), impact_source_draft(chapter));
        }
        Ok(ToolResult { output, data })
    }
}

/// Revise / rewrite a chapter outline (`chapters/NNN/outline.json`) without running the writer.
struct ReviseOutline;

#[async_trait]
impl ToolHandler for ReviseOutline {
    fn name(&self) -> &'static str {
        "revise_outline"
    }
    fn description(&self) -> &'static str {
        "修订指定章的章纲（outline.json）。按用户指令改目标/冲突/关键事件/出场名单等；\
         只改正纲，不写正文。改正文请用 revise_chapter。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "instructions":{"type":"string","description":"章纲修订要求，如：加强章末钩子、补地点名单、对齐剧情卡收束"},
                "force":{"type":"boolean","description":"忽略结构审计 BLOCKER 强制进入预览/落盘"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_body":{"type":"string","description":"预览确认后携带的章纲 JSON，避免重复生成"}
            },
            "required":["project","chapter","instructions"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let chapter = args["chapter"].as_u64().unwrap_or(0) as u32;
        let instructions = args["instructions"].as_str().unwrap_or("").trim();
        let force = args["force"].as_bool().unwrap_or(false);
        if project.is_empty() || chapter == 0 {
            anyhow::bail!("project 与 chapter（≥1）必填");
        }
        if instructions.is_empty() {
            anyhow::bail!("instructions 必填（说明要改章纲的哪些点）");
        }
        if apply_without_mutation_id(&ctx.config_root, &args) {
            return Ok(reject_apply_without_id());
        }
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let existing = read_chapter_outline(&dir, chapter).unwrap_or_default();
        let draft = read_chapter_draft(&dir, chapter).unwrap_or_default();

        let generated = if wants_apply(&args) {
            args.get("cached_body")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };

        let generated = match generated {
            Some(cached) => cached,
            None => {
                let canon = build_chapter_context(
                    &dir,
                    chapter,
                    &draft,
                    &existing,
                    ContextProfile::Full,
                );
                let naming = NamingRules::load_from_config_root(&ctx.config_root).prompt_block();
                let skill = load_agent_skill(ctx, "chapter-planner");
                let draft_note = if draft.trim().is_empty() {
                    "（尚无正文；仅按指令与 CanonContext 修订章纲）".into()
                } else {
                    let excerpt: String = draft.chars().take(1200).collect();
                    format!("# 现有正文摘录（供对齐，勿输出正文）\n{excerpt}")
                };
                let existing_block = if existing.trim().is_empty() {
                    "（尚无章纲，请按指令新建完整 JSON）".into()
                } else {
                    format!("# 现有章纲 JSON\n{existing}")
                };
                let prompt = format!(
                    "修订小说《{}》（题材：{}）第{chapter}章章纲。\n\
                     用户修订要求：{instructions}\n\n\
                     只输出一个 JSON 对象（可包在 ```json 代码块中），不要 Markdown 散文或解释。\n\
                     必填字段：title, pov, time_location, goal, conflict, emotion_curve,\n\
                     key_events(数组≥2), characters(数组), items(数组), locations(数组),\n\
                     scene_tags(数组), cliffhanger, lore_queries(数组)。\n\
                     JSON 硬约束：字符串内禁止未转义的英文双引号；对话/强调用「」或『』；不要尾逗号。\n\
                     在现有章纲上按指令修改；未点名的情节尽量保留。必须服从 CanonContext。\n\
                     人物/组织命名遵守取名硬约束。\n\n\
                     {naming}\n\n{canon_md}\n\n{existing_block}\n\n{draft_note}",
                    state.name,
                    state.genre,
                    canon_md = canon.markdown,
                );
                let mut out = ctx
                    .llm
                    .complete(
                        &skill,
                        &prompt,
                        Some(&ctx.llm.model_for_agent("chapter_planner")),
                    )
                    .await?
                    .trim()
                    .to_string();
                // One repair pass if invalid.
                if parse_chapter_outline_text(&out).is_err() {
                    let err = parse_chapter_outline_text(&out)
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_default();
                    let repair = format!(
                        "第{chapter}章章纲 JSON 无法解析，请输出修正后的完整 JSON 对象（可包在 ```json 中），不要解释。\n\
                         错误：{err}\n\
                         要求：落实用户修订「{instructions}」；保留未点名情节；\
                         字符串内勿用未转义英文双引号；字段齐全。\n\n# 待修正原文\n{out}"
                    );
                    out = ctx
                        .llm
                        .complete(
                            &skill,
                            &repair,
                            Some(&ctx.llm.model_for_agent("chapter_planner")),
                        )
                        .await?
                        .trim()
                        .to_string();
                }
                out
            }
        };

        let parsed = match parse_chapter_outline_text(&generated) {
            Ok(o) => o,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("章纲格式不合规，未写入：{e}"),
                    data: json!({"blocked": true, "schema_error": e.to_string(), "chapter": chapter}),
                });
            }
        };
        let pretty = serde_json::to_string_pretty(&parsed)?;
        let preview_md = display_chapter_outline(&parsed);
        let structure_pack = build_structure_audit_candidate(
            &dir,
            &format!("第{chapter}章章纲"),
            &preview_md,
        );
        let audit = tool_setting_audit(ctx, project, "拟修订章纲", &structure_pack).await?;
        let setup_soft = resolve_setup_phase(&dir) != SetupPhase::Ready;
        if let Some(blocked) =
            mutation_gate::require_audit_pass_with(&audit, force, setup_soft)
        {
            return Ok(blocked);
        }

        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "revise_outline",
            &format!(
                "将修订第{chapter}章章纲：{}",
                instructions.chars().take(80).collect::<String>()
            ),
            json!({
                "kind": "revise_outline",
                "chapter": chapter,
                "path": dir.join(format!("chapters/{chapter:03}/outline.json")),
                "markdown": preview_md,
                "audit": mutation_gate::audit_preview_value(&audit),
            }),
            "revise_outline",
            json!({
                "project": project,
                "chapter": chapter,
                "instructions": instructions,
                "force": force,
                "cached_body": pretty,
            }),
        ) {
            return Ok(prev);
        }

        write_chapter_outline(&dir, chapter, &pretty)?;
        Ok(ToolResult {
            output: format!("已更新第{chapter}章章纲（outline.json）"),
            data: json!({
                "project": project,
                "chapter": chapter,
                "path": dir.join(format!("chapters/{chapter:03}/outline.json")),
                "title": parsed.title,
                "preview": preview_md.chars().take(600).collect::<String>(),
                "audit": mutation_gate::audit_preview_value(&audit),
                "impact_source": impact_source_outline(chapter, &existing, &pretty),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for AuditChapter {
    fn name(&self) -> &'static str {
        "audit_chapter"
    }
    fn description(&self) -> &'static str {
        "只审校不改正文。未通过时按结构化 issues 出决策项（修某条/修全部阻断/接受），经 offer_decisions 或系统兜底开卡。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"}
            },
            "required":["project","chapter"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("").to_string();
        let chapter = args["chapter"].as_u64().unwrap_or(1) as u32;
        // Audit must not use spawn/wait_agent — that path has stranded studio turns
        // after consistency_auditor finished while the parent stayed in wait_agent.
        let run = run_pipeline_streaming(
            ctx,
            &project,
            chapter,
            RunMode::AuditOnly,
            RevisionOptions::default(),
        )
        .await?;
        // When progress is live, the report already streamed via LlmDelta / step lines.
        // Do NOT paste run.message again — that duplicated the whole audit in the tool card.
        // Keep coda aligned with the real gate: consistency fail ≠ hard-rule block.
        let output = if ctx.progress.is_some() {
            audit_tool_coda(&run, chapter)
        } else if !run.report.is_empty() {
            run.report.clone()
        } else if !run.message.is_empty() {
            run.message.clone()
        } else {
            format!("第{chapter}章审校完成")
        };
        Ok(ToolResult {
            output,
            data: serde_json::to_value(&run)?,
        })
    }
}

#[async_trait]
impl ToolHandler for AuditChapters {
    fn name(&self) -> &'static str {
        "audit_chapters"
    }
    fn description(&self) -> &'static str {
        "多章逐章审阅队列（读正文）：action=start 可用 from/to 连续区间，或 chapters=[…] 不连续列表；\
         next/continue/cancel/status。整卷先复盘请用 audit_volume。禁止同轮多次 audit_chapter。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "from":{"type":"integer","description":"起始章（与 to 联用；无 chapters 时）"},
                "to":{"type":"integer","description":"结束章"},
                "chapters":{
                    "type":"array",
                    "items":{"type":"integer"},
                    "description":"不连续章号列表（优先于 from/to；卷复盘深审用）"
                },
                "action":{
                    "type":"string",
                    "enum":["start","continue","next","cancel","status"],
                    "description":"默认 start（有 chapters 或 from/to）或 continue（已有队列）"
                }
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("").to_string();
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let action = args["action"].as_str().unwrap_or("").to_string();
        let from = args["from"].as_u64().map(|n| n as u32);
        let to = args["to"].as_u64().map(|n| n as u32);
        let chapters_arg: Option<Vec<u32>> = args
            .get("chapters")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_u64().map(|n| n as u32))
                    .filter(|&n| n >= 1)
                    .collect()
            })
            .filter(|v: &Vec<u32>| !v.is_empty());

        let action = if action.is_empty() {
            if chapters_arg.is_some() || (from.is_some() && to.is_some()) {
                "start"
            } else if load_audit_queue(&ctx.projects_root, &project).is_some() {
                "continue"
            } else {
                anyhow::bail!("请提供 chapters 或 from/to 以开始审阅队列，或先 start");
            }
        } else {
            action.as_str()
        };

        match action {
            "status" => {
                let Some(q) = load_audit_queue(&ctx.projects_root, &project) else {
                    return Ok(ToolResult {
                        output: "当前无审阅队列".into(),
                        data: json!({"has_queue": false}),
                    });
                };
                Ok(ToolResult {
                    output: q.checklist_markdown(),
                    data: json!({"has_queue": true, "audit_queue": q.to_json()}),
                })
            }
            "cancel" => {
                let (summary, todos) = load_audit_queue(&ctx.projects_root, &project)
                    .map(|q| (q.summary_markdown(), q.to_codex_todos()))
                    .unwrap_or_else(|| ("当前无审阅队列".into(), vec![]));
                clear_audit_queue(&ctx.projects_root, &project)?;
                Ok(ToolResult {
                    output: summary,
                    data: json!({
                        "queue_cancelled": true,
                        "needs_user_choice": false,
                        "audit_queue": null,
                        "todos": todos,
                    }),
                })
            }
            "start" => {
                let mut q = if let Some(chs) = chapters_arg.clone() {
                    // Same chapter list already in progress → resume (do not wipe passed chapters).
                    if let Some(existing) = load_audit_queue(&ctx.projects_root, &project) {
                        let mut want = chs.clone();
                        want.sort_unstable();
                        want.dedup();
                        let mut have = existing.chapters.clone();
                        have.sort_unstable();
                        if have == want && !existing.is_finished() {
                            tracing::info!(
                                %project,
                                index = existing.index,
                                "audit_chapters start resumes existing queue"
                            );
                            let mut q = existing;
                            save_audit_queue(&ctx.projects_root, &q)?;
                            return run_audit_queue_until_gate(ctx, &mut q).await;
                        }
                        // Any unfinished queue: never silently reset to chapter 1.
                        if !existing.is_finished() {
                            tracing::warn!(
                                %project,
                                have = ?have,
                                want = ?want,
                                "audit_chapters start refused to reset active queue; resuming"
                            );
                            let mut q = existing;
                            save_audit_queue(&ctx.projects_root, &q)?;
                            return run_audit_queue_until_gate(ctx, &mut q).await;
                        }
                    }
                    AuditQueue::from_chapters(&project, chs)
                } else {
                    // Range start while a queue is active → resume instead of wiping.
                    if let Some(existing) = load_audit_queue(&ctx.projects_root, &project) {
                        if !existing.is_finished() {
                            tracing::info!(
                                %project,
                                index = existing.index,
                                "audit_chapters range start resumes existing queue"
                            );
                            let mut q = existing;
                            save_audit_queue(&ctx.projects_root, &q)?;
                            return run_audit_queue_until_gate(ctx, &mut q).await;
                        }
                    }
                    let from = from.unwrap_or(1);
                    let to = to.unwrap_or(from);
                    AuditQueue::new(&project, from, to)
                };
                save_audit_queue(&ctx.projects_root, &q)?;
                run_audit_queue_until_gate(ctx, &mut q).await
            }
            "next" => {
                let mut q = load_audit_queue(&ctx.projects_root, &project)
                    .ok_or_else(|| anyhow::anyhow!("无审阅队列，请先 start"))?;
                // Mark current as skipped if still pending/failed.
                if !q.is_finished() {
                    let cur = q.results.get(q.index).map(|r| r.status.clone());
                    if matches!(
                        cur,
                        Some(AuditQueueStatus::Pending | AuditQueueStatus::Failed)
                    ) {
                        q.set_current(AuditQueueStatus::Skipped, "用户跳过，继续下一章");
                    }
                    if !q.advance() {
                        let todos = q.to_codex_todos();
                        let out = q.summary_markdown();
                        clear_audit_queue(&ctx.projects_root, &project)?;
                        return Ok(ToolResult {
                            output: out,
                            data: queue_data_finished(&project, todos),
                        });
                    }
                    save_audit_queue(&ctx.projects_root, &q)?;
                }
                run_audit_queue_until_gate(ctx, &mut q).await
            }
            "continue" => {
                // After revise: re-audit the SAME chapter (Codex: finish in_progress before next).
                let mut q = load_audit_queue(&ctx.projects_root, &project)
                    .ok_or_else(|| anyhow::anyhow!("无审阅队列，请先 start"))?;
                if q.is_finished() {
                    let todos = q.to_codex_todos();
                    let out = q.summary_markdown();
                    clear_audit_queue(&ctx.projects_root, &project)?;
                    return Ok(ToolResult {
                        output: out,
                        data: queue_data_finished(&project, todos),
                    });
                }
                run_audit_queue_until_gate(ctx, &mut q).await
            }
            other => anyhow::bail!("未知 action: {other}"),
        }
    }
}

#[async_trait]
impl ToolHandler for AuditVolume {
    fn name(&self) -> &'static str {
        "audit_volume"
    }
    fn description(&self) -> &'static str {
        "整卷复盘（L1）：只读本卷各章摘要与卷纲，输出跨章问题与建议深审章号；\
         不逐章读正文。深审请再 audit_chapters(chapters=[…]) 或点「按建议深审」。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "volume":{"type":"integer","description":"卷第，默认当前未完成卷或最近完成卷"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let bounds = load_volume_bounds(&dir);
        let pc = state.published_count.max(1);
        let mut volume = if let Some(vi) = args["volume"].as_u64() {
            bound_for_volume(&bounds, vi as u32)
                .ok_or_else(|| anyhow::anyhow!("无法解析第{}卷", vi))?
        } else if let Some(b) = active_volume_for_chapter(&dir, pc) {
            b
        } else if let Some(b) = bounds.iter().rev().find(|b| b.completed).cloned() {
            b
        } else {
            bound_for_volume(&bounds, 1)
                .ok_or_else(|| anyhow::anyhow!("无法推断卷，请指定 volume"))?
        };
        if volume.end_chapter == 0 {
            volume.end_chapter = pc;
        }
        let (from, to) = volume_chapter_span(&volume, volume.end_chapter.max(1));
        if from > to {
            anyhow::bail!("本卷尚无已发布章节可复盘");
        }

        if let Some(p) = &ctx.progress {
            let _ = p.send(format!(
                "\n——\n卷级复盘：第{}卷（第{from}–{to}章，摘要层）\n",
                volume.volume_index
            ));
        }

        let skill = load_agent_skill(ctx, "volume-auditor");
        let report = run_volume_audit(&dir, &volume, ctx.llm.clone(), &skill).await?;
        let needs_choice = !report.suggested_chapters.is_empty();
        Ok(ToolResult {
            output: report.report_markdown.clone(),
            data: json!({
                "volume_index": report.volume_index,
                "from": report.from,
                "to": report.to,
                "passed": report.passed,
                "suggested_chapters": report.suggested_chapters,
                "issues": report.issues,
                "needs_user_choice": needs_choice,
                "volume_audit": true,
            }),
        })
    }
}

/// Audit current chapter; on pass, auto-advance until fail or queue done.
async fn run_audit_queue_until_gate(
    ctx: &ToolContext,
    q: &mut AuditQueue,
) -> Result<ToolResult> {
    let mut reports = Vec::new();
    loop {
        let Some(chapter) = q.current_chapter() else {
            let todos = q.to_codex_todos();
            let out = q.summary_markdown();
            let project = q.project.clone();
            clear_audit_queue(&ctx.projects_root, &project)?;
            return Ok(ToolResult {
                output: format!("{}\n\n{}", reports.join("\n\n——\n\n"), out),
                data: queue_data_finished(&project, todos),
            });
        };

        if let Some(p) = &ctx.progress {
            let _ = p.send(format!(
                "\n——\n审阅队列：第{chapter}章（{}/{}）\n{}\n",
                q.index + 1,
                q.chapters.len(),
                q.checklist_markdown()
            ));
        }

        let run = run_pipeline_streaming(
            ctx,
            &q.project,
            chapter,
            RunMode::AuditOnly,
            RevisionOptions::default(),
        )
        .await?;

        let consistency_ok = run.consistency_passed == Some(true);
        let hard_blocked = run.content_rule_blocked;
        // Advance only when consistency is clean and nothing else needs a human gate.
        let passed = consistency_ok && !hard_blocked && !run.needs_user_choice;
        let short = if passed {
            "一致性通过".to_string()
        } else if hard_blocked && consistency_ok {
            "硬规则未通过".to_string()
        } else if !consistency_ok {
            "一致性未通过".to_string()
        } else {
            "需选择下一步".to_string()
        };
        let body = if !run.message.is_empty() {
            run.message.clone()
        } else if !run.report.is_empty() {
            run.report.clone()
        } else {
            format!("第{chapter}章审校完成")
        };
        reports.push(format!("### 第{chapter}章\n{body}"));
        // Prior status before this audit result (Failed ⇒ this is a post-revise re-audit).
        let prior = q.results.get(q.index).map(|r| r.status.clone());

        if passed {
            let was_retry = matches!(prior, Some(AuditQueueStatus::Failed));
            q.set_current(
                if was_retry {
                    AuditQueueStatus::Revised
                } else {
                    AuditQueueStatus::Passed
                },
                &short,
            );
            save_audit_queue(&ctx.projects_root, q)?;
            if !q.advance() {
                let todos = q.to_codex_todos();
                let out = format!("{}\n\n{}", reports.join("\n\n——\n\n"), q.summary_markdown());
                let project = q.project.clone();
                clear_audit_queue(&ctx.projects_root, &project)?;
                return Ok(ToolResult {
                    output: out,
                    data: {
                        let mut d = queue_data_finished(&project, todos);
                        if let Some(obj) = d.as_object_mut() {
                            obj.insert("consistency_passed".into(), json!(true));
                            obj.insert("chapter".into(), json!(chapter));
                        }
                        d
                    },
                });
            }
            save_audit_queue(&ctx.projects_root, q)?;
            continue;
        }

        q.set_current(AuditQueueStatus::Failed, &short);
        save_audit_queue(&ctx.projects_root, q)?;
        let checklist = q.checklist_markdown();
        // Progress already carried the chapter report; keep a short fail coda + queue checklist.
        // Hard-rule block with clean consistency is not「一致性未通过」.
        let output = if ctx.progress.is_some() {
            if hard_blocked && consistency_ok {
                format!(
                    "\n——\n第{chapter}章硬规则未通过（详见上方流式输出；请在下方选择修正本章）\n\n{checklist}"
                )
            } else {
                format!(
                    "\n——\n第{chapter}章一致性未通过（详见上方流式输出）\n\n{checklist}"
                )
            }
        } else if hard_blocked && consistency_ok {
            format!(
                "\n——\n{}\n\n{}\n\n（硬规则未通过 — 请在下方选择修正本章）",
                reports.join("\n\n——\n\n"),
                checklist
            )
        } else {
            format!(
                "\n——\n{}\n\n{}\n\n（审校未通过 — 请在下方决策卡选择）",
                reports.join("\n\n——\n\n"),
                checklist
            )
        };
        let mut data = serde_json::to_value(&run)?;
        if let Some(obj) = data.as_object_mut() {
            obj.insert("needs_user_choice".into(), json!(true));
            // Keep real consistency result so hard-rule blocks open chapter_next, not audit P0 cards.
            if !consistency_ok {
                obj.insert("consistency_passed".into(), json!(false));
            }
            obj.insert("project".into(), json!(q.project));
            obj.insert("chapter".into(), json!(chapter));
            obj.insert("audit_queue".into(), q.to_json());
            obj.insert("queue_active".into(), json!(true));
            obj.insert("todos".into(), json!(q.to_codex_todos()));
        }
        return Ok(ToolResult { output, data });
    }
}

fn queue_data_finished(project: &str, todos: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "queue_finished": true,
        "needs_user_choice": false,
        "audit_queue": null,
        "project": project,
        "todos": todos,
    })
}

#[async_trait]
impl ToolHandler for ApplyDraftPatch {
    fn name(&self) -> &'static str {
        "apply_draft_patch"
    }
    fn description(&self) -> &'static str {
        "对章节正文应用局部段落补丁（显式局部编辑）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "instructions":{"type":"string"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_patches":{"description":"局部修订确认时携带的 before/after 补丁数组"}
            },
            "required":["project","chapter","instructions"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        // Delegate to revise with local-only (mutation preview handled in ReviseChapter).
        ReviseChapter.call(ctx, args).await
    }
}

#[async_trait]
impl ToolHandler for InitNovel {
    fn name(&self) -> &'static str {
        "init_novel"
    }
    fn description(&self) -> &'static str {
        "创建新小说项目"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "name":{"type":"string"},
                "genre":{"type":"string"},
                "chapters":{"type":"integer"}
            },
            "required":["name"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let name = args["name"].as_str().unwrap_or("untitled").to_string();
        let genre = args["genre"].as_str().unwrap_or("未定").to_string();
        let chapters = args["chapters"].as_u64().unwrap_or(100) as u32;
        let dir = init_project(&ctx.projects_root, &name, &genre, chapters)?;
        Ok(ToolResult {
            output: format!("已创建项目 {}", dir.display()),
            data: json!({"project": name, "path": dir}),
        })
    }
}

/// Read on-disk chapter draft / outline (not CanonContext).
pub struct ReadChapter;

#[async_trait]
impl ToolHandler for ReadChapter {
    fn name(&self) -> &'static str {
        "read_chapter"
    }
    fn description(&self) -> &'static str {
        "读取指定章的正文 draft.md 与章纲 outline（核对时间线/伤势/能力位置时用这个，不要用 query_lore）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer","description":"章号；默认最近已写章"},
                "max_chars":{"type":"integer","description":"正文最多返回字数，默认 12000"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let chapter = args["chapter"]
            .as_u64()
            .map(|n| n as u32)
            .unwrap_or_else(|| state.published_count.max(1));
        let max_chars = args["max_chars"].as_u64().unwrap_or(12000) as usize;
        let draft = read_chapter_draft(&dir, chapter).unwrap_or_default();
        let outline = read_chapter_outline(&dir, chapter).unwrap_or_default();
        let draft_chars = draft.chars().count();
        let draft_body: String = if draft_chars > max_chars {
            let head: String = draft.chars().take(max_chars).collect();
            format!("{head}\n\n…（正文共 {draft_chars} 字，已截断；可增大 max_chars）")
        } else {
            draft.clone()
        };
        let output = format!(
            "项目 {project} | 第{chapter}章 | published_count={} | next_chapter={}\n\n\
             # 章纲\n{}\n\n# 正文（draft.md，{} 字）\n{}",
            state.published_count,
            state.next_chapter,
            if outline.trim().is_empty() {
                "（无 outline）"
            } else {
                outline.trim()
            },
            draft_chars,
            if draft_body.trim().is_empty() {
                "（尚无正文）"
            } else {
                draft_body.trim()
            }
        );
        Ok(ToolResult {
            output,
            data: json!({
                "project": project,
                "chapter": chapter,
                "draft_chars": draft_chars,
                "has_draft": draft_chars > 0,
                "has_outline": !outline.trim().is_empty(),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for QueryLore {
    fn name(&self) -> &'static str {
        "query_lore"
    }
    fn description(&self) -> &'static str {
        "查询设定库/卷幕/记忆切片（CanonContext）。不含正文全文；要读章节正文请用 read_chapter"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "query":{"type":"string"},
                "chapter":{"type":"integer","description":"可选，默认最近已写章"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let query = args["query"].as_str().unwrap_or("");
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let chapter = args["chapter"]
            .as_u64()
            .map(|n| n as u32)
            .unwrap_or_else(|| state.published_count.max(1));
        let draft = read_chapter_draft(&dir, chapter).unwrap_or_default();
        let outline = read_chapter_outline(&dir, chapter).unwrap_or_default();
        // Seed haystack with query so name matches still work when draft is thin.
        let seeded = format!("{draft}\n{outline}\n{query}");
        let pack = build_chapter_context(&dir, chapter, &seeded, &outline, ContextProfile::Full);
        let lore = lore_query(&dir, chapter, &format!("{outline}\n{query}"), &draft);
        let wants_draft = {
            let q = query.to_lowercase();
            q.contains("正文")
                || q.contains("全文")
                || q.contains("draft")
                || q.contains("逐字")
                || q.contains("原稿")
        };
        let draft_note = if wants_draft {
            let n = draft.chars().count();
            format!(
                "\n\n⚠ query_lore 不返回正文全文（当前第{chapter}章 draft 约 {n} 字）。\
                 请改用工具 read_chapter(project, chapter={chapter})。"
            )
        } else {
            String::new()
        };
        let output = format!(
            "项目 {} | 查询章 {chapter} | next_chapter {}\n命中：{}\n\n{}\n\n{}{draft_note}",
            state.name,
            state.next_chapter,
            if pack.hits.is_empty() {
                "（基础卷幕/记忆）".into()
            } else {
                pack.hits.join(", ")
            },
            pack.markdown,
            lore
        );
        Ok(ToolResult {
            output,
            data: json!({
                "state": state,
                "chapter": chapter,
                "hits": pack.hits,
                "context_chars": pack.markdown.chars().count(),
                "lore_chars": lore.chars().count(),
                "hint_read_chapter": wants_draft,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for DesignEntity {
    fn name(&self) -> &'static str {
        "design_entity"
    }
    fn description(&self) -> &'static str {
        "新增或补全人物/物品/地点设定卡（写入 entities/）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "kind":{"type":"string","enum":["character","item","location"],"description":"实体类型"},
                "name":{"type":"string"},
                "brief":{"type":"string","description":"一句话设定或要点"},
                "role":{"type":"string","description":"可选：protagonist / supporting 等"},
                "force":{"type":"boolean","description":"忽略设定审计 BLOCKER 强制写入"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_body":{"type":"string","description":"预览确认后携带，避免重复生成"}
            },
            "required":["project","kind","name"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let kind = args["kind"].as_str().unwrap_or("character");
        let name = args["name"].as_str().unwrap_or("未命名").trim();
        let brief = args["brief"].as_str().unwrap_or("").trim();
        let role = args["role"].as_str().unwrap_or("");
        if project.is_empty() || name.is_empty() {
            anyhow::bail!("project 与 name 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let group = match kind {
            "item" => "items",
            "location" => "locations",
            _ => "characters",
        };
        let folder = dir.join("entities").join(group);
        let force = args["force"].as_bool().unwrap_or(false);
        // Reuse existing card when name is an alias / paren swap (避免「老严」与「严国栋（老严）」各一张).
        let (path, existing) =
            novelx_pipeline::resolve_entity_card_path(&folder, group, name);
        if let Some(card) = existing.as_ref() {
            if brief.is_empty() {
                return Ok(ToolResult {
                    output: format!("设定卡已存在 {}", path.display()),
                    data: json!({
                        "path": path,
                        "kind": kind,
                        "name": card.name,
                        "preview": card.markdown.chars().take(400).collect::<String>(),
                    }),
                });
            }
        } else if path.exists() && brief.is_empty() {
            let body = std::fs::read_to_string(&path)?;
            return Ok(ToolResult {
                output: format!("设定卡已存在 {}", path.display()),
                data: json!({"path": path, "kind": kind, "name": name, "preview": body.chars().take(400).collect::<String>()}),
            });
        }
        let generated = if wants_apply(&args) {
            args.get("cached_body")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let generated = match generated {
            Some(cached) => cached,
            None => generate_entity_card(ctx, project, kind, name, brief, role).await?,
        };
        let ek = EntityKind::from_group(group).unwrap_or(EntityKind::Character);
        let generated = match validate_entity_card(ek, &generated) {
            Ok(n) => n,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("设定卡格式不合规，未写入：{e}"),
                    data: json!({"blocked": true, "schema_error": e.to_string()}),
                });
            }
        };
        let audit = tool_setting_audit(ctx, project, "拟新增实体卡", &generated).await?;
        if let Some(blocked) = mutation_gate::require_audit_pass(&audit, force) {
            return Ok(blocked);
        }
        let write_name = existing
            .as_ref()
            .map(|c| c.name.as_str())
            .unwrap_or(name);
        let before_body = existing
            .as_ref()
            .map(|c| c.markdown.clone())
            .or_else(|| std::fs::read_to_string(&path).ok())
            .unwrap_or_default();
        let summary = if existing.is_some() {
            format!("将更新已有{kind}设定卡「{write_name}」（请求名「{name}」）")
        } else {
            format!("将写入{kind}设定卡「{name}」")
        };
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "design_entity",
            &summary,
            json!({
                "kind": kind,
                "name": write_name,
                "path": path,
                "markdown": generated,
                "merged_from": if existing.is_some() { name } else { "" },
                "audit": mutation_gate::audit_preview_value(&audit),
            }),
            "design_entity",
            json!({
                "project": project,
                "kind": kind,
                "name": name,
                "brief": brief,
                "role": role,
                "force": force,
                "cached_body": generated,
            }),
        ) {
            return Ok(prev);
        }
        // Apply path: re-check cached body (preview may be stale / tampered).
        if wants_apply(&args) {
            let generated_check = match validate_entity_card(ek, &generated) {
                Ok(n) => n,
                Err(e) => {
                    return Ok(ToolResult {
                        output: format!("设定卡格式不合规，未写入：{e}"),
                        data: json!({"blocked": true, "schema_error": e.to_string()}),
                    });
                }
            };
            let audit_apply =
                tool_setting_audit(ctx, project, "拟新增实体卡", &generated_check).await?;
            if let Some(blocked) = mutation_gate::require_audit_pass(&audit_apply, force) {
                return Ok(blocked);
            }
        }
        std::fs::create_dir_all(&folder)?;
        // Drop other near-duplicate filenames after writing the canonical path.
        if let Some(card) = existing.as_ref() {
            for other in novelx_pipeline::load_markdown_cards(&folder, group) {
                if other.slug == card.slug {
                    continue;
                }
                if novelx_pipeline::entity_names_equivalent(&other.name, name)
                    || novelx_pipeline::entity_names_equivalent(&other.name, &card.name)
                {
                    let other_path = folder.join(format!("{}.md", other.slug));
                    if other_path != path {
                        let _ = std::fs::remove_file(&other_path);
                    }
                }
            }
        }
        std::fs::write(&path, &generated)?;
        let keys = novelx_pipeline::load_markdown_cards(&folder, group)
            .into_iter()
            .find(|c| novelx_pipeline::entity_names_equivalent(&c.name, write_name))
            .map(|c| c.match_keys())
            .unwrap_or_else(|| vec![write_name.to_string()]);
        Ok(ToolResult {
            output: format!("已写入设定卡 {}", path.display()),
            data: json!({
                "path": path,
                "kind": kind,
                "name": write_name,
                "preview": generated.chars().take(400).collect::<String>(),
                "audit": mutation_gate::audit_preview_value(&audit),
                "impact_source": impact_source_entity(
                    kind,
                    write_name,
                    keys,
                    &before_body,
                    &generated,
                ),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for DesignPlot {
    fn name(&self) -> &'static str {
        "design_plot"
    }
    fn description(&self) -> &'static str {
        "新增卷内一段剧情卡到 plots/（须已有卷纲；scope=local，禁止复述整卷；有 in_progress/bridging 或欠衔接章时拒绝）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "title":{"type":"string"},
                "brief":{"type":"string"},
                "act":{"type":"integer","description":"可选卷/幕编号"},
                "force":{"type":"boolean","description":"跳过卷纲校验、进行中卡门控与设定审计阻断"},
                "activate":{"type":"boolean","description":"写入后设为 in_progress 并 set_active_main（卷间交接一键开卡）"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_body":{"type":"string","description":"预览确认后携带，避免重复生成"}
            },
            "required":["project","title"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let title = args["title"].as_str().unwrap_or("未命名剧情").trim();
        let brief = args["brief"].as_str().unwrap_or("").trim();
        let act = args["act"].as_u64();
        let mut force = args["force"].as_bool().unwrap_or(false);
        let activate = args["activate"].as_bool().unwrap_or(false);
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let enforce = PhaseEnforceFlags::load(&ctx.config_root);
        // Handoff / pre-setup: ignore force so volume_phase / arc gate cannot be bypassed.
        if force && enforce.volume && !design_plot_force_allowed(&dir) {
            tracing::info!(
                setup = %resolve_setup_phase(&dir).as_str(),
                volume = %resolve_volume_phase(&dir).as_str(),
                "design_plot force ignored during setup/handoff"
            );
            force = false;
        }
        if enforce.volume {
            // Mistaken volume-end (plot ladder unfinished) must not block design_plot.
            let _ = novelx_pipeline::recover_false_volume_end(&dir);
            match resolve_volume_phase(&dir) {
                VolumePhase::AwaitingSync => {
                    return Ok(ToolResult {
                        output: "卷末仍待设定同步（volume_phase=awaiting_sync）。请先 sync_volume 或跳过，再 design_arc_outline / design_plot。".into(),
                        data: json!({
                            "blocked": true,
                            "reason": "awaiting_sync",
                            "volume_phase": VolumePhase::AwaitingSync.as_str(),
                            "project": project,
                        }),
                    });
                }
                VolumePhase::AwaitingNextArc => {
                    return Ok(ToolResult {
                        output: "卷间交接须先 design_arc_outline（volume_phase=awaiting_next_arc），再 design_plot。".into(),
                        data: json!({
                            "blocked": true,
                            "reason": "awaiting_next_arc",
                            "volume_phase": VolumePhase::AwaitingNextArc.as_str(),
                            "project": project,
                        }),
                    });
                }
                _ => {}
            }
        }
        let vol_phase_now = resolve_volume_phase(&dir);
        let arc_excerpt = read_arc_outline_excerpt(&dir, 2500);
        if arc_excerpt.is_none() && !force {
            return Ok(ToolResult {
                output: format!(
                    "未找到可用卷纲（artifacts/arc_outlines/），请先 design_arc_outline 再 design_plot。\
                     （紧急跳过可传 force=true）"
                ),
                data: json!({
                    "blocked": true,
                    "reason": "missing_arc_outline",
                    "volume_phase": vol_phase_now.as_str(),
                    "project": project,
                }),
            });
        }
        if !force {
            if let Some(reason) = plot_design_blocked_reason(&dir) {
                // Hint: designing the named next_plot while predecessor is still open.
                let next_hint = {
                    let index = novelx_pipeline::load_plot_index(&dir);
                    index
                        .volumes
                        .iter()
                        .flat_map(|v| v.plots.iter())
                        .find(|p| {
                            matches!(p.status.as_str(), "in_progress" | "bridging")
                                && !p.next_plot.trim().is_empty()
                                && (p.next_plot.trim() == title
                                    || title.contains(p.next_plot.trim())
                                    || p.next_plot.trim().contains(title))
                        })
                        .map(|p| {
                            format!(
                                "\n下一步：先用 update_plot 将「{}」标为 completed（收束已兑现时），\
                                 若 needs_bridge 则先写衔接章或标 bridge_done，再 design_plot「{}」。\
                                 这不是整卷结束。",
                                p.title, p.next_plot.trim()
                            )
                        })
                        .unwrap_or_default()
                };
                return Ok(ToolResult {
                    output: format!("{reason}{next_hint}"),
                    data: json!({
                        "blocked": true,
                        "reason": "active_plot_or_pending_bridge",
                        "volume_phase": vol_phase_now.as_str(),
                        "project": project,
                    }),
                });
            }
        }
        let folder = dir.join("plots");
        let safe = sanitize_filename(title);
        let path = folder.join(format!("{safe}.md"));
        let body = if wants_apply(&args) {
            args.get("cached_body")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let body = match body {
            Some(cached) => cached,
            None => {
                generate_plot_card(
                    ctx,
                    project,
                    title,
                    brief,
                    act,
                    arc_excerpt.as_deref().unwrap_or(""),
                )
                .await?
            }
        };
        // Save-first: never discard a non-empty generation on schema mismatch.
        let (body, shape_repairs) = normalize_plot_card_best_effort(title, &body);
        let audit = tool_setting_audit(ctx, project, "拟新增剧情卡", &body).await?;
        if audit.blocker && !force {
            return Ok(ToolResult {
                output: format!("设定审计 BLOCKER，未写入：{}\n{}", audit.summary, audit.report),
                data: json!({
                    "blocked": true,
                    "reason": "setting_audit_blocker",
                    "audit": audit.raw,
                    "audit_summary": audit.summary,
                    "volume_phase": vol_phase_now.as_str(),
                    "project": project,
                }),
            });
        }
        let repair_note = if shape_repairs.is_empty() {
            String::new()
        } else {
            format!("；已自动修正格式：{}", shape_repairs.join("；"))
        };
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "design_plot",
            &format!(
                "将新增剧情卡「{title}」{}{repair_note}",
                if activate { "并激活" } else { "" }
            ),
            json!({
                "title": title,
                "activate": activate,
                "path": path,
                "markdown": body,
                "shape_repairs": shape_repairs,
            }),
            "design_plot",
            json!({
                "project": project,
                "title": title,
                "brief": brief,
                "act": act,
                "force": force,
                "activate": activate,
                "cached_body": body,
            }),
        ) {
            return Ok(prev);
        }
        std::fs::create_dir_all(&folder)?;
        std::fs::write(&path, &body)?;
        let vol = act
            .map(|n| n as u32)
            .filter(|n| *n >= 1)
            .unwrap_or_else(|| novelx_pipeline::resolve_arc_outline_volume(&dir, None));
        let initial_status = if activate { "in_progress" } else { "planned" };
        let _ = ensure_plot_card_lifecycle_frontmatter(&path, vol, initial_status);
        let _ = rebuild_plot_index(&dir);
        if activate {
            let _ = update_plot_card(
                &dir,
                title,
                Some("in_progress"),
                None,
                None,
                None,
                None,
                true,
            )?;
        }
        let vol_phase = resolve_volume_phase(&dir);
        let output = if activate {
            format!(
                "已写入并激活剧情卡 {}（status=in_progress，volume_phase={}）{repair_note}",
                path.display(),
                vol_phase.as_str()
            )
        } else {
            format!(
                "已写入剧情卡 {}（status=planned，已入 plots/index.json）{repair_note}",
                path.display()
            )
        };
        Ok(ToolResult {
            output,
            data: json!({
                "path": path,
                "title": title,
                "audit_summary": audit.summary,
                "activated": activate,
                "volume_phase": vol_phase.as_str(),
                "project": project,
                "shape_repairs": shape_repairs,
            }),
        })
    }
}

pub struct ListPlots;
pub struct UpdatePlot;

#[async_trait]
impl ToolHandler for ListPlots {
    fn name(&self) -> &'static str {
        "list_plots"
    }
    fn description(&self) -> &'static str {
        "汇报剧情进度（中文摘要：当前主推卡/收束条件/卷内卡链）；可按卷/状态过滤"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "volume":{"type":"integer","description":"可选：卷号 volume_index"},
                "status":{"type":"string","description":"可选：planned|in_progress|bridging|completed|abandoned"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let _ = rebuild_plot_index(&dir);
        let volume = args["volume"].as_u64().map(|v| v as u32);
        let status = args["status"].as_str();
        let data = list_plots_summary(&dir, volume, status);
        let plots = data
            .get("plots")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();
        // Human / plot_status intent → Chinese progress report.
        // Status-filtered queries keep a compact machine list for agents.
        // Note: volume-only filter (e.g.「第三卷进行到哪了」) must stay human-readable.
        let output = if status.is_none() {
            format_plot_progress_report_for(&dir, volume)
        } else if plots.is_empty() {
            "尚无剧情卡（或过滤结果为空）".into()
        } else {
            let lines: Vec<String> = plots
                .iter()
                .map(|p| {
                    format!(
                        "v{} | {} | {} | {} | next={}",
                        p["volume_index"].as_u64().unwrap_or(0),
                        p["status"].as_str().unwrap_or("?"),
                        p["plot_type"].as_str().unwrap_or("?"),
                        p["title"].as_str().unwrap_or("?"),
                        p["next_plot"].as_str().unwrap_or("")
                    )
                })
                .collect();
            format!("剧情卡 {} 张：\n{}", lines.len(), lines.join("\n"))
        };
        Ok(ToolResult { output, data })
    }
}

#[async_trait]
impl ToolHandler for UpdatePlot {
    fn name(&self) -> &'static str {
        "update_plot"
    }
    fn description(&self) -> &'static str {
        "更新剧情卡状态/下一卡；可设为卷内 active_main（planned|in_progress|bridging|completed|abandoned）。完结以卡面「收束条件」+ plot_acceptor 为准；chapter_from/to/bridge_chapter 仅作展示、不驱动生命周期。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "title":{"type":"string","description":"剧情卡标题或文件名 slug"},
                "status":{"type":"string"},
                "chapter_from":{"type":"integer","description":"已弃用：不驱动状态机，仅写入展示字段"},
                "chapter_to":{"type":"integer","description":"已弃用：不驱动状态机，仅写入展示字段"},
                "bridge_chapter":{"type":"integer","description":"已弃用：衔接由 needs_bridge 决定"},
                "next_plot":{"type":"string"},
                "set_active_main":{"type":"boolean","description":"设为该卷当前主线剧情卡"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"}
            },
            "required":["project","title"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let title = args["title"].as_str().unwrap_or("").trim();
        if project.is_empty() || title.is_empty() {
            anyhow::bail!("project 与 title 必填");
        }
        let status = args["status"].as_str();
        let next_plot = args["next_plot"].as_str();
        let set_active = args["set_active_main"].as_bool().unwrap_or(false);
        let mut fields = serde_json::Map::new();
        fields.insert("title".into(), json!(title));
        if let Some(st) = status {
            fields.insert("status".into(), json!(st));
        }
        if let Some(np) = next_plot {
            fields.insert("next_plot".into(), json!(np));
        }
        if set_active {
            fields.insert("set_active_main".into(), json!(true));
        }
        if let Some(v) = args["chapter_from"].as_u64() {
            fields.insert("chapter_from".into(), json!(v));
        }
        if let Some(v) = args["chapter_to"].as_u64() {
            fields.insert("chapter_to".into(), json!(v));
        }
        if let Some(v) = args["bridge_chapter"].as_u64() {
            fields.insert("bridge_chapter".into(), json!(v));
        }
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "update_plot",
            &format!("将更新剧情卡「{title}」"),
            json!({"fields": Value::Object(fields)}),
            "update_plot",
            args.clone(),
        ) {
            return Ok(prev);
        }
        let dir = project_dir(&ctx.projects_root, project);
        let data = update_plot_card(
            &dir,
            title,
            args["status"].as_str(),
            args["chapter_from"].as_u64().map(|v| v as u32),
            args["chapter_to"].as_u64().map(|v| v as u32),
            args["bridge_chapter"].as_u64().map(|v| v as u32),
            args["next_plot"].as_str(),
            args["set_active_main"].as_bool().unwrap_or(false),
        )?;
        Ok(ToolResult {
            output: format!("已更新剧情卡「{title}」"),
            data,
        })
    }
}

#[async_trait]
impl ToolHandler for UpsertSetting {
    fn name(&self) -> &'static str {
        "upsert_setting"
    }
    fn description(&self) -> &'static str {
        "写入/更新设定：topic=名词表/nomenclature 时写 artifacts/nomenclature.md + lore/nomenclature.json；其它 topic 写 Bible（artifacts/bible.md）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "topic":{"type":"string","description":"设定主题：名词表 / nomenclature / 力量体系 / 地理 等"},
                "content":{"type":"string","description":"要点或完整 Markdown；可空则由模型补全"},
                "force":{"type":"boolean"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_body":{"type":"string","description":"预览确认后携带，避免重复生成"}
            },
            "required":["project","topic"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let topic = args["topic"].as_str().unwrap_or("总览").trim();
        let content = args["content"].as_str().unwrap_or("").trim();
        let force = args["force"].as_bool().unwrap_or(false);
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        if is_nomenclature_topic(topic) {
            return upsert_nomenclature_setting(ctx, &dir, project, content, force, &args).await;
        }
        let art = dir.join("artifacts");
        let path = art.join("bible.md");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let cached_merged = wants_apply(&args)
            .then(|| {
                args.get("cached_body")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            })
            .flatten();
        let full_bible = is_full_bible_topic(topic);
        let (section, merged) = if let Some(cached) = cached_merged {
            (None, cached)
        } else if full_bible {
            let body = generate_full_bible(ctx, project, content, &existing).await?;
            (Some(body.clone()), body)
        } else {
            let section = generate_bible_section(ctx, project, topic, content, &existing).await?;
            let merged = merge_bible_section(&existing, topic, &section);
            (Some(section), merged)
        };
        let merged = match validate_bible(&merged) {
            Ok(n) => n,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!(
                        "世界观格式不合规，未写入：{e}\n\
                         （Bible 须含 # 世界观 与 ## 0./1./2./7. 等必填节）"
                    ),
                    data: json!({"blocked": true, "schema_error": e.to_string()}),
                });
            }
        };
        let audit_candidate = section.as_deref().unwrap_or(merged.as_str());
        let audit = tool_setting_audit(
            ctx,
            project,
            &format!("拟更新 Bible·{topic}"),
            audit_candidate,
        )
        .await?;
        if let Some(blocked) = mutation_gate::require_audit_pass(&audit, force) {
            return Ok(blocked);
        }
        let preview_md = section.as_deref().unwrap_or(content);
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "upsert_setting",
            &format!("将更新 Bible 设定「{topic}」"),
            json!({
                "topic": topic,
                "path": path,
                "markdown": preview_md,
                "audit": mutation_gate::audit_preview_value(&audit),
            }),
            "upsert_setting",
            json!({
                "project": project,
                "topic": topic,
                "content": content,
                "force": force,
                "cached_body": merged,
            }),
        ) {
            return Ok(prev);
        }
        if wants_apply(&args) {
            let audit_apply = tool_setting_audit(
                ctx,
                project,
                &format!("拟更新 Bible·{topic}"),
                audit_candidate,
            )
            .await?;
            if let Some(blocked) = mutation_gate::require_audit_pass(&audit_apply, force) {
                return Ok(blocked);
            }
        }
        std::fs::create_dir_all(&art)?;
        std::fs::write(&path, &merged)?;
        Ok(ToolResult {
            output: format!("已更新 Bible：{topic} → {}", path.display()),
            data: json!({
                "path": path,
                "topic": topic,
                "chars": merged.chars().count(),
                "audit": mutation_gate::audit_preview_value(&audit),
                "impact_source": impact_source_bible(topic, &existing, &merged),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for DesignMasterOutline {
    fn name(&self) -> &'static str {
        "design_master_outline"
    }
    fn description(&self) -> &'static str {
        "生成/更新总纲（artifacts/master_outline.md；并镜像 markdown 到 story_outline.json）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "brief":{"type":"string","description":"一句话卖点或核心冲突"},
                "force":{"type":"boolean","description":"忽略结构审计 BLOCKER 强制进入预览/落盘"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_body":{"type":"string","description":"预览确认后携带，避免重复生成"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let brief = args["brief"].as_str().unwrap_or("");
        let force = args["force"].as_bool().unwrap_or(false);
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let art = dir.join("artifacts");
        let path = art.join("master_outline.md");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let text = if wants_apply(&args) {
            args.get("cached_body")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let text = match text {
            Some(cached) => cached,
            None => {
                let skill = load_agent_skill(ctx, "master-planner");
                let prompt = format!(
                    "为小说《{}》（题材：{}）撰写/更新总纲 Markdown。\n\
                     硬性输出要求：\n\
                     - 只输出 Markdown 正文本身，第一行必须是 `# 总纲`（可带副标题）\n\
                     - 必须含 H2：一句话卖点、三幕结构（或分卷）、主角弧、主线冲突；可含中后期升级台阶\n\
                     - 禁止寒暄、禁止自我介绍、禁止用 ``` 代码块包裹、禁止提及文件路径 / story_outline.json / JSON\n\
                     - 禁止规划「第N章」列表或全书目标章数\n\
                     用户补充：{brief}\n\n现有总纲（可空，可在其上修订）：\n{existing}",
                    state.name, state.genre
                );
                ctx.llm
                    .complete(
                        &skill,
                        &prompt,
                        Some(&ctx.llm.model_for_agent("master_planner")),
                    )
                    .await?
                    .trim()
                    .to_string()
            }
        };
        let text = match validate_master_outline(&text) {
            Ok(n) => n,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("总纲格式不合规，未写入：{e}"),
                    data: json!({"blocked": true, "schema_error": e.to_string()}),
                });
            }
        };
        let structure_pack = build_structure_audit_candidate(&dir, "总纲", &text);
        let audit = tool_setting_audit(ctx, project, "拟更新总纲", &structure_pack).await?;
        let setup_soft = resolve_setup_phase(&dir) != SetupPhase::Ready;
        if let Some(blocked) =
            mutation_gate::require_audit_pass_with(&audit, force, setup_soft)
        {
            return Ok(blocked);
        }
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "design_master_outline",
            "将生成/更新总纲 master_outline.md",
            json!({
                "path": path,
                "markdown": text,
                "audit": mutation_gate::audit_preview_value(&audit),
            }),
            "design_master_outline",
            json!({
                "project": project,
                "brief": brief,
                "force": force,
                "cached_body": text,
            }),
        ) {
            return Ok(prev);
        }
        std::fs::create_dir_all(&art)?;
        std::fs::write(&path, &text)?;
        if !brief.is_empty() {
            let _ = lock_brief(&dir, brief);
        }
        // Merge into story_outline.json; preserve acts and attach chapter bounds when parseable.
        let outline_path = art.join("story_outline.json");
        let mut outline: Value = if outline_path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&outline_path)?)
                .unwrap_or_else(|_| json!({"acts": []}))
        } else {
            json!({"acts": []})
        };
        if let Some(obj) = outline.as_object_mut() {
            obj.insert("markdown".into(), Value::String(text.clone()));
            obj.insert("updated_from".into(), Value::String("design_master_outline".into()));
            obj.entry("acts").or_insert_with(|| Value::Array(vec![]));
        }
        std::fs::write(&outline_path, serde_json::to_string_pretty(&outline)?)?;
        for b in novelx_pipeline::parse_bounds_from_markdown(&text) {
            let _ = novelx_pipeline::sync_act_chapter_bounds(&dir, &b);
        }
        let setup = maybe_advance_setup_after_outlines(&dir).unwrap_or(SetupPhase::Collecting);
        let offer_setup = setup == SetupPhase::AwaitingConfirm;
        Ok(ToolResult {
            output: format!(
                "已写入总纲 {}（并同步 story_outline.json）；setup_phase={}",
                path.display(),
                setup.as_str()
            ),
            data: json!({
                "path": path,
                "setup_phase": setup.as_str(),
                "offer_setup_confirm": offer_setup,
                "audit": mutation_gate::audit_preview_value(&audit),
                "impact_source": impact_source_master(&existing, &text),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for DesignArcOutline {
    fn name(&self) -> &'static str {
        "design_arc_outline"
    }
    fn description(&self) -> &'static str {
        "生成/更新指定卷的卷纲（artifacts/arc_outlines/{NN}.md；不用单一文件覆盖）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "arc":{"type":"integer","description":"卷第；默认当前活动卷"},
                "brief":{"type":"string"},
                "force":{"type":"boolean","description":"忽略结构审计 BLOCKER 强制进入预览/落盘"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"},
                "cached_body":{"type":"string","description":"预览确认后携带，避免重复生成"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let brief = args["brief"].as_str().unwrap_or("");
        let force = args["force"].as_bool().unwrap_or(false);
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let art = dir.join("artifacts");
        novelx_pipeline::migrate_arc_outlines(&dir);
        let arc = args["arc"]
            .as_u64()
            .map(|n| n as u32)
            .filter(|n| *n >= 1)
            .unwrap_or_else(|| novelx_pipeline::resolve_arc_outline_volume(&dir, None));
        let path = novelx_pipeline::arc_outline_path(&dir, arc);
        let text = if wants_apply(&args) {
            args.get("cached_body")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        let text = match text {
            Some(cached) => cached,
            None => {
                let master = std::fs::read_to_string(art.join("master_outline.md")).unwrap_or_default();
                let prior = novelx_pipeline::list_arc_outline_volumes(&dir)
                    .into_iter()
                    .filter(|v| *v != arc)
                    .filter_map(|v| {
                        novelx_pipeline::read_arc_outline_text(&dir, v).map(|t| {
                            format!(
                                "【已有第{v}卷卷纲节选】\n{}",
                                t.chars().take(800).collect::<String>()
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let current = novelx_pipeline::read_arc_outline_text(&dir, arc).unwrap_or_default();
                let current_block = if current.trim().is_empty() {
                    "（尚无本卷卷纲，请新建完整卷纲）".into()
                } else {
                    format!(
                        "# 现有本卷卷纲（请在其上按用户补充修订；未点名的目标/终止条件/人物弧尽量保留）\n{}",
                        current.chars().take(3500).collect::<String>()
                    )
                };
                let skill = load_agent_skill(ctx, "arc-planner");
                let prompt = format!(
                    "为《{}》第{arc}卷写/修订卷纲 Markdown：卷目标、冲突阶梯、关键节点、人物弧。\
                     标题必须是 `# 第{arc}卷 · …`（只写本卷，不要覆盖或改写其他卷）。\
                     **必须**含「## 卷末终止条件」条列（≥2条可核验条件）；不定章数，禁止写第A–B章硬区间。\
                     若已有本卷卷纲：只按用户补充改冲突点，禁止整卷无故重写。\
                     用户补充：{brief}\n\n总纲节选：\n{}\n\n{current_block}\n\n{prior}",
                    state.name,
                    master.chars().take(2500).collect::<String>(),
                );
                ctx.llm
                    .complete(
                        &skill,
                        &prompt,
                        Some(&ctx.llm.model_for_agent("arc_planner")),
                    )
                    .await?
                    .trim()
                    .to_string()
            }
        };
        let text = match validate_arc_outline(&text) {
            Ok(n) => n,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("卷纲格式不合规，未写入：{e}"),
                    data: json!({"blocked": true, "schema_error": e.to_string()}),
                });
            }
        };
        let before_arc = novelx_pipeline::read_arc_outline_text(&dir, arc).unwrap_or_default();
        let structure_pack =
            build_structure_audit_candidate(&dir, &format!("第{arc}卷卷纲"), &text);
        let audit = tool_setting_audit(ctx, project, "拟更新卷纲", &structure_pack).await?;
        let setup_soft = resolve_setup_phase(&dir) != SetupPhase::Ready;
        if let Some(blocked) =
            mutation_gate::require_audit_pass_with(&audit, force, setup_soft)
        {
            return Ok(blocked);
        }
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "design_arc_outline",
            &format!("将生成/更新第{arc}卷卷纲"),
            json!({
                "arc": arc,
                "path": path,
                "markdown": text,
                "audit": mutation_gate::audit_preview_value(&audit),
            }),
            "design_arc_outline",
            json!({
                "project": project,
                "arc": arc,
                "brief": brief,
                "force": force,
                "cached_body": text,
            }),
        ) {
            return Ok(prev);
        }
        std::fs::create_dir_all(&art)?;
        novelx_pipeline::write_arc_outline_text(&dir, arc, &text)?;
        // Persist ending conditions (+ soft meta) into story_outline.json.
        let mut bounds = novelx_pipeline::parse_volume_meta_from_markdown(&text);
        if bounds.is_empty() {
            if let Some(mut b) = bound_for_volume(&load_volume_bounds(&dir), arc) {
                b.volume_index = arc;
                bounds.push(b);
            }
        }
        for b in &bounds {
            if b.volume_index == arc || bounds.len() == 1 {
                let _ = novelx_pipeline::sync_act_chapter_bounds(&dir, b);
            }
        }
        let cond_n = bounds
            .iter()
            .find(|b| b.volume_index == arc)
            .map(|b| b.ending_conditions.len())
            .unwrap_or(0);
        let setup = maybe_advance_setup_after_outlines(&dir).unwrap_or(SetupPhase::Collecting);
        // Volume handoff: arc ready → need next plot card.
        if resolve_volume_phase(&dir) == VolumePhase::AwaitingNextArc {
            let _ = set_volume_phase(&dir, VolumePhase::AwaitingNextPlot);
        }
        let vol_phase = resolve_volume_phase(&dir);
        let offer_setup = setup == SetupPhase::AwaitingConfirm;
        let next_ch = load_project_state(&dir)
            .map(|s| s.next_chapter.max(1))
            .unwrap_or(1);
        let ee_hard = novelx_pipeline::list_hard_ok_candidates(&dir, next_ch).len();
        Ok(ToolResult {
            output: format!(
                "已写入第{arc}卷卷纲 {}（终止条件 {} 条）；setup_phase={}；volume_phase={}{}",
                path.display(),
                cond_n,
                setup.as_str(),
                vol_phase.as_str(),
                if ee_hard > 0 {
                    format!("；有 {ee_hard} 条预期硬条件已满足，建议 review_expected_events(scope=volume)")
                } else {
                    String::new()
                }
            ),
            data: json!({
                "path": path,
                "arc": arc,
                "ending_conditions": cond_n,
                "setup_phase": setup.as_str(),
                "volume_phase": vol_phase.as_str(),
                "offer_setup_confirm": offer_setup,
                "offer_volume_handoff": vol_phase == VolumePhase::AwaitingNextPlot,
                "offer_expected_review": ee_hard > 0,
                "expected_hard_ok": ee_hard,
                "project": project,
                "audit": mutation_gate::audit_preview_value(&audit),
                "impact_source": impact_source_arc(arc, &before_arc, &text),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for SyncVolume {
    fn name(&self) -> &'static str {
        "sync_volume"
    }
    fn description(&self) -> &'static str {
        "卷末批量同步设定库：人物/地点/物品、名词表、世界观、卷进度（不新建剧情卡，不写关系图谱）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "volume":{"type":"integer","description":"卷第，默认按 published_count 推断刚结束的卷"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let bounds = load_volume_bounds(&dir);
        let pc = state.published_count.max(1);
        let mut volume = if let Some(vi) = args["volume"].as_u64() {
            bound_for_volume(&bounds, vi as u32)
                .ok_or_else(|| anyhow::anyhow!("无法解析第{}卷", vi))?
        } else if let Some(b) = bounds.iter().rev().find(|b| b.completed).cloned() {
            b
        } else if let Some(b) = novelx_pipeline::active_volume_for_chapter(&dir, pc) {
            b
        } else {
            bound_for_volume(&bounds, 1)
                .ok_or_else(|| anyhow::anyhow!("无法推断卷，请指定 volume"))?
        };
        // Open-ended volumes (end_chapter=0) must still sync through published chapters.
        if volume.end_chapter == 0 {
            volume.end_chapter = pc;
        }

        let progress = ctx.progress.clone();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let forward = tokio::spawn(async move {
            if let Some(p) = progress {
                let mut saw_llm = false;
                while let Some(ev) = rx.recv().await {
                    match ev {
                        novelx_pipeline::PipelineEvent::StepStarted { agent } => {
                            let label = agent_label_zh(&agent);
                            let _ = p.send(format!("\n▶ {label}\n"));
                        }
                        novelx_pipeline::PipelineEvent::StepCompleted { agent, summary } => {
                            let label = agent_label_zh(&agent);
                            let _ = p.send(format!("✓ {label}: {summary}\n"));
                        }
                        novelx_pipeline::PipelineEvent::LlmDelta { .. } => {
                            if !saw_llm {
                                saw_llm = true;
                                let _ = p.send("…生成中（详见右侧阅读区）\n".into());
                            }
                        }
                        novelx_pipeline::PipelineEvent::Error { message } => {
                            let _ = p.send(format!("\n✕ {message}\n"));
                        }
                        _ => {}
                    }
                }
            } else {
                while rx.recv().await.is_some() {}
            }
        });

        let report = run_volume_sync(&dir, &volume, ctx.llm.clone(), Some(tx)).await?;
        let _ = forward.await;
        let _ = mark_volume_sync_skipped(&dir, false);
        let _ = set_volume_phase(&dir, VolumePhase::AwaitingNextArc);

        Ok(ToolResult {
            output: format!(
                "卷末同步完成：{}。volume_phase=awaiting_next_arc — 请 design_arc_outline 细化下卷。",
                report.message
            ),
            data: json!({
                "ok": true,
                "project": project,
                "volume_index": report.volume_index,
                "entities_upserted": report.entities_upserted,
                "plots_written": report.plots_written,
                "nomenclature_added": report.nomenclature_added,
                "bible_patches": report.bible_patches,
                "incomplete_entities": report.incomplete_entities,
                "message": report.message,
                "volume_phase": VolumePhase::AwaitingNextArc.as_str(),
                "offer_volume_handoff": true,
            }),
        })
    }
}

async fn generate_entity_card(
    ctx: &ToolContext,
    project: &str,
    kind: &str,
    name: &str,
    brief: &str,
    role: &str,
) -> Result<String> {
    let role_line = if role.is_empty() {
        String::new()
    } else {
        format!("role: {role}\n")
    };
    let skill = load_agent_skill(ctx, "entity-designer");
    let sections = match kind {
        "item" => "## Origin / ## Usage / ## Current status（中英标题均可）",
        "location" => "## Overview / ## Factions / ## Production（中英标题均可）",
        _ => "## History / ## Personality / ## Core events / ## Current status（中英标题均可）",
    };
    let prompt = format!(
        "为项目《{project}》设计{kind}「{name}」设定卡。要点：{brief}\n\
         输出 Markdown，含 YAML frontmatter：name；status=active|background|exited|consumed（默认 active）；\
         人物可填 holdings（持有物品，逗号分隔）；aliases 可选。\n\
         正文必须含固定节：{sections}。"
    );
    let body = ctx
        .llm
        .complete(
            &skill,
            &prompt,
            Some(&ctx.llm.model_for_agent("entity_designer")),
        )
        .await?;
    let cleaned = body.trim().to_string();
    if cleaned.starts_with("---") {
        Ok(cleaned)
    } else {
        Ok(format!(
            "---\nname: {name}\n{role_line}---\n\n# {name}\n\n{}\n",
            if brief.is_empty() {
                cleaned
            } else {
                format!("{brief}\n\n{cleaned}")
            }
        ))
    }
}

async fn generate_plot_card(
    ctx: &ToolContext,
    project: &str,
    title: &str,
    brief: &str,
    act: Option<u64>,
    arc_excerpt: &str,
) -> Result<String> {
    let skill = load_agent_skill(ctx, "plot-designer");
    let arc_block = if arc_excerpt.trim().is_empty() {
        "（无卷纲节选；仍须写出可核验进入/收束条件）".to_string()
    } else {
        format!("【当前卷纲】\n{arc_excerpt}")
    };
    let prompt = format!(
        "为《{project}》写剧情卡「{title}》。要点：{brief}\n\
         {arc_block}\n\
         【层级】卷纲统揽整卷；本卡只写卷内**一段**情节（scope 必须是 local）。\
         从卷纲冲突阶梯中切出本段节点，禁止把整卷阶梯/终止条件抄进一张卡。\
         收束条件须是本段落点（通常严于/早于卷纲终止条件），并写 next_plot 指向下一段。\
         不得另起与卷纲冲突的主线。\
         写概览、叙事走向节点、进入/收束条件、人物物品设定；\
         禁止填写 chapter_from/chapter_to/bridge_chapter 或「第N章」区间；\
         勿将 status=exited/consumed 的实体列入常规出场（闪回须注明）。\n\
         按 Skill 输出**严格 JSON**（不要 markdown 代码块、不要外层再包一层卡）。\
         status=planned；scope=local；含 volume_index/needs_bridge/next_plot/fields."
    );
    let body = ctx
        .llm
        .complete(
            &skill,
            &prompt,
            Some(&ctx.llm.model_for_agent("plot_designer")),
        )
        .await?;
    Ok(materialize_plot_card_markdown(&body, title, act))
}

fn is_full_bible_topic(topic: &str) -> bool {
    matches!(
        topic.trim(),
        "完整世界观" | "世界观" | "世界观 Bible" | "bible" | "Bible"
    )
}

async fn generate_bible_section(
    ctx: &ToolContext,
    project: &str,
    topic: &str,
    content: &str,
    existing: &str,
) -> Result<String> {
    if !content.is_empty() && content.chars().count() > 80 {
        return Ok(content.to_string());
    }
    let skill = load_agent_skill(ctx, "world-architect");
    let prompt = format!(
        "为《{project}》世界观补充主题「{topic}」。用户要点：{content}\n\
         现有 Bible 节选：\n{}\n\n只输出该主题下的 Markdown 正文（不要整本重写）。",
        existing.chars().take(2000).collect::<String>()
    );
    ctx.llm
        .complete(
            &skill,
            &prompt,
            Some(&ctx.llm.model_for_agent("world_architect")),
        )
        .await
}

async fn generate_full_bible(
    ctx: &ToolContext,
    project: &str,
    content: &str,
    existing: &str,
) -> Result<String> {
    if !content.is_empty()
        && content.chars().count() > 200
        && content.contains("# ")
        && validate_bible(content).is_ok()
    {
        return Ok(content.to_string());
    }
    let dir = project_dir(&ctx.projects_root, project);
    let meta = load_meta_json(&dir);
    let brief = meta
        .get("brief")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let state = load_project_state(&dir).ok();
    let genre = state
        .as_ref()
        .map(|s| s.genre.as_str())
        .unwrap_or("未定");
    let skill = load_agent_skill(ctx, "world-architect");
    let prompt = format!(
        "为小说《{project}》（题材：{genre}）生成**完整**世界观 Bible Markdown。\n\
         用户 brief：{brief}\n\
         补充要点：{content}\n\
         现有 Bible（可空，可在其上增补，勿无故推翻禁忌）：\n{}\n\n\
         必须输出完整文档：首个 H1 为「# 世界观」或「# 世界观 Bible」，\
         且含 ## 0./1./2./7. 等必填节；题材中立，勿套用无关模板。只输出 Markdown。",
        existing.chars().take(3000).collect::<String>()
    );
    ctx.llm
        .complete(
            &skill,
            &prompt,
            Some(&ctx.llm.model_for_agent("world_architect")),
        )
        .await
        .map(|s| s.trim().to_string())
}

fn load_agent_skill(ctx: &ToolContext, name: &str) -> String {
    let outcome = load_skills(&[
        (SkillScope::Studio, ctx.config_root.join("skills")),
        (SkillScope::Agent, ctx.config_root.join("skills/agents")),
    ]);
    let key = name.replace('_', "-");
    let inj = build_skill_injections(&outcome.skills, &[key.clone()]);
    inj.into_iter()
        .next()
        .map(|i| i.body)
        .unwrap_or_else(|| format!("你是 {name} Agent。"))
}

async fn tool_setting_audit(
    ctx: &ToolContext,
    project: &str,
    label: &str,
    candidate: &str,
) -> Result<novelx_pipeline::SettingAuditResult> {
    let dir = project_dir(&ctx.projects_root, project);
    run_setting_audit(
        &dir,
        &ctx.config_root,
        ctx.llm.as_ref(),
        label,
        candidate,
    )
    .await
}

fn is_nomenclature_topic(topic: &str) -> bool {
    let t = topic.trim().to_lowercase();
    t == "nomenclature"
        || t == "名词表"
        || t == "名词"
        || t.contains("名词表")
        || t.contains("nomenclature")
}

async fn upsert_nomenclature_setting(
    ctx: &ToolContext,
    dir: &std::path::Path,
    project: &str,
    content: &str,
    force: bool,
    args: &Value,
) -> Result<ToolResult> {
    let art = dir.join("artifacts");
    let lore = dir.join("lore");
    let md_path = art.join("nomenclature.md");
    let json_path = lore.join("nomenclature.json");
    let existing_md = std::fs::read_to_string(&md_path).unwrap_or_default();
    let body = if wants_apply(args) {
        args.get("cached_body")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let body = match body {
        Some(cached) => cached,
        None => {
            if content.trim().chars().count() > 80 {
                let trimmed = content.trim();
                if trimmed.starts_with('#') {
                    trimmed.to_string()
                } else {
                    format!("# 名词表\n\n{trimmed}\n")
                }
            } else {
                let skill = load_agent_skill(ctx, "nomenclature-curator");
                let prompt = format!(
                    "为《{project}》补全名词表 Markdown（表格或分级列表均可）。\
                     用户要点：{content}\n\n现有名词表：\n{existing_md}"
                );
                ctx.llm
                    .complete(
                        &skill,
                        &prompt,
                        Some(&ctx.llm.model_for_agent("nomenclature_curator")),
                    )
                    .await?
                    .trim()
                    .to_string()
            }
        }
    };
    let audit = tool_setting_audit(ctx, project, "拟更新名词表", &body).await?;
    if audit.blocker && !force {
        return Ok(ToolResult {
            output: format!("设定审计 BLOCKER，未写入：{}\n{}", audit.summary, audit.report),
            data: json!({"blocked": true, "audit": audit.raw}),
        });
    }
    if let Some(prev) = mutation::maybe_preview(
        &ctx.config_root,
        args,
        "upsert_setting",
        "将更新名词表 nomenclature.md",
        json!({
            "topic": "名词表",
            "path": md_path,
            "markdown": body,
        }),
        "upsert_setting",
        json!({
            "project": project,
            "topic": "名词表",
            "content": content,
            "force": force,
            "cached_body": body,
        }),
    ) {
        return Ok(prev);
    }
    std::fs::create_dir_all(&art)?;
    std::fs::create_dir_all(&lore)?;
    std::fs::write(&md_path, &body)?;
    let added = merge_nomenclature_json_from_markdown(&json_path, &body)?;
    Ok(ToolResult {
        output: format!(
            "已更新名词表 → {}（并合并 lore/nomenclature.json，新增/保留条目约 {added}）",
            md_path.display()
        ),
        data: json!({
            "path": md_path,
            "json_path": json_path,
            "topic": "名词表",
            "chars": body.chars().count(),
            "entities_touched": added,
        }),
    })
}

/// Merge ### Name entries from nomenclature markdown into lore/nomenclature.json.
fn merge_nomenclature_json_from_markdown(json_path: &std::path::Path, md: &str) -> Result<usize> {
    let mut root: Value = if json_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(json_path)?).unwrap_or_else(|_| json!({}))
    } else {
        json!({"entities": []})
    };
    let arr = root
        .as_object_mut()
        .map(|o| {
            o.entry("entities")
                .or_insert_with(|| Value::Array(vec![]))
                .as_array_mut()
        })
        .flatten();
    let Some(arr) = arr else {
        return Ok(0);
    };
    let mut touched = 0usize;
    let mut current: Option<(String, String)> = None;
    let mut buf = String::new();
    let flush = |arr: &mut Vec<Value>,
                 current: &mut Option<(String, String)>,
                 buf: &mut String,
                 touched: &mut usize| {
        let Some((name, category)) = current.take() else {
            buf.clear();
            return;
        };
        let summary: String = buf.split_whitespace().collect::<Vec<_>>().join(" ");
        let summary: String = summary.chars().take(200).collect();
        buf.clear();
        if name.is_empty() {
            return;
        }
        if let Some(existing) = arr.iter_mut().find(|e| {
            e.get("canonical_name").and_then(|x| x.as_str()) == Some(name.as_str())
                || e.get("name").and_then(|x| x.as_str()) == Some(name.as_str())
        }) {
            if !summary.is_empty() {
                if let Some(obj) = existing.as_object_mut() {
                    obj.insert("ability_or_trait".into(), Value::String(summary));
                    if let Some(cat) = obj.get("category").and_then(|x| x.as_str()) {
                        if cat.is_empty() {
                            obj.insert("category".into(), Value::String(category));
                        }
                    } else {
                        obj.insert("category".into(), Value::String(category));
                    }
                }
                *touched += 1;
            }
        } else {
            arr.push(json!({
                "canonical_name": name,
                "category": category,
                "aliases": [],
                "ability_or_trait": summary,
                "source": "upsert_setting",
            }));
            *touched += 1;
        }
    };
    for line in md.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("### ") {
            flush(arr, &mut current, &mut buf, &mut touched);
            let name = rest
                .split(['/', '（', '('])
                .next()
                .unwrap_or(rest)
                .trim()
                .to_string();
            current = Some((name, "concept".into()));
        } else if trimmed.starts_with("## ") {
            flush(arr, &mut current, &mut buf, &mut touched);
        } else if current.is_some() && !trimmed.is_empty() && !trimmed.starts_with('#') {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(trimmed);
        }
    }
    flush(arr, &mut current, &mut buf, &mut touched);
    std::fs::write(json_path, serde_json::to_string_pretty(&root)?)?;
    Ok(touched)
}

fn merge_bible_section(existing: &str, topic: &str, section: &str) -> String {
    let heading = format!("## {topic}");
    if existing.trim().is_empty() {
        return format!("# 世界观 Bible\n\n{heading}\n\n{}\n", section.trim());
    }
    if let Some(idx) = existing.find(&heading) {
        let after = &existing[idx + heading.len()..];
        let next = after.find("\n## ").map(|i| idx + heading.len() + i);
        let mut out = String::new();
        out.push_str(&existing[..idx]);
        out.push_str(&heading);
        out.push_str("\n\n");
        out.push_str(section.trim());
        out.push('\n');
        if let Some(n) = next {
            out.push_str(&existing[n..]);
        }
        out
    } else {
        format!("{}\n\n{heading}\n\n{}\n", existing.trim_end(), section.trim())
    }
}

fn sanitize_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "untitled".into()
    } else {
        s.chars().take(40).collect()
    }
}

#[async_trait]
impl ToolHandler for CreateNovel {
    fn name(&self) -> &'static str {
        "create_novel"
    }
    fn description(&self) -> &'static str {
        "创建新小说项目（init_novel 别名）"
    }
    fn parameters(&self) -> Value {
        InitNovel.parameters()
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        InitNovel.call(ctx, args).await
    }
}

#[async_trait]
impl ToolHandler for SupplementSetting {
    fn name(&self) -> &'static str {
        "supplement_setting"
    }
    fn description(&self) -> &'static str {
        "补充世界观设定（upsert_setting 别名）"
    }
    fn parameters(&self) -> Value {
        UpsertSetting.parameters()
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        UpsertSetting.call(ctx, args).await
    }
}

#[async_trait]
impl ToolHandler for AuditSetting {
    fn name(&self) -> &'static str {
        "audit_setting"
    }
    fn description(&self) -> &'static str {
        "设定层审计：冲突 + stub 缺口 + 摘要与卡漂移（不审章正文）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "focus":{"type":"string","description":"可选关注点"},
                "chapter":{"type":"integer","description":"可选：附带近章摘要上限章号"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let focus = args["focus"].as_str().unwrap_or("全局设定");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let upto = args["chapter"].as_u64().map(|n| n as u32).or_else(|| {
            load_project_state(&dir)
                .ok()
                .map(|s| s.published_count.max(1))
        });
        let pack = build_setting_audit_pack(
            &dir,
            &SettingAuditPackOpts {
                focus_plot: None,
                upto_chapter: upto,
                summary_limit: 5,
            },
        );
        let audit = tool_setting_audit(ctx, project, focus, &pack).await?;
        Ok(ToolResult {
            output: format!(
                "设定审计：{}\nblocker={}\n{}",
                audit.summary, audit.blocker, audit.report
            ),
            data: audit.raw,
        })
    }
}

#[async_trait]
impl ToolHandler for GetProjectStatus {
    fn name(&self) -> &'static str {
        "get_project_status"
    }
    fn description(&self) -> &'static str {
        "查看项目进度、setup/volume 阶段、active_agents、已有章节草稿"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{"project":{"type":"string"}},
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let state = load_project_state(&dir)?;
        let setup = resolve_setup_phase(&dir);
        let volume = resolve_volume_phase(&dir);
        let meta = load_meta_json(&dir);
        let brief = meta
            .get("brief")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let brief_preview: String = brief.chars().take(80).collect();
        let mut chapters = Vec::new();
        let ch_root = dir.join("chapters");
        if let Ok(rd) = std::fs::read_dir(ch_root) {
            for e in rd.flatten() {
                let draft = e.path().join("draft.md");
                if draft.exists() {
                    let n = e.file_name().to_string_lossy().to_string();
                    let len = std::fs::read_to_string(&draft)
                        .map(|t| t.chars().count())
                        .unwrap_or(0);
                    chapters.push(json!({"dir": n, "draft_chars": len}));
                }
            }
        }
        chapters.sort_by(|a, b| {
            a["dir"]
                .as_str()
                .unwrap_or("")
                .cmp(b["dir"].as_str().unwrap_or(""))
        });
        let output = format!(
            "《{}》题材={} 下一章={} 已发布={} setup={} volume={} brief={} agents={:?} 草稿章数={}",
            state.name,
            state.genre,
            state.next_chapter,
            state.published_count,
            setup.as_str(),
            volume.as_str(),
            if brief_preview.is_empty() {
                "（空）"
            } else {
                &brief_preview
            },
            state.active_agents,
            chapters.len()
        );
        // Progress authority: disk chapters + counters — strip stale state.extra.chapters.
        let mut state_json = serde_json::to_value(&state).unwrap_or(json!({}));
        if let Some(obj) = state_json.as_object_mut() {
            obj.remove("chapters");
            obj.insert("published_count".into(), json!(state.published_count));
            obj.insert("next_chapter".into(), json!(state.next_chapter));
        }
        let (ee_pending, ee_approved, ee_due) = novelx_pipeline::count_by_status(&dir);
        Ok(ToolResult {
            output: format!(
                "{output} 预期(待={ee_pending}/批={ee_approved}/到期={ee_due})"
            ),
            data: json!({
                "state": state_json,
                "chapters": chapters,
                "setup_phase": setup.as_str(),
                "volume_phase": volume.as_str(),
                "brief": brief,
                "deferred_pending": ee_pending,
                "deferred_approved": ee_approved,
                "deferred_due": ee_due,
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for LockBrief {
    fn name(&self) -> &'static str {
        "lock_brief"
    }
    fn description(&self) -> &'static str {
        "将用户灵感/一句话卖点写入 meta.brief（定稿收集阶段）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "brief":{"type":"string","description":"灵感摘要或一句话卖点"}
            },
            "required":["project","brief"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let brief = args["brief"].as_str().unwrap_or("").trim();
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        if brief.is_empty() {
            anyhow::bail!("brief 不能为空");
        }
        let dir = project_dir(&ctx.projects_root, project);
        if !dir.exists() {
            anyhow::bail!("项目不存在：{project}");
        }
        lock_brief(&dir, brief)?;
        let setup = resolve_setup_phase(&dir);
        Ok(ToolResult {
            output: format!(
                "已锁定灵感 brief（{}字）；setup_phase={}",
                brief.chars().count(),
                setup.as_str()
            ),
            data: json!({
                "brief": brief,
                "setup_phase": setup.as_str(),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for ConfirmSetup {
    fn name(&self) -> &'static str {
        "confirm_setup"
    }
    fn description(&self) -> &'static str {
        "确认或打回灵感定稿：approve 需总纲+卷纲+Bible 最低完备（0/1/2/7）；revise→collecting"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "action":{
                    "type":"string",
                    "description":"approve | revise",
                    "enum":["approve","revise"]
                },
                "instructions":{"type":"string","description":"revise 时的修改说明（可选）"}
            },
            "required":["project","action"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let action = args["action"].as_str().unwrap_or("").trim();
        let instructions = args["instructions"].as_str().unwrap_or("").trim();
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        if !dir.exists() {
            anyhow::bail!("项目不存在：{project}");
        }
        match action {
            "approve" | "confirm" | "确认定稿" => {
                let phase = confirm_setup_approve(&dir)?;
                // Fresh project ready to draft first volume.
                if resolve_volume_phase(&dir) != VolumePhase::AwaitingSync {
                    let _ = set_volume_phase(&dir, VolumePhase::DraftingVolume);
                }
                Ok(ToolResult {
                    output: format!(
                        "已确认定稿（setup_phase={}）。可 design_plot → update_plot(in_progress) → continue_writing。",
                        phase.as_str()
                    ),
                    data: json!({
                        "setup_phase": phase.as_str(),
                        "volume_phase": resolve_volume_phase(&dir).as_str(),
                        "action": "approve",
                    }),
                })
            }
            "revise" | "修改再生成" | "reject" => {
                let phase = confirm_setup_revise(&dir)?;
                let hint = if instructions.is_empty() {
                    String::new()
                } else {
                    format!(" 修改说明：{instructions}")
                };
                Ok(ToolResult {
                    output: format!(
                        "已打回定稿（setup_phase={}）。请按用户意见重新 design_master_outline / design_arc_outline。{hint}",
                        phase.as_str()
                    ),
                    data: json!({
                        "setup_phase": phase.as_str(),
                        "action": "revise",
                        "instructions": instructions,
                    }),
                })
            }
            _ => anyhow::bail!("action 须为 approve 或 revise，收到：{action}"),
        }
    }
}

#[async_trait]
impl ToolHandler for SteerRun {
    fn name(&self) -> &'static str {
        "steer_run"
    }
    fn description(&self) -> &'static str {
        "接续审校门控：revise（按 issue_ids 局部修订）/ accept（接受并结束）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "choice":{"type":"string","description":"revise | accept"},
                "instructions":{"type":"string"},
                "issue_ids":{
                    "type":"array",
                    "items":{"type":"string"},
                    "description":"只修这些 issue id；省略则修全部非基础设施问题"
                }
            },
            "required":["project","chapter","choice"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let chapter = args["chapter"].as_u64().unwrap_or(1) as u32;
        let choice = args["choice"].as_str().unwrap_or("").trim().to_ascii_lowercase();
        let extra = args["instructions"].as_str().unwrap_or("");
        let issue_ids: Vec<String> = args
            .get("issue_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let accept = choice == "accept"
            || choice.contains("接受")
            || choice == "accept_issues";
        if accept {
            return Ok(ToolResult {
                output: "已接受问题并关闭审校门控。未自动复审、未自动发布；需要时可手动 audit_chapter。".into(),
                data: json!({"accepted": true, "chapter": chapter, "consistency_passed": true}),
            });
        }
        let audit_path = dir
            .join("chapters")
            .join(format!("{chapter:03}"))
            .join("audit.json");
        let audit_text = std::fs::read_to_string(&audit_path).unwrap_or_default();
        let issues: Vec<Value> = serde_json::from_str::<Value>(&audit_text)
            .ok()
            .and_then(|v| v.get("issues").cloned())
            .and_then(|x| x.as_array().cloned())
            .unwrap_or_default();
        // Skip META infrastructure noise — it must not drive local-patch revise loops.
        // Re-normalize priorities so soft false-P0s (e.g. 「不算断档」) do not force rewrite.
        let issues: Vec<Value> = {
            let filtered: Vec<Value> = issues
                .into_iter()
                .filter(|i| {
                    let ty = i.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let msg = i.get("message").and_then(|v| v.as_str()).unwrap_or("");
                    !(ty.eq_ignore_ascii_case("META")
                        && (msg.contains("返回为空")
                            || msg.contains("无法解析")
                            || msg.contains("请重试")))
                })
                .collect();
            novelx_harness::normalize_consistency_issues(filtered).1
        };
        let issues = if issue_ids.is_empty() {
            issues
        } else {
            novelx_harness::filter_issues_by_ids(&issues, &issue_ids)
        };
        if !issue_ids.is_empty() && issues.is_empty() {
            anyhow::bail!(
                "issue_ids 未匹配到任何审校问题：{}",
                issue_ids.join(", ")
            );
        }
        let issue_brief = novelx_draft_patch::format_audit_issues_brief(&issues);
        let instructions = if !extra.is_empty() {
            if issue_brief.is_empty() {
                extra.to_string()
            } else {
                format!("{extra}\n\n{issue_brief}")
            }
        } else if !issue_brief.is_empty() {
            format!("按下列审校问题局部修订（仅这些项）。\n\n{issue_brief}")
        } else {
            "按最近一致性审计意见局部修订".to_string()
        };
        // Reuse revise_chapter so local patches go through diff confirm + cached apply.
        let revise_args = json!({
            "project": project,
            "chapter": chapter,
            "instructions": instructions,
        });
        let mut result = ReviseChapter.call(ctx, revise_args).await?;
        if let Some(obj) = result.data.as_object_mut() {
            obj.insert("issue_ids".into(), json!(issue_ids));
            obj.insert("targeted_issues".into(), json!(issues.len()));
        }
        Ok(result)
    }
}

#[async_trait]
impl ToolHandler for OfferDecisions {
    fn name(&self) -> &'static str {
        "offer_decisions"
    }
    fn description(&self) -> &'static str {
        "审校未通过后给出决策项（修某条 issue / 修全部阻断 / 接受等）。\
         每项须含 id、label、action=revise|accept|skip_queue|cancel_queue，revise 时带 issue_ids。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "chapter":{"type":"integer"},
                "prompt":{"type":"string","description":"短提示，可选"},
                "options":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{
                            "id":{"type":"string"},
                            "label":{"type":"string"},
                            "action":{"type":"string","description":"revise|accept|skip_queue|cancel_queue"},
                            "issue_ids":{"type":"array","items":{"type":"string"}},
                            "instructions":{"type":"string"}
                        },
                        "required":["label","action"]
                    }
                }
            },
            "required":["project","chapter","options"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let chapter = args["chapter"].as_u64().unwrap_or(1) as u32;
        let options = args
            .get("options")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        if options.is_empty() {
            anyhow::bail!("options 不能为空");
        }
        // Load issue ids from disk so the model can be validated server-side.
        let audit_path = project_dir(&ctx.projects_root, project)
            .join("chapters")
            .join(format!("{chapter:03}"))
            .join("audit.json");
        let issues: Vec<Value> = std::fs::read_to_string(audit_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("issues").and_then(|x| x.as_array()).cloned())
            .unwrap_or_default();
        let issues = novelx_harness::with_issue_ids(issues);
        let labels: Vec<String> = options
            .iter()
            .filter_map(|o| o.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect();
        Ok(ToolResult {
            output: format!(
                "已提交 {} 项决策供用户选择：{}",
                options.len(),
                labels.join(" · ")
            ),
            data: json!({
                "offer_decisions": true,
                "project": project,
                "chapter": chapter,
                "options": options,
                "issues": issues,
                "prompt": args.get("prompt").and_then(|v| v.as_str()).unwrap_or(""),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for ActivateAgents {
    fn name(&self) -> &'static str {
        "activate_agents"
    }
    fn description(&self) -> &'static str {
        "持久化激活/停用流水线 Agent（写入 state.active_agents）。润色用 literary_editor：mode=add 开启、remove 关闭；勿当成长文扩写。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "agents":{"type":"array","items":{"type":"string"},"description":"要激活的 agent id 列表"},
                "mode":{"type":"string","enum":["set","add","remove"],"description":"默认 add"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"}
            },
            "required":["project","agents"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let mode = args["mode"].as_str().unwrap_or("add");
        let agents: Vec<String> = args["agents"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let mut state = load_project_state(&dir)?;
        let before = state.active_agents.clone();
        match mode {
            "set" => state.active_agents = agents.clone(),
            "remove" => {
                state
                    .active_agents
                    .retain(|a| !agents.iter().any(|x| x == a));
            }
            _ => {
                for a in &agents {
                    if !state.active_agents.iter().any(|x| x == a) {
                        state.active_agents.push(a.clone());
                    }
                }
            }
        }
        // Always keep MVP.
        for m in novelx_harness::PipelineConfig::load(&ctx.config_root).mvp() {
            if !state.active_agents.iter().any(|x| x == m) {
                state.active_agents.push(m.clone());
            }
        }
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "activate_agents",
            &format!("将更新 active_agents（mode={mode}）"),
            json!({
                "fields": {
                    "mode": mode,
                    "agents": agents,
                    "before": before,
                    "after": state.active_agents,
                }
            }),
            "activate_agents",
            args.clone(),
        ) {
            return Ok(prev);
        }
        save_project_state(&dir, &state)?;
        Ok(ToolResult {
            output: format!("active_agents = {:?}", state.active_agents),
            data: json!({"active_agents": state.active_agents}),
        })
    }
}

#[async_trait]
impl ToolHandler for QueryMemory {
    fn name(&self) -> &'static str {
        "query_memory"
    }
    fn description(&self) -> &'static str {
        "查询 lore/memory.json 滚动摘要、未收线与已入库事实"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "query":{"type":"string"}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let query = args["query"].as_str().unwrap_or("");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let mem = load_memory(&dir);
        let mut lines = Vec::new();
        if !mem.rolling_summary.is_empty() {
            lines.push(format!("## 滚动摘要\n{}", mem.rolling_summary));
        }
        let open: Vec<_> = mem
            .open_threads
            .iter()
            .filter(|t| t.status == "open")
            .map(|t| format!("- {}", t.text))
            .collect();
        if !open.is_empty() {
            lines.push(format!("## 未收线\n{}", open.join("\n")));
        }
        let facts: Vec<_> = mem
            .asserted_facts
            .iter()
            .rev()
            .filter(|f| query.is_empty() || f.text.contains(query))
            .take(12)
            .map(|f| format!("- [第{}章] {}", f.chapter, f.text))
            .collect();
        if !facts.is_empty() {
            lines.push(format!("## 事实\n{}", facts.join("\n")));
        }
        let output = if lines.is_empty() {
            "记忆库尚空".into()
        } else {
            lines.join("\n\n")
        };
        Ok(ToolResult {
            output,
            data: json!({
                "digests": mem.recent_digests.len(),
                "open_threads": mem.open_threads.len(),
                "facts": mem.asserted_facts.len(),
            }),
        })
    }
}

#[async_trait]
impl ToolHandler for ListEntities {
    fn name(&self) -> &'static str {
        "list_entities"
    }
    fn description(&self) -> &'static str {
        "列出项目实体卡（人物/物品/地点）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "kind":{"type":"string","enum":["character","item","location","all"]}
            },
            "required":["project"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let kind = args["kind"].as_str().unwrap_or("all");
        if project.is_empty() {
            anyhow::bail!("project 必填");
        }
        let dir = project_dir(&ctx.projects_root, project);
        let groups: &[&str] = match kind {
            "character" => &["characters"],
            "item" => &["items"],
            "location" => &["locations"],
            _ => &["characters", "items", "locations"],
        };
        let mut names = Vec::new();
        for g in groups {
            let folder = dir.join("entities").join(g);
            if let Ok(rd) = std::fs::read_dir(folder) {
                for e in rd.flatten() {
                    if e.path().extension().and_then(|x| x.to_str()) == Some("md") {
                        names.push(format!(
                            "{}/{}",
                            g,
                            e.path().file_stem().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
            }
        }
        names.sort();
        Ok(ToolResult {
            output: if names.is_empty() {
                "尚无实体卡".into()
            } else {
                names.join("\n")
            },
            data: json!({"entities": names}),
        })
    }
}

#[async_trait]
impl ToolHandler for DeleteEntity {
    fn name(&self) -> &'static str {
        "delete_entity"
    }
    fn description(&self) -> &'static str {
        "删除人物/物品/地点设定卡（用于去重合并后清理冗余文件）"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "project":{"type":"string"},
                "kind":{"type":"string","enum":["character","item","location"],"description":"实体类型"},
                "name":{"type":"string","description":"卡文件名（不含 .md），如 闻曜（父亲）"},
                "apply":{"type":"boolean","description":"true=确认落盘；默认预览"},
                "mutation_id":{"type":"string"}
            },
            "required":["project","kind","name"]
        })
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let project = args["project"].as_str().unwrap_or("");
        let kind = args["kind"].as_str().unwrap_or("character");
        let name = args["name"].as_str().unwrap_or("").trim();
        if project.is_empty() || name.is_empty() {
            anyhow::bail!("project 与 name 必填");
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            anyhow::bail!("非法名称");
        }
        let group = match kind {
            "item" => "items",
            "location" => "locations",
            _ => "characters",
        };
        let path = project_dir(&ctx.projects_root, project)
            .join("entities")
            .join(group)
            .join(format!("{name}.md"));
        if !path.is_file() {
            return Ok(ToolResult {
                output: format!("未找到设定卡：{group}/{name}.md"),
                data: json!({"deleted": false, "path": path, "kind": kind, "name": name}),
            });
        }
        if let Some(prev) = mutation::maybe_preview(
            &ctx.config_root,
            &args,
            "delete_entity",
            &format!("将删除设定卡 {group}/{name}.md"),
            json!({
                "fields": {
                    "kind": kind,
                    "name": name,
                    "path": path,
                }
            }),
            "delete_entity",
            args.clone(),
        ) {
            return Ok(prev);
        }
        std::fs::remove_file(&path)?;
        Ok(ToolResult {
            output: format!("已删除设定卡 {group}/{name}.md"),
            data: json!({"deleted": true, "path": path, "kind": kind, "name": name}),
        })
    }
}

pub fn all_tools() -> Vec<Arc<dyn ToolHandler>> {
    vec![
        Arc::new(ListProjects),
        Arc::new(ContinueWriting),
        Arc::new(ReviseChapter),
        Arc::new(ReviseOutline),
        Arc::new(AuditChapter),
        Arc::new(AuditChapters),
        Arc::new(AuditVolume),
        Arc::new(ApplyDraftPatch),
        Arc::new(InitNovel),
        Arc::new(CreateNovel),
        Arc::new(QueryLore),
        Arc::new(ReadChapter),
        Arc::new(QueryMemory),
        Arc::new(GetProjectStatus),
        Arc::new(LockBrief),
        Arc::new(ConfirmSetup),
        Arc::new(ListEntities),
        Arc::new(DeleteEntity),
        Arc::new(DesignEntity),
        Arc::new(DesignPlot),
        Arc::new(ListPlots),
        Arc::new(UpdatePlot),
        Arc::new(UpsertSetting),
        Arc::new(SupplementSetting),
        Arc::new(AuditSetting),
        Arc::new(DesignMasterOutline),
        Arc::new(DesignArcOutline),
        Arc::new(SyncVolume),
        Arc::new(SteerRun),
        Arc::new(OfferDecisions),
        Arc::new(ActivateAgents),
        Arc::new(expected_tools::EnqueueExpectedEvent),
        Arc::new(expected_tools::UpdateExpectedEvent),
        Arc::new(expected_tools::ListExpectedEvents),
        Arc::new(expected_tools::ReviewExpectedEvents),
        Arc::new(expected_tools::ResolveExpectedEvent),
        Arc::new(SpawnAgent),
        Arc::new(SendAgentMessage),
        Arc::new(FollowupTask),
        Arc::new(WaitAgent),
        Arc::new(ListAgents),
        Arc::new(InterruptAgent),
    ]
}

pub fn tool_specs(tools: &[Arc<dyn ToolHandler>]) -> Vec<novelx_llm::ToolSpec> {
    tools
        .iter()
        .map(|t| novelx_llm::ToolSpec {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters(),
        })
        .collect()
}

pub async fn dispatch(
    tools: &[Arc<dyn ToolHandler>],
    ctx: &ToolContext,
    name: &str,
    arguments: &str,
) -> Result<ToolResult> {
    let mut args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    normalize_tool_args(&mut args);
    for t in tools {
        if t.name() == name {
            return t.call(ctx, args).await;
        }
    }
    anyhow::bail!("unknown tool: {name}")
}

/// Accept common aliases from models (project_id → project, etc.).
fn normalize_tool_args(args: &mut Value) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    let aliases = [
        ("project_id", "project"),
        ("project_name", "project"),
        ("novel", "project"),
        ("chapter_number", "chapter"),
        ("chapter_num", "chapter"),
        ("user_instructions", "instructions"),
        ("instruction", "instructions"),
        ("message", "instructions"),
    ];
    for (from, to) in aliases {
        if !obj.contains_key(to) {
            if let Some(v) = obj.remove(from) {
                obj.insert(to.to_string(), v);
            }
        } else {
            obj.remove(from);
        }
    }
}

/// Short tool-card coda after live-streamed audit progress.
/// Consistency fail and hard-rule block must not share the same「一致性未通过」wording.
fn audit_tool_coda(run: &novelx_pipeline::PipelineRun, chapter: u32) -> String {
    if run.consistency_passed == Some(false) {
        return "\n——\n一致性未通过（详见上方流式输出；请在下方决策卡选择）".into();
    }
    if run.content_rule_blocked {
        return format!(
            "\n——\n第{chapter}章硬规则未通过（未发布；详见上方；请在下方选择修正本章）"
        );
    }
    if run.needs_user_choice {
        return format!("\n——\n第{chapter}章审校需你选择下一步（请在下方决策卡选择）");
    }
    if run.consistency_passed == Some(true) {
        return "\n——\n一致性通过".into();
    }
    format!("\n——\n第{chapter}章审校完成")
}

/// Pipeline run that forwards step / LLM deltas into `ctx.progress` when set.
/// Public for studio/core callers that must avoid spawn/wait_agent.
pub async fn run_pipeline_streaming(
    ctx: &ToolContext,
    project: &str,
    chapter: u32,
    mode: RunMode,
    revision: RevisionOptions,
) -> Result<novelx_pipeline::PipelineRun> {
    use novelx_pipeline::PipelineEvent;

    if ctx.progress.is_none() {
        return execute_pipeline(
            &ctx.projects_root,
            &ctx.config_root,
            project,
            chapter,
            mode,
            revision,
            ctx.llm.clone(),
            None,
        )
        .await;
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<PipelineEvent>();
    let progress = ctx.progress.clone();
    let forward = tokio::spawn(async move {
        // Sparse step timeline — avoid flooding WS (which also drops ▶/✓ under lag).
        let mut prose_chars: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut last_emit_chars: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut last_prose_emit: std::collections::HashMap<String, std::time::Instant> =
            std::collections::HashMap::new();
        let mut draft_backed = false;
        let mut last_draft_chars: usize = 0;
        while let Some(ev) = rx.recv().await {
            let Some(p) = progress.as_ref() else {
                continue;
            };
            let chunk = match ev {
                PipelineEvent::StepStarted { agent } => {
                    prose_chars.remove(&agent);
                    last_emit_chars.remove(&agent);
                    last_prose_emit.remove(&agent);
                    draft_backed = false;
                    last_draft_chars = 0;
                    let label = agent_label_zh(&agent);
                    format!("\n▶ {label}\n")
                }
                PipelineEvent::StepCompleted { agent, summary } => {
                    prose_chars.remove(&agent);
                    last_emit_chars.remove(&agent);
                    last_prose_emit.remove(&agent);
                    draft_backed = false;
                    last_draft_chars = 0;
                    let label = agent_label_zh(&agent);
                    format!("✓ {label}: {summary}\n")
                }
                PipelineEvent::LlmDelta { agent, delta } => {
                    match classify_llm_progress_delta(&delta) {
                        LlmProgressKind::CallStarted => "  调用模型中…\n".into(),
                        LlmProgressKind::WaitingFirst { secs } => {
                            if secs > 0 && secs % 15 == 0 {
                                format!("  仍在等待首包… {secs}s\n")
                            } else {
                                continue;
                            }
                        }
                        LlmProgressKind::GeneratingHint => continue,
                        LlmProgressKind::ModelDone { chars } => {
                            if chars == 0 {
                                "  模型返回空（将按缺省处理）\n".into()
                            } else {
                                continue;
                            }
                        }
                        LlmProgressKind::Prose => {
                            // Never mirror raw tokens into the tool card — only sparse ticks.
                            // (Reader refresh still comes from DraftFlushed markers.)
                            let n = {
                                let e = prose_chars.entry(agent.clone()).or_insert(0);
                                *e += delta.chars().count();
                                *e
                            };
                            let prev = last_emit_chars.get(&agent).copied().unwrap_or(0);
                            let due = last_prose_emit
                                .get(&agent)
                                .map(|t| t.elapsed() >= std::time::Duration::from_secs(2))
                                .unwrap_or(true);
                            let milestone = n >= prev + 400;
                            if !(due || milestone) || n == prev {
                                continue;
                            }
                            last_prose_emit.insert(agent.clone(), std::time::Instant::now());
                            last_emit_chars.insert(agent.clone(), n);
                            if draft_backed {
                                format!("  … 正文生成中 · {n}字\n")
                            } else {
                                format!("  … 生成中 · {n}字\n")
                            }
                        }
                    }
                }
                PipelineEvent::DraftFlushed { chars } => {
                    draft_backed = true;
                    if chars <= last_draft_chars {
                        continue;
                    }
                    last_draft_chars = chars;
                    // Keep both: compact marker for reader sync + visible progress for the tool card.
                    format!("↻ draft · {chars}\n  正文已写入 · {chars}字\n")
                }
                PipelineEvent::AutoFixStarted { reason } => {
                    format!("\n⚙ 自动修复: {reason}\n")
                }
                PipelineEvent::AwaitingHuman { prompt, options } => {
                    let opts = options
                        .iter()
                        .enumerate()
                        .map(|(i, o)| format!("{}. {o}", i + 1))
                        .collect::<Vec<_>>()
                        .join(" · ");
                    format!(
                        "\n⏸ {}{}\n",
                        prompt.chars().take(200).collect::<String>(),
                        if opts.is_empty() {
                            String::new()
                        } else {
                            format!("（{opts}）")
                        }
                    )
                }
                PipelineEvent::Error { message } => format!("\n✕ {message}\n"),
                _ => continue,
            };
            if p.send(chunk).is_err() {
                break;
            }
        }
    });

    let run = execute_pipeline(
        &ctx.projects_root,
        &ctx.config_root,
        project,
        chapter,
        mode,
        revision,
        ctx.llm.clone(),
        Some(tx),
    )
    .await;
    let _ = forward.await;
    run
}

enum LlmProgressKind {
    CallStarted,
    WaitingFirst { secs: u32 },
    GeneratingHint,
    ModelDone { chars: usize },
    /// Mirrored model tokens — throttled char ticks (skipped when draft flush is active).
    Prose,
}

fn classify_llm_progress_delta(delta: &str) -> LlmProgressKind {
    let t = delta.trim();
    if t.is_empty() {
        return LlmProgressKind::Prose;
    }
    if t.contains("调用模型中") {
        return LlmProgressKind::CallStarted;
    }
    if let Some(secs) = parse_wait_first_secs(t) {
        return LlmProgressKind::WaitingFirst { secs };
    }
    if t.contains("生成中…") || t.contains("已生成约") {
        return LlmProgressKind::GeneratingHint;
    }
    if t.contains("模型返回完成") {
        let chars = t
            .rsplit(['，', ','])
            .next()
            .and_then(|s| {
                s.chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .unwrap_or(0);
        return LlmProgressKind::ModelDone { chars };
    }
    if t.starts_with("…") && t.contains("流式生成中") {
        return LlmProgressKind::GeneratingHint;
    }
    // Do NOT treat every chunk starting with '（' as status — writer/literary prose
    // often begins a parenthetical (e.g. 「（BLOOD-001）」) and would leak bare「（」
    // lines into the tool card. Harness status lines are already matched above.
    LlmProgressKind::Prose
}

fn parse_wait_first_secs(t: &str) -> Option<u32> {
    if !t.contains("等待首包") {
        return None;
    }
    // 「（仍在等待首包… 15s）」→ 15
    let digits: String = t
        .chars()
        .rev()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}
