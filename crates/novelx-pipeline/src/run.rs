use anyhow::Result;
use novelx_draft_patch::{
    apply_local_patches, format_audit_issues_brief, plan_revision, segment_paragraphs, slice_span,
    RevisionTarget,
};
use novelx_harness::{
    check_draft_with, collect_signals, evaluate_activation, has_blocking_violation,
    ContentRuleViolation,
    ContentRulesConfig, has_timeline_p0,
    consistency_human_option_labels, merge_verify_audit, normalize_consistency_issues,
    on_consistency_result, on_pacing_result, with_issue_ids, should_publish,
    ChapterBudget, GateDecision, HandlerKind, LengthAssessment, NamingRules, PipelineConfig,
    ScriptShapeConfig,
};
use novelx_llm::LlmClient;
use novelx_skills::{build_skill_injections, load_skills, SkillScope};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::context::{build_chapter_context, ContextProfile};
use crate::lore::{lore_assert_from_summary, lore_query};
use crate::memory::{apply_summary_json, load_memory, save_memory, OpenThread};
use crate::project::{
    chapter_memory_artifact_matches_draft, load_project_state, read_chapter_draft,
    read_chapter_outline, save_project_state, write_chapter_draft, write_chapter_memory_artifact,
    write_chapter_outline, write_chapter_outline_budget, ProjectState,
};
use crate::schemas::{
    draft_body_chars, normalize_draft_best_effort, normalize_script_best_effort,
    validate_draft, validate_script, MIN_DRAFT_BODY_CHARS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Continue,
    Revise,
    AuditOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionOptions {
    pub prefer_local_patch: bool,
    pub user_instructions: Option<String>,
    pub audit_issues: Vec<Value>,
    pub pacing_suggestions: Vec<Value>,
    /// When steering an existing draft, always revise locally first
    pub revision_mode: bool,
    /// Re-audit: verify previous issues (fixed/still_open) instead of full rediscovery.
    #[serde(default)]
    pub verify_previous: bool,
    /// Force full rediscovery even when prior open issues exist.
    #[serde(default)]
    pub full_rescan: bool,
    /// Batch: SoftLong still warns but skip publish so one compress revise can run first.
    /// Non-batch Continue keeps default false (SoftLong remains publishable).
    #[serde(default)]
    pub defer_soft_long_publish: bool,
}

impl Default for RevisionOptions {
    fn default() -> Self {
        Self {
            prefer_local_patch: true,
            user_instructions: None,
            audit_issues: vec![],
            pacing_suggestions: vec![],
            revision_mode: false,
            verify_previous: false,
            full_rescan: false,
            defer_soft_long_publish: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStep {
    pub agent: String,
    pub status: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: String,
    pub project: String,
    pub chapter: u32,
    pub mode: RunMode,
    pub steps: Vec<PipelineStep>,
    pub status: String,
    /// Human-readable aggregate (includes model audit reports when available)
    pub message: String,
    pub prefer_local_patch: bool,
    pub revision_applied_via_patch: bool,
    /// Concatenated model outputs for studio ToolCall cards
    #[serde(default)]
    pub report: String,
    /// Consistency auditor result (None if auditor did not run).
    #[serde(default)]
    pub consistency_passed: Option<bool>,
    /// Normalized consistency issues (for studio gate brief / revise).
    #[serde(default)]
    pub issues: Vec<Value>,
    /// Whether publish advanced `next_chapter` / `published_count`.
    #[serde(default)]
    pub published: bool,
    /// Hard content rules (banned names / meta「第N章」) blocked publish.
    #[serde(default)]
    pub content_rule_blocked: bool,
    /// Structured hard-rule hits for Studio chapter_next prompts (blocking + warnings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_rule_violations: Vec<ContentRuleViolation>,
    /// Studio should present fix options (audit fail / gate await / volume end).
    #[serde(default)]
    pub needs_user_choice: bool,
    /// Set when publish advanced to a volume's last chapter (sync/skip choice).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_ended: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_ended_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_ended_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_ended_end: Option<u32>,
    /// Plot cards completed on this publish (setting pass may have run).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plots_completed: Vec<String>,
    #[serde(default)]
    pub plot_setting_blocker: bool,
    /// `plot_acceptor` result when it ran: true=pass, false=fail, None=skipped/absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plot_accept_passed: Option<bool>,
    /// Rationale when plot accept failed (for gate revise instructions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plot_accept_rationale: Option<String>,
    /// Layered audit used light (Pacing) consistency context.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub consistency_light: bool,
    /// Length gate: ok | soft_short | hard_short | soft_short_escalated | soft_long | hard_long
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub length_status: String,
    /// HardLong auto-split wrote `chapter+1` (see `auto_split_to`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_split: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_split_to: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    RunStarted { run_id: String },
    StepStarted { agent: String },
    StepCompleted { agent: String, summary: String },
    /// Token/chunk from an in-step LLM call (for ToolCall streaming UI).
    LlmDelta { agent: String, delta: String },
    DraftPatched {
        start_para: usize,
        end_para: usize,
        before: String,
        after: String,
    },
    /// Intermittent draft.md flush while writer streams (UI looks live; not per-token).
    DraftFlushed { chars: usize },
    AwaitingHuman { prompt: String, options: Vec<String> },
    AutoFixStarted { reason: String },
    /// Batch continue banner / chapter outcome (tool-card progress).
    BatchProgress { message: String },
    /// Progressive Codex-style checklist (batch write, multi-step jobs).
    TodoList {
        todos: Vec<novelx_protocol::TodoItem>,
    },
    RunCompleted { run_id: String, message: String },
    Error { message: String },
}

/// Continue steps: MVP + project active + agents.yaml activation suggestions.
pub fn plan_chapter_steps_with_activation(
    config_root: &Path,
    project_dir: &Path,
    state: &ProjectState,
    draft: &str,
) -> Vec<String> {
    plan_chapter_steps_with_activation_for(config_root, project_dir, state, draft, state.next_chapter.max(1))
}

/// Plan chapter steps for a specific chapter (activation uses that chapter for lean/arc counts).
pub fn plan_chapter_steps_with_activation_for(
    config_root: &Path,
    project_dir: &Path,
    state: &ProjectState,
    draft: &str,
    chapter: u32,
) -> Vec<String> {
    let mode_key = crate::project::resolve_project_mode(project_dir).as_str();
    let short = crate::project::is_short_drama(project_dir);
    let pipe = PipelineConfig::load_for_mode(config_root, mode_key);
    let mut signals = collect_signals(project_dir, state.published_count, draft);
    signals.chapter = chapter;
    if let Some(vol) = crate::volume::active_volume_for_chapter(project_dir, chapter) {
        let (start, end) = crate::volume::volume_chapter_span(&vol, chapter);
        signals.chapters_in_current_arc = end.saturating_sub(start).saturating_add(1);
    } else {
        signals.chapters_in_current_arc = state.published_count.min(chapter);
    }
    let phase = crate::phases::resolve_volume_phase(project_dir);
    signals.volume_handoff = matches!(
        phase,
        crate::phases::VolumePhase::AwaitingSync
            | crate::phases::VolumePhase::AwaitingNextArc
            | crate::phases::VolumePhase::AwaitingNextPlot
    );
    let suggestions = evaluate_activation(config_root, &signals);
    if !suggestions.is_empty() {
        tracing::info!(
            suggestions = ?suggestions.iter().map(|s| (&s.agent, &s.reason)).collect::<Vec<_>>(),
            "agent activation suggestions"
        );
    }
    // Short-drama: no longform lean filtering (foreshadow/volume agents not in pipeline).
    let lean = (!short && longform_lean_enabled(config_root)).then_some(&signals);
    let tier = novelx_harness::LongformConfig::load_from_config_root(config_root).quality_tier;
    novelx_harness::resolve_pipeline_agents_with_tier(
        &state.active_agents,
        &pipe,
        &suggestions,
        lean,
        tier,
    )
}

fn longform_lean_enabled(config_root: &Path) -> bool {
    feature_flag(config_root, "pipeline.longform_lean", true)
}

/// Auto-split overlong tip drafts into chapter + chapter+1 (default on).
pub(crate) fn auto_split_hard_long_enabled(config_root: &Path) -> bool {
    feature_flag(config_root, "pipeline.auto_split_hard_long", true)
}

fn feature_flag(config_root: &Path, key: &str, default: bool) -> bool {
    let path = config_root.join("features.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return default;
    };
    #[derive(Deserialize)]
    struct FeaturesFile {
        #[serde(default)]
        features: std::collections::HashMap<String, bool>,
    }
    serde_yaml::from_str::<FeaturesFile>(&raw)
        .ok()
        .and_then(|f| f.features.get(key).copied())
        .unwrap_or(default)
}

fn use_layered_consistency_light(
    config_root: &Path,
    chapter: u32,
    outline: &str,
    verify_previous: bool,
) -> bool {
    if verify_previous {
        return false;
    }
    let lf = novelx_harness::LongformConfig::load_from_config_root(config_root);
    if !matches!(lf.audit_tier, novelx_harness::AuditTier::Layered) {
        return false;
    }
    let tags = outline.to_lowercase();
    let climax = ["战斗", "高潮", "决战", "对决", "battle", "climax", "confrontation"]
        .iter()
        .any(|t| tags.contains(t));
    // Full audit on odd chapters and climax-tagged outlines; light on routine even chapters.
    !(chapter % 2 == 1 || climax)
}

pub fn plan_revision_steps_with(pipe: &PipelineConfig, local_patch: bool) -> Vec<String> {
    if local_patch {
        // Distinct step id so UI/activity shows「局部修订」not full-chapter writer.
        vec!["local_reviser".into()]
    } else {
        pipe.revise_default().to_vec()
    }
}

/// Plan which pipeline agents run for a mode (disk-aware for Continue).
pub fn plan_steps_for_mode(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    mode: RunMode,
    revision: &RevisionOptions,
) -> Result<Vec<String>> {
    let project_dir = projects_root.join(project);
    let state = load_project_state(&project_dir)?;
    let existing_draft = read_chapter_draft(&project_dir, chapter).unwrap_or_default();
    if matches!(mode, RunMode::AuditOnly) && existing_draft.trim().is_empty() {
        let unit = if crate::project::is_short_drama(&project_dir) { "集" } else { "章" };
        let body = crate::project::unit_body_filename(&project_dir);
        anyhow::bail!("第{chapter}{unit}尚无正文（{body}），无法审校");
    }
    let mode_key = crate::project::resolve_project_mode(&project_dir).as_str();
    let pipe = PipelineConfig::load_for_mode(config_root, mode_key);
    let prefer_local = revision.prefer_local_patch
        && (revision.revision_mode
            || matches!(mode, RunMode::Revise)
            || !existing_draft.is_empty() && revision.user_instructions.is_some());
    Ok(match mode {
        RunMode::AuditOnly => pipe.audit_only().to_vec(),
        RunMode::Revise => plan_revision_steps_with(&pipe, prefer_local),
        RunMode::Continue => {
            if revision.revision_mode && !existing_draft.is_empty() {
                plan_revision_steps_with(&pipe, true)
            } else {
                plan_chapter_steps_with_activation_for(
                    config_root,
                    &project_dir,
                    &state,
                    &existing_draft,
                    chapter,
                )
            }
        }
    })
}

/// Run a single pipeline agent step (persists draft/outline to disk).
pub async fn execute_single_agent_step(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    agent: &str,
    mode: RunMode,
    revision: RevisionOptions,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<PipelineRun> {
    // Single-step (SPAWN child) must never publish — parent finalizes once.
    execute_pipeline_with_steps(
        projects_root,
        config_root,
        project,
        chapter,
        mode,
        revision,
        llm,
        tx,
        vec![agent.to_string()],
        false,
    )
    .await
}


fn normalize_unit_body_best_effort(
    project_dir: &Path,
    unit: u32,
    text: &str,
    script_shape: &ScriptShapeConfig,
) -> (String, Vec<String>) {
    if crate::project::is_short_drama(project_dir) {
        normalize_script_best_effort(unit, text, script_shape)
    } else {
        normalize_draft_best_effort(unit, text)
    }
}

fn validate_unit_body(
    project_dir: &Path,
    unit: u32,
    text: &str,
    script_shape: &ScriptShapeConfig,
) -> Result<String, crate::schemas::SchemaError> {
    if crate::project::is_short_drama(project_dir) {
        validate_script(unit, text, script_shape)
    } else {
        validate_draft(unit, text)
    }
}

pub async fn execute_pipeline(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    mode: RunMode,
    revision: RevisionOptions,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<PipelineRun> {
    let steps = plan_steps_for_mode(
        projects_root,
        config_root,
        project,
        chapter,
        mode,
        &revision,
    )?;
    execute_pipeline_with_steps(
        projects_root,
        config_root,
        project,
        chapter,
        mode,
        revision,
        llm,
        tx,
        steps,
        true,
    )
    .await
}

pub async fn execute_pipeline_with_steps(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    mode: RunMode,
    mut revision: RevisionOptions,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
    steps: Vec<String>,
    allow_publish: bool,
) -> Result<PipelineRun> {
    let project_dir = projects_root.join(project);
    let mut state = load_project_state(&project_dir)?;
    let run_id = format!("run_{}", Uuid::new_v4().simple());
    let pipeline_started = std::time::Instant::now();

    let emit = |tx: &Option<mpsc::UnboundedSender<PipelineEvent>>, ev: PipelineEvent| {
        if let Some(tx) = tx {
            let _ = tx.send(ev);
        }
    };

    emit(&tx, PipelineEvent::RunStarted { run_id: run_id.clone() });

    let existing_draft = read_chapter_draft(&project_dir, chapter).unwrap_or_default();
    if matches!(mode, RunMode::AuditOnly) && existing_draft.trim().is_empty() {
        let unit = if crate::project::is_short_drama(&project_dir) { "集" } else { "章" };
        let body = crate::project::unit_body_filename(&project_dir);
        anyhow::bail!("第{chapter}{unit}尚无正文（{body}），无法审校");
    }
    // Any re-audit with prior open issues defaults to verify (stops rediscovery growth),
    // unless caller forces full_rescan.
    if matches!(mode, RunMode::AuditOnly) && !revision.full_rescan {
        let audit_path = project_dir
            .join("chapters")
            .join(format!("{chapter:03}"))
            .join("audit.json");
        let prior = load_previous_audit_issues(&audit_path);
        if !prior.is_empty() {
            revision.verify_previous = true;
        }
    }
    let prefer_local = revision.prefer_local_patch
        && (revision.revision_mode
            || matches!(mode, RunMode::Revise)
            || !existing_draft.is_empty() && revision.user_instructions.is_some());

    let mut run = PipelineRun {
        id: run_id.clone(),
        project: project.to_string(),
        chapter,
        mode,
        steps: steps
            .iter()
            .map(|a| PipelineStep {
                agent: a.clone(),
                status: "pending".into(),
                summary: String::new(),
            })
            .collect(),
        status: "running".into(),
        message: String::new(),
        prefer_local_patch: prefer_local,
        revision_applied_via_patch: false,
        report: String::new(),
        consistency_passed: None,
        issues: Vec::new(),
        published: false,
        content_rule_blocked: false,
        content_rule_violations: Vec::new(),
        needs_user_choice: false,
        volume_ended: None,
        volume_ended_name: None,
        volume_ended_start: None,
        volume_ended_end: None,
        plots_completed: Vec::new(),
        plot_setting_blocker: false,
        plot_accept_passed: None,
        plot_accept_rationale: None,
        consistency_light: false,
        length_status: String::new(),
        auto_split: false,
        auto_split_to: None,
    };
    let audit_only = matches!(mode, RunMode::AuditOnly);
    let mut report_parts: Vec<String> = Vec::new();
    // Publish gate: only advance next_chapter when consistency ok & no blockers.
    let mut consistency_passed = true;
    let mut has_blocking = false;
    let mut await_human = false;
    let mut ran_summarizer = false;
    // Apply only after publish_ok — never complete a plot on unpublished drafts.
    let mut pending_plot_accept: Option<String> = None;

    // Agent SKILL.md + top-level shared docs (e.g. prose-pitfalls.md).
    let skill_outcome = load_skills(&[
        (SkillScope::Studio, config_root.join("skills")),
        (SkillScope::Agent, config_root.join("skills/agents")),
    ]);
    let naming = NamingRules::load_from_config_root(config_root);
    let naming_block = naming.prompt_block();
    let content_rules = ContentRulesConfig::load_from_config_root(config_root);
    let project_mode = crate::project::resolve_project_mode(&project_dir);
    let mode_key = project_mode.as_str();
    let short_drama = project_mode == crate::project::ProjectMode::ShortDrama;
    let chapter_budget = ChapterBudget::load_for_mode(config_root, mode_key);
    let script_shape = ScriptShapeConfig::load_from_config_root(config_root);
    let pipe = PipelineConfig::load_for_mode(config_root, mode_key);

    let mut draft = existing_draft.clone();
    let mut outline = read_chapter_outline(&project_dir, chapter).unwrap_or_default();
    let mut lore_slice = String::new();

    // Rebuild when draft/outline change materially; refresh each step for writers.
    let rebuild_canon = |draft: &str, outline: &str, profile: ContextProfile| {
        let pack = build_chapter_context(&project_dir, chapter, draft, outline, profile);
        tracing::info!(
            chapter,
            context_chars = pack.markdown.chars().count(),
            hits = ?pack.hits,
            ?profile,
            "canon context built"
        );
        pack
    };

    for (idx, agent) in steps.iter().enumerate() {
        run.steps[idx].status = "running".into();
        emit(
            &tx,
            PipelineEvent::StepStarted {
                agent: agent.clone(),
            },
        );

        // Inject skill full body for this agent (progressive disclosure at step).
        // local_reviser reuses writer skill for full-revise fallback; local patches use a
        // short system prompt inside revise_by_local_patches.
        let skill_key = if agent == "local_reviser" {
            if short_drama {
                "script-writer".to_string()
            } else {
                "writer".to_string()
            }
        } else {
            agent.replace('_', "-")
        };
        let injections = build_skill_injections(&skill_outcome.skills, &[skill_key]);
        let skill_body = injections
            .first()
            .map(|i| i.body.as_str())
            .unwrap_or("你是小说创作 Agent。");

        let handler_kind = pipe.handler_for(agent).map(|h| h.kind);
        let handler_focus = pipe
            .handler_for(agent)
            .and_then(|h| h.focus.clone());
        let summary = match handler_kind {
            Some(HandlerKind::ChapterPlanner) => {
                let canon = rebuild_canon(&draft, &outline, ContextProfile::Full);
                let mat_hooks = crate::materials::format_material_hooks_for_context(
                    &project_dir,
                    900,
                );
                let canon_md = if mat_hooks.is_empty() {
                    canon.markdown.clone()
                } else {
                    format!("{}\n\n{mat_hooks}", canon.markdown)
                };
                let prev_outline = outline.clone();
                let mut generated = run_chapter_planner(
                    &llm,
                    skill_body,
                    &state,
                    chapter,
                    &canon_md,
                    &naming_block,
                )
                .await?;
                let mut write_err = None;
                for attempt in 0..2u8 {
                    let write_outline = if short_drama {
                        write_chapter_outline(&project_dir, chapter, &generated)
                    } else {
                        write_chapter_outline_budget(&project_dir, chapter, &generated)
                    };
                    match write_outline {
                        Ok(()) => {
                            write_err = None;
                            break;
                        }
                        Err(e) => {
                            write_err = Some(e);
                            if attempt == 0 {
                                tracing::warn!(
                                    chapter,
                                    error = %write_err.as_ref().unwrap(),
                                    "chapter_planner JSON invalid; requesting one repair pass"
                                );
                                emit(
                                    &tx,
                                    PipelineEvent::LlmDelta {
                                        agent: agent.clone(),
                                        delta: "\n章纲 JSON 不合规，正在自动修正…\n".into(),
                                    },
                                );
                                generated = run_chapter_planner_repair(
                                    &llm,
                                    skill_body,
                                    chapter,
                                    &generated,
                                    &write_err.as_ref().unwrap().to_string(),
                                )
                                .await?;
                            }
                        }
                    }
                }
                // Soft budget repair: empty includes / missing defers under active plot.
                if write_err.is_none() {
                    if let Ok(parsed) =
                        crate::schemas::parse_chapter_outline_text_budget(&generated)
                    {
                        let has_plot =
                            crate::plots::active_plot_exit_context(&project_dir).is_some();
                        if let Some(hint) =
                            crate::schemas::outline_budget_repair_hint(&parsed, has_plot)
                        {
                            tracing::warn!(chapter, %hint, "chapter_planner budget soft repair");
                            emit(
                                &tx,
                                PipelineEvent::LlmDelta {
                                    agent: agent.clone(),
                                    delta: "\n章纲篇幅切片不完整，正在补全…\n".into(),
                                },
                            );
                            let repaired = run_chapter_planner_repair(
                                &llm,
                                skill_body,
                                chapter,
                                &generated,
                                &hint,
                            )
                            .await?;
                            let write_repaired = if short_drama {
                                write_chapter_outline(&project_dir, chapter, &repaired)
                            } else {
                                write_chapter_outline_budget(&project_dir, chapter, &repaired)
                            };
                            match write_repaired {
                                Ok(()) => generated = repaired,
                                Err(e) => {
                                    tracing::warn!(
                                        chapter,
                                        error = %e,
                                        "budget soft repair still invalid; keeping prior outline"
                                    );
                                }
                            }
                            if let Ok(again) =
                                crate::schemas::parse_chapter_outline_text_budget(&generated)
                            {
                                if crate::schemas::outline_budget_repair_hint(&again, has_plot)
                                    .is_some()
                                {
                                    tracing::warn!(
                                        chapter,
                                        "chapter_planner budget gaps remain after soft repair"
                                    );
                                }
                            }
                        }
                    }
                }
                match write_err {
                    None => {
                        // Persist pretty JSON from disk so downstream sees validated form.
                        outline = read_chapter_outline(&project_dir, chapter)
                            .unwrap_or(generated);
                        format!("章纲已生成（{} 字）", outline.chars().count())
                    }
                    Some(e) => {
                        // Keep a valid on-disk outline rather than failing the whole chapter.
                        if !prev_outline.trim().is_empty()
                            && crate::schemas::parse_chapter_outline_text(&prev_outline).is_ok()
                        {
                            tracing::warn!(
                                chapter,
                                error = %e,
                                "chapter_planner output invalid; keeping previous outline"
                            );
                            outline = prev_outline;
                            format!("章纲生成不合规，已保留上一版（{}）", e)
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
            Some(HandlerKind::LoreQuery) => {
                lore_slice = lore_query(&project_dir, chapter, &outline, &draft);
                if lore_slice.is_empty() {
                    "Lore query：暂无切片（记忆/实体尚薄）".into()
                } else {
                    format!("Lore query 完成（{} 字）", lore_slice.chars().count())
                }
            }
            Some(HandlerKind::Writer) => {
                let canon = rebuild_canon(&draft, &outline, ContextProfile::Full);
                let canon_md = if lore_slice.is_empty() {
                    canon.markdown
                } else {
                    format!("{}\n\n{lore_slice}", canon.markdown)
                };
                let (new_draft, via_patch) = run_writer(
                    &llm,
                    skill_body,
                    &state,
                    chapter,
                    &project_dir,
                    &outline,
                    &draft,
                    &revision,
                    prefer_local,
                    &tx,
                    &canon_md,
                    &naming_block,
                    &chapter_budget,
                    &content_rules,
                )
                .await?;
                draft = new_draft;
                // Hard gate: scrub DeepSeek-flavored / corpus-common names.
                let banned = naming.find_in_text(&draft);
                if !banned.is_empty() {
                    tracing::warn!(?banned, chapter, "banned names in draft; running nomenclature fix");
                    let nom_skill =
                        load_agent_skill_body(&skill_outcome.skills, "nomenclature-curator");
                    draft = run_nomenclature_rename(
                        &llm,
                        &nom_skill,
                        &project_dir,
                        &draft,
                        &banned,
                        &naming_block,
                        &tx,
                    )
                    .await
                    .unwrap_or(draft);
                }
                // Save-first: never discard a non-empty generation on schema mismatch.
                let (shaped, mut shape_issues) = normalize_unit_body_best_effort(
                    &project_dir,
                    chapter,
                    &draft,
                    &script_shape,
                );
                if shaped.trim().is_empty() {
                    anyhow::bail!("writer 产出为空，无法保留");
                }
                draft = shaped;
                write_chapter_draft(&project_dir, chapter, &draft)?;

                if !shape_issues.is_empty() {
                    emit(
                        &tx,
                        PipelineEvent::AutoFixStarted {
                            reason: format!(
                                "正文形状不合规，先已落盘保留，尝试自动修正：{}",
                                shape_issues.join("；")
                            ),
                        },
                    );
                    match run_draft_shape_fix(&llm, chapter, &draft, &shape_issues, &tx).await
                    {
                        Ok(fixed) => {
                            let (again, issues2) = normalize_unit_body_best_effort(
                                &project_dir,
                                chapter,
                                &fixed,
                                &script_shape,
                            );
                            if !again.trim().is_empty() {
                                draft = again;
                                write_chapter_draft(&project_dir, chapter, &draft)?;
                            }
                            shape_issues = issues2;
                        }
                        Err(e) => {
                            tracing::warn!(
                                chapter,
                                error = %e,
                                "draft shape fix LLM failed; keeping saved draft"
                            );
                        }
                    }
                }

                if shape_issues.is_empty() {
                    if let Ok(ok) =
                        validate_unit_body(&project_dir, chapter, &draft, &script_shape)
                    {
                        draft = ok;
                        write_chapter_draft(&project_dir, chapter, &draft)?;
                    }
                } else {
                    // Keep draft on disk; block publish so unfinished shape cannot ship.
                    has_blocking = true;
                    tracing::warn!(
                        chapter,
                        issues = ?shape_issues,
                        "draft retained with schema issues"
                    );
                }

                run.revision_applied_via_patch = via_patch;
                if !shape_issues.is_empty() {
                    format!(
                        "正文已保留（{} 字），形状待修：{}",
                        draft.chars().count(),
                        shape_issues.join("；")
                    )
                } else if via_patch {
                    format!("局部补丁修订完成（{} 字）", draft.chars().count())
                } else {
                    format!("正文已写入（{} 字）", draft.chars().count())
                }
            }
            Some(HandlerKind::Nomenclature) => {
                let banned = naming.find_in_text(&draft);
                if banned.is_empty() {
                    "名词检查通过（无禁名命中）".into()
                } else {
                    let before = draft.clone();
                    let nom_skill =
                        load_agent_skill_body(&skill_outcome.skills, "nomenclature-curator");
                    draft = run_nomenclature_rename(
                        &llm,
                        &nom_skill,
                        &project_dir,
                        &draft,
                        &banned,
                        &naming_block,
                        &tx,
                    )
                    .await?;
                    if draft != before {
                        let (shaped, shape_issues) = normalize_draft_best_effort(chapter, &draft);
                        if !shaped.trim().is_empty() {
                            draft = shaped;
                        }
                        write_chapter_draft(&project_dir, chapter, &draft)?;
                        if !shape_issues.is_empty() {
                            has_blocking = true;
                            tracing::warn!(
                                chapter,
                                issues = ?shape_issues,
                                "nomenclature rename left draft shape issues"
                            );
                        }
                        format!("禁名已替换：{}", banned.join("、"))
                    } else {
                        format!("禁名仍可能残留：{}", banned.join("、"))
                    }
                }
            }
            Some(HandlerKind::SpecialistRewrite) => {
                if draft.trim().is_empty() {
                    format!("正文为空，跳过 {agent}")
                } else {
                    let focus = handler_focus
                        .as_deref()
                        .unwrap_or("专改正文，不改情节事实。");
                    let polished = run_specialist_rewrite(
                        &llm,
                        skill_body,
                        agent,
                        focus,
                        &draft,
                        &tx,
                    )
                    .await?;
                    if polished.trim().is_empty() || polished == draft {
                        format!("{agent} 无实质改动")
                    } else {
                        let (shaped, shape_issues) =
                            normalize_draft_best_effort(chapter, &polished);
                        if shaped.trim().is_empty() {
                            format!("{agent} 产出为空，已保留原稿")
                        } else {
                            draft = shaped;
                            write_chapter_draft(&project_dir, chapter, &draft)?;
                            if !shape_issues.is_empty() {
                                has_blocking = true;
                                tracing::warn!(
                                    chapter,
                                    agent,
                                    issues = ?shape_issues,
                                    "specialist rewrite left draft shape issues"
                                );
                            }
                            format!("{agent} 完成（{} 字）", draft.chars().count())
                        }
                    }
                }
            }
            Some(HandlerKind::Consistency) => {
                tracing::info!(chapter, "audit: consistency_auditor start");
                let light = use_layered_consistency_light(
                    config_root,
                    chapter,
                    &outline,
                    revision.verify_previous,
                );
                run.consistency_light = light;
                let profile = if light {
                    tracing::info!(chapter, "audit: layered light consistency context");
                    ContextProfile::Pacing
                } else {
                    ContextProfile::Full
                };
                let canon = rebuild_canon(&draft, &outline, profile);
                let audit_path = project_dir
                    .join("chapters")
                    .join(format!("{chapter:03}"))
                    .join("audit.json");
                let previous_issues = if revision.verify_previous {
                    load_previous_audit_issues(&audit_path)
                } else {
                    vec![]
                };
                let (passed, issues, report) = if revision.verify_previous
                    && !previous_issues.is_empty()
                {
                    let (verdicts, new_raw, report) = run_consistency_verify(
                        &llm,
                        skill_body,
                        &draft,
                        &tx,
                        &canon.markdown,
                        &naming_block,
                        &previous_issues,
                    )
                    .await?;
                    let (passed, merged, fixed_n, new_n) =
                        merge_verify_audit(&previous_issues, &verdicts, new_raw, Some(&draft));
                    tracing::info!(
                        chapter,
                        passed,
                        fixed_n,
                        new_n,
                        open = merged.len(),
                        "audit: verify_previous merged"
                    );
                    let report = format!(
                        "{report}\n\n（复审：已核实上一轮问题；已修复 {fixed_n}，新发现 {new_n}，仍未关闭 {}）",
                        merged.len()
                    );
                    (passed, merged, report)
                } else {
                    let (passed_raw, issues_raw, report) = run_consistency_auditor(
                        &llm,
                        skill_body,
                        &draft,
                        &tx,
                        &canon.markdown,
                        &naming_block,
                    )
                    .await?;
                    let (has_p0, issues) = normalize_consistency_issues(issues_raw);
                    // Publish gate follows hard P0 only. Soft/fake P0s are demoted/dropped
                    // in normalize; model `passed=false` on P1-only must not block forever.
                    let passed = !has_p0;
                    if passed_raw != passed {
                        tracing::info!(
                            chapter,
                            passed_raw,
                            passed,
                            has_p0,
                            issues = issues.len(),
                            "consistency passed recomputed after priority normalize"
                        );
                    }
                    (passed, with_issue_ids(issues), report)
                };
                let has_p0 = issues.iter().any(|i| {
                    i.get("priority").and_then(|v| v.as_str()) == Some("P0")
                });
                tracing::info!(
                    chapter,
                    passed,
                    has_p0,
                    verify = revision.verify_previous,
                    report_chars = report.chars().count(),
                    "audit: consistency_auditor done"
                );
                consistency_passed = passed;
                run.consistency_passed = Some(passed);
                run.issues = issues.clone();
                has_blocking = !passed || has_p0;
                // Persist for steer_run / fail-rate activation.
                if let Some(parent) = audit_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(
                    &audit_path,
                    serde_json::to_string_pretty(&json!({
                        "passed": passed,
                        "issues": issues,
                        "report": report,
                        "verify_previous": revision.verify_previous,
                    }))
                    .unwrap_or_else(|_| "{}".into()),
                );
                if audit_only {
                    // Audit-only never rewrites; on fail, Studio/core opens per-issue decisions.
                    if !passed || has_p0 {
                        await_human = true;
                        has_blocking = true;
                        run.needs_user_choice = true;
                        let p0_n = issues
                            .iter()
                            .filter(|i| {
                                i.get("priority").and_then(|v| v.as_str()) == Some("P0")
                            })
                            .count();
                        emit(
                            &tx,
                            PipelineEvent::AwaitingHuman {
                                prompt: format!(
                                    "第{chapter}章未通过（{p0_n} 条阻断），请按问题选择处理项"
                                ),
                                options: consistency_human_option_labels(&issues),
                            },
                        );
                    }
                } else {
                    let gate = on_consistency_result(passed, &issues, true);
                    match gate.decision {
                        GateDecision::AutoFix if !draft.is_empty() => {
                            emit(
                                &tx,
                                PipelineEvent::AutoFixStarted {
                                    reason: gate.message.clone(),
                                },
                            );
                            let rev = RevisionOptions {
                                prefer_local_patch: true,
                                revision_mode: true,
                                user_instructions: Some("按一致性审计 P0/P1 局部修复".into()),
                                audit_issues: gate.auto_fix_issues,
                                pacing_suggestions: vec![],
                                verify_previous: false,
                                full_rescan: false,
                                defer_soft_long_publish: false,
                            };
                            let fix_canon =
                                rebuild_canon(&draft, &outline, ContextProfile::Full);
                            let (new_draft, via_patch) = run_writer(
                                &llm,
                                skill_body,
                                &state,
                                chapter,
                                &project_dir,
                                &outline,
                                &draft,
                                &rev,
                                true,
                                &tx,
                                &fix_canon.markdown,
                                &naming_block,
                                &chapter_budget,
                                &content_rules,
                            )
                            .await?;
                            let (shaped, shape_issues) =
                                normalize_draft_best_effort(chapter, &new_draft);
                            if !shaped.trim().is_empty() {
                                draft = shaped;
                            } else {
                                draft = new_draft;
                            }
                            write_chapter_draft(&project_dir, chapter, &draft)?;
                            run.revision_applied_via_patch = via_patch;
                            if !shape_issues.is_empty() {
                                tracing::warn!(
                                    chapter,
                                    issues = ?shape_issues,
                                    "consistency AutoFix left draft shape issues"
                                );
                            }
                            // AutoFix 后仍需复审才可发布：保持 has_blocking。
                        }
                        GateDecision::AwaitHuman => {
                            await_human = true;
                            has_blocking = true;
                            run.needs_user_choice = true;
                            let p0_n = issues
                                .iter()
                                .filter(|i| {
                                    i.get("priority").and_then(|v| v.as_str()) == Some("P0")
                                })
                                .count();
                            emit(
                                &tx,
                                PipelineEvent::AwaitingHuman {
                                    prompt: format!(
                                        "第{chapter}章待确认（{p0_n} 条阻断），请按问题选择处理项"
                                    ),
                                    options: consistency_human_option_labels(&issues),
                                },
                            );
                        }
                        GateDecision::Continue => {
                            has_blocking = false;
                            consistency_passed = true;
                            run.consistency_passed = Some(true);
                        }
                        _ => {}
                    }
                }
                let block = format!(
                    "## 一致性审计（第{chapter}章）\n结果：{}\n\n{report}",
                    if passed { "通过" } else { "未通过" }
                );
                report_parts.push(block);
                // Short step label — full report already streamed via LlmDelta.
                if passed {
                    "一致性通过".into()
                } else {
                    "一致性未通过".into()
                }
            }
            Some(HandlerKind::Pacing) => {
                // Verify re-audits focus on prior consistency issues; pacing rediscovery
                // makes the report look like "越审越多".
                if revision.verify_previous && audit_only {
                    "节奏审查跳过（复审模式）".into()
                } else {
                tracing::info!(chapter, "audit: pacing_reviewer start");
                let canon = rebuild_canon(&draft, &outline, ContextProfile::Pacing);
                let (suggestions, report) =
                    run_pacing_reviewer(&llm, skill_body, &draft, &tx, &canon.markdown).await?;
                tracing::info!(
                    chapter,
                    report_chars = report.chars().count(),
                    "audit: pacing_reviewer done"
                );
                if !audit_only {
                    let gate = on_pacing_result(&suggestions, true);
                    if gate.decision == GateDecision::AutoFix {
                        emit(
                            &tx,
                            PipelineEvent::AutoFixStarted {
                                reason: gate.message.clone(),
                            },
                        );
                        let rev = RevisionOptions {
                            prefer_local_patch: true,
                            revision_mode: true,
                            user_instructions: Some("按节奏建议局部调整".into()),
                            audit_issues: vec![],
                            pacing_suggestions: gate.auto_fix_suggestions,
                            verify_previous: false,
                            full_rescan: false,
                            defer_soft_long_publish: false,
                        };
                        let fix_canon = rebuild_canon(&draft, &outline, ContextProfile::Full);
                        let (new_draft, via_patch) = run_writer(
                            &llm,
                            skill_body,
                            &state,
                            chapter,
                            &project_dir,
                            &outline,
                            &draft,
                            &rev,
                            true,
                            &tx,
                            &fix_canon.markdown,
                            &naming_block,
                            &chapter_budget,
                            &content_rules,
                        )
                        .await?;
                        let (shaped, shape_issues) =
                            normalize_draft_best_effort(chapter, &new_draft);
                        if !shaped.trim().is_empty() {
                            draft = shaped;
                        } else {
                            draft = new_draft;
                        }
                        write_chapter_draft(&project_dir, chapter, &draft)?;
                        run.revision_applied_via_patch = via_patch;
                        if !shape_issues.is_empty() {
                            tracing::warn!(
                                chapter,
                                issues = ?shape_issues,
                                "pacing AutoFix left draft shape issues"
                            );
                        }
                    }
                }
                let block = format!("## 节奏审查（第{chapter}章）\n\n{report}");
                report_parts.push(block);
                "节奏审查完成".into()
                }
            }
            Some(HandlerKind::Foreshadow) => {
                // Chapter-local only until publish — hard rules / length / accept run later.
                if has_blocking || await_human {
                    "一致性/门控未通过，已跳过伏笔追踪".into()
                } else {
                    let report = run_foreshadow_tracker(
                        &llm,
                        skill_body,
                        &project_dir,
                        chapter,
                        &draft,
                        &outline,
                    )
                    .await?;
                    write_chapter_memory_artifact(
                        &project_dir,
                        chapter,
                        "foreshadow.json",
                        &report,
                        &draft,
                    )?;
                    report_parts.push(format!("## 伏笔追踪（第{chapter}章）\n\n{report}"));
                    "伏笔追踪完成（待发布后入库）".into()
                }
            }
            Some(HandlerKind::Summarizer) => {
                // Write chapter summary.json only; hot digests / lore assert wait for publish.
                if has_blocking || await_human {
                    "一致性/门控未通过，已跳过摘要".into()
                } else {
                    let summary = run_summarizer(&llm, skill_body, &draft).await?;
                    write_chapter_memory_artifact(
                        &project_dir,
                        chapter,
                        "summary.json",
                        &summary,
                        &draft,
                    )?;
                    ran_summarizer = true;
                    report_parts.push(format!("## 章节摘要（第{chapter}章 · 待发布后入库）"));
                    "摘要已生成（待发布后入库）".into()
                }
            }
            Some(HandlerKind::PlotAccept) => {
                if has_blocking || await_human {
                    "一致性/门控未通过，已跳过剧情验收".into()
                } else {
                    let summary_path = project_dir
                        .join("chapters")
                        .join(format!("{chapter:03}"))
                        .join("summary.json");
                    let summary = std::fs::read_to_string(&summary_path).unwrap_or_default();
                    let verdict = run_plot_acceptor(
                        &llm,
                        skill_body,
                        &project_dir,
                        chapter,
                        &draft,
                        &summary,
                    )
                    .await?;
                    let dir = project_dir.join("chapters").join(format!("{chapter:03}"));
                    std::fs::create_dir_all(&dir)?;
                    std::fs::write(dir.join("plot_accept.json"), &verdict)?;
                    pending_plot_accept = Some(verdict.clone());
                    let v = serde_json::from_str::<serde_json::Value>(&verdict).unwrap_or_default();
                    if v.get("skipped").and_then(|x| x.as_bool()).unwrap_or(false) {
                        let r = v
                            .get("rationale")
                            .and_then(|x| x.as_str())
                            .unwrap_or("已跳过");
                        // Missing exit on the card is a hard schema gap — block publish.
                        // "无进行中剧情卡" is handled by plot write gate earlier; do not soft-skip.
                        if r.contains("缺少收束") {
                            has_blocking = true;
                            run.needs_user_choice = true;
                            run.plot_accept_passed = Some(false);
                            run.plot_accept_rationale = Some(r.to_string());
                            report_parts.push(format!("## 剧情验收（阻塞发布）\n{r}"));
                        }
                        format!("剧情验收：{r}")
                    } else if crate::plots::accept_verdict_is_pass(&verdict) {
                        run.plot_accept_passed = Some(true);
                        "剧情验收通过（待本章发布后 completed）".into()
                    } else if let Some(r) = v.get("rationale").and_then(|x| x.as_str()) {
                        // Multi-chapter cards: early chapters default fail (see plot-acceptor skill).
                        // Incomplete progress must NOT block publish — only pass completes the card.
                        run.plot_accept_passed = Some(false);
                        run.plot_accept_rationale = Some(r.to_string());
                        report_parts.push(format!(
                            "## 剧情验收（未收束·可发布）\n卡仍进行中，缺口：{r}"
                        ));
                        format!("剧情验收未收束（已放行发布）：{r}")
                    } else {
                        run.plot_accept_passed = Some(false);
                        report_parts.push(
                            "## 剧情验收（未收束·可发布）\n卡仍进行中，本章未兑现全部收束条件。"
                                .into(),
                        );
                        "剧情验收：未收束（已放行发布）".into()
                    }
                }
            }
            None => {
                // Unknown / unwired agent: skip — do not burn tokens on empty passthrough.
                tracing::warn!(agent = %agent, chapter, "unwired pipeline agent skipped");
                format!("跳过未接线 Agent：{agent}")
            }
        };

        run.steps[idx].status = "completed".into();
        run.steps[idx].summary = summary.clone();
        emit(
            &tx,
            PipelineEvent::StepCompleted {
                agent: agent.clone(),
                summary: summary.clone(),
            },
        );

        // Audit-only fail: stop before pacing_reviewer so the tool returns and UI can unlock.
        if audit_only && await_human {
            for rest in run.steps.iter_mut().skip(idx + 1) {
                if rest.status == "pending" {
                    rest.status = "skipped".into();
                    rest.summary = "一致性未通过，已跳过".into();
                }
            }
            break;
        }
    }

    // Always scan enabled hard rules (audit + write). Only `blocking: true` blocks publish.
    let mut violations = if !draft.is_empty() {
        check_draft_with(&content_rules, &draft, &naming.forbidden_names)
    } else {
        Vec::new()
    };
    // Light deterministic body-state checks (side flip + ability host locus).
    if !draft.is_empty() {
        for msg in crate::body_state::check_body_state_side_conflicts(&project_dir, chapter, &draft)
        {
            violations.push(ContentRuleViolation {
                rule: "body_state_side".into(),
                message: msg,
                blocking: true,
            });
        }
        for msg in crate::body_state::check_body_state_locus_conflicts(&project_dir, chapter, &draft)
        {
            violations.push(ContentRuleViolation {
                rule: "body_state_locus".into(),
                message: msg,
                blocking: true,
            });
        }
    }
    if !violations.is_empty() {
        let blocking_n = violations.iter().filter(|v| v.blocking).count();
        let warn_n = violations.len() - blocking_n;
        let listed = violations
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let tag = if v.blocking { "阻断" } else { "警告" };
                format!("{}. [{}] {}", i + 1, tag, v.message)
            })
            .collect::<Vec<_>>()
            .join("\n");
        tracing::warn!(
            chapter,
            blocking = blocking_n,
            warnings = warn_n,
            detail = %listed,
            "content rule violations"
        );
                if blocking_n > 0 {
            report_parts.push(format!(
                "## 硬规则（{blocking_n} 条阻断{}，本章未发布）\n{listed}\n\n\
                 如何处理：选择「修正本章」，按上方违规逐条改句。",
                if warn_n > 0 {
                    format!("、{warn_n} 条警告")
                } else {
                    String::new()
                }
            ));
            let warn = format!(
                "完成，但有 {blocking_n} 条硬规则阻断（本章未发布）\n{listed}\n\n\
                 如何处理：选择「修正本章」，按上方违规逐条改句。"
            );
            if run.message.is_empty() {
                run.message = warn;
            } else {
                run.message = format!("{}\n\n{warn}", run.message);
            }
        } else {
            report_parts.push(format!(
                "## 硬规则（{warn_n} 条警告，不阻断发布）\n{listed}"
            ));
            let note = format!("完成，另有 {warn_n} 条硬规则警告（不阻断发布）\n{listed}");
            if run.message.is_empty() {
                run.message = note;
            } else {
                run.message = format!("{}\n\n{note}", run.message);
            }
        }
    }

    // Length budget: hard short blocks; soft short warns; streak may escalate SoftShort.
    // HardLong + pipeline.auto_split_hard_long → split then defer publish for re-audit.
    let mut length_soft_short = false;
    let mut length_ok = false;
    let mut just_auto_split = false;
    if !draft.is_empty() {
        let body_n = draft_body_chars(&draft);
        match chapter_budget.assess_body_chars(body_n) {
            LengthAssessment::HardShort => {
                has_blocking = true;
                run.length_status = "hard_short".into();
                // Surface chapter_next「修正本章」even when consistency passed.
                run.needs_user_choice = true;
                let msg = chapter_budget.hard_short_message(body_n);
                tracing::warn!(chapter, body_chars = body_n, "{msg}");
                report_parts.push(format!("## 字数门控（阻断发布）\n{msg}"));
                if run.message.is_empty() {
                    run.message = format!("完成，但{msg}");
                } else {
                    run.message = format!("{}\n\n{msg}", run.message);
                }
            }
            LengthAssessment::SoftShort => {
                length_soft_short = true;
                let prev_streak =
                    novelx_harness::consecutive_soft_short_from_meta(&state.meta);
                let next_streak = prev_streak.saturating_add(1);
                let threshold = chapter_budget.soft_short_auto_revise_after;
                if threshold > 0 && next_streak >= threshold {
                    has_blocking = true;
                    run.length_status = "soft_short_escalated".into();
                    let msg =
                        chapter_budget.soft_short_escalate_message(body_n, next_streak);
                    tracing::warn!(chapter, body_chars = body_n, streak = next_streak, "{msg}");
                    report_parts.push(format!("## 字数门控（连续偏短·阻断发布）\n{msg}"));
                    if run.message.is_empty() {
                        run.message = format!("完成，但{msg}");
                    } else {
                        run.message = format!("{}\n\n{msg}", run.message);
                    }
                    run.needs_user_choice = true;
                } else {
                    run.length_status = "soft_short".into();
                    let msg = chapter_budget.soft_short_message(body_n);
                    tracing::info!(
                        chapter,
                        body_chars = body_n,
                        streak = next_streak,
                        "{msg}"
                    );
                    report_parts.push(format!(
                        "## 字数门控（软警告 · 连续偏短 {next_streak}/{}）\n{msg}",
                        if threshold == 0 { 0 } else { threshold }
                    ));
                    if run.message.is_empty() {
                        run.message = format!("完成，另有{msg}");
                    } else {
                        run.message = format!("{}\n\n{msg}", run.message);
                    }
                }
            }
            LengthAssessment::SoftLong => {
                // Soft long: warn but allow publish; resets soft-short streak.
                length_ok = true;
                run.length_status = "soft_long".into();
                let msg = chapter_budget.soft_long_message(body_n);
                tracing::info!(chapter, body_chars = body_n, "{msg}");
                report_parts.push(format!("## 字数门控（软警告）\n{msg}"));
                if run.message.is_empty() {
                    run.message = format!("完成，另有{msg}");
                } else {
                    run.message = format!("{}\n\n{msg}", run.message);
                }
            }
            LengthAssessment::HardLong => {
                let mut handled = false;
                if !short_drama && auto_split_hard_long_enabled(config_root) {
                    match crate::split_chapter::split_chapter_draft(
                        &project_dir,
                        chapter,
                        0.5,
                        None,
                        false,
                    ) {
                        Ok(split) => {
                            if let Some(new_draft) = read_chapter_draft(&project_dir, chapter) {
                                draft = new_draft;
                                // Re-scan hard rules on the shortened half.
                                violations = check_draft_with(
                                    &content_rules,
                                    &draft,
                                    &naming.forbidden_names,
                                );
                                for msg in crate::body_state::check_body_state_side_conflicts(
                                    &project_dir,
                                    chapter,
                                    &draft,
                                ) {
                                    violations.push(ContentRuleViolation {
                                        rule: "body_state_side".into(),
                                        message: msg,
                                        blocking: true,
                                    });
                                }
                                for msg in crate::body_state::check_body_state_locus_conflicts(
                                    &project_dir,
                                    chapter,
                                    &draft,
                                ) {
                                    violations.push(ContentRuleViolation {
                                        rule: "body_state_locus".into(),
                                        message: msg,
                                        blocking: true,
                                    });
                                }
                                let body_after = draft_body_chars(&draft);
                                run.auto_split = true;
                                run.auto_split_to = Some(split.chapter_b);
                                just_auto_split = true;
                                let note = format!(
                                    "正文超长（{body_n} 字），已自动拆成第{}章（{}字）+ 第{}章（{}字）；将对前半复审后再发布",
                                    split.chapter_a,
                                    split.chars_a,
                                    split.chapter_b,
                                    split.chars_b
                                );
                                tracing::info!(
                                    chapter_a = split.chapter_a,
                                    chapter_b = split.chapter_b,
                                    chars_a = split.chars_a,
                                    chars_b = split.chars_b,
                                    "auto-split hard_long chapter"
                                );
                                report_parts.push(format!("## 字数门控（自动拆章）\n{note}"));
                                if run.message.is_empty() {
                                    run.message = note.clone();
                                } else {
                                    run.message = format!("{}\n\n{note}", run.message);
                                }
                                match chapter_budget.assess_body_chars(body_after) {
                                    LengthAssessment::Ok => {
                                        length_ok = true;
                                        run.length_status = "ok".into();
                                        handled = true;
                                    }
                                    LengthAssessment::SoftLong => {
                                        length_ok = true;
                                        run.length_status = "soft_long".into();
                                        handled = true;
                                    }
                                    LengthAssessment::SoftShort => {
                                        length_soft_short = true;
                                        run.length_status = "soft_short".into();
                                        handled = true;
                                    }
                                    LengthAssessment::HardShort => {
                                        has_blocking = true;
                                        run.length_status = "hard_short".into();
                                        run.needs_user_choice = true;
                                        just_auto_split = false;
                                        let msg = chapter_budget.hard_short_message(body_after);
                                        report_parts.push(format!(
                                            "## 字数门控（拆章后偏短·阻断）\n{msg}"
                                        ));
                                        handled = true;
                                    }
                                    LengthAssessment::HardLong => {
                                        just_auto_split = false;
                                        run.auto_split = false;
                                        run.auto_split_to = None;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(chapter, error = %e, "auto-split hard_long failed");
                            report_parts.push(format!(
                                "## 字数门控（自动拆章失败）\n{e}；请人工拆章或压缩。"
                            ));
                        }
                    }
                }
                if !handled {
                    has_blocking = true;
                    run.length_status = "hard_long".into();
                    run.needs_user_choice = true;
                    let msg = chapter_budget.hard_long_message(body_n);
                    tracing::warn!(chapter, body_chars = body_n, "{msg}");
                    report_parts.push(format!("## 字数门控（严重超长·阻断发布）\n{msg}"));
                    if run.message.is_empty() {
                        run.message = format!("完成，但{msg}");
                    } else {
                        run.message = format!("{}\n\n{msg}", run.message);
                    }
                }
            }
            LengthAssessment::Ok => {
                length_ok = true;
                run.length_status = "ok".into();
            }
        }
    }

    // Final shape gate: specialists / AutoFix may drop title — coerce then hard-block.
    if allow_publish && !draft.trim().is_empty() {
        let (shaped, shape_issues) = if short_drama {
            normalize_script_best_effort(chapter, &draft, &script_shape)
        } else {
            normalize_draft_best_effort(chapter, &draft)
        };
        if !shaped.trim().is_empty() && shaped != draft {
            draft = shaped;
            let _ = write_chapter_draft(&project_dir, chapter, &draft);
        }
        let shape_err = if short_drama {
            validate_script(chapter, &draft, &script_shape).err()
        } else {
            validate_draft(chapter, &draft).err()
        };
        if let Some(e) = shape_err {
            has_blocking = true;
            run.needs_user_choice = true;
            let label = if short_drama { "剧本" } else { "正文" };
            let msg = format!("{label}形状不合规，已拦发布：{}", e.message);
            tracing::warn!(chapter, error = %e.message, "pre-publish shape validate failed");
            report_parts.push(format!("## {label}形状门控（阻断发布）\n{msg}"));
            if !shape_issues.is_empty() {
                report_parts.push(format!("残留问题：{}", shape_issues.join("；")));
            }
            if run.message.is_empty() {
                run.message = msg;
            } else {
                run.message = format!("{}\n\n{msg}", run.message);
            }
        }
    }

    let banned_blocking = has_blocking_violation(&violations);
    run.content_rule_blocked = banned_blocking;
    run.content_rule_violations = violations.clone();
    // Batch SoftLong: warn-only product semantics, but hold publish once for compress.
    let defer_soft_long =
        revision.defer_soft_long_publish && run.length_status == "soft_long";
    if defer_soft_long {
        report_parts.push(
            "## 字数门控（批写暂缓发布）\n正文字数偏长（SoftLong）：先自动压缩一轮再发布。"
                .into(),
        );
    }
    // After auto-split, body changed — defer publish so caller re-audits part A.
    let publish_ok = allow_publish
        && !just_auto_split
        && !defer_soft_long
        && should_publish(consistency_passed, has_blocking || await_human)
        && !banned_blocking;

    if !allow_publish {
        tracing::debug!(chapter, "skip publish finalize (single-step / no-publish run)");
    } else if defer_soft_long {
        // finalize_chapter_publish ignores SoftLong defer — skip it so batch can compress.
        run.published = false;
        tracing::info!(
            chapter,
            body_status = %run.length_status,
            "defer SoftLong publish for batch compress"
        );
    } else if !draft.is_empty() {
        // AuditOnly must also publish when consistency now passes (write failed gate,
        // then revise → reaudit). Previously only Continue advanced counters, leaving
        // UI stuck on「已写 N 章」while chapter N+1 draft/audit already existed.
        let before_published = state.published_count;
        let fin = finalize_chapter_publish(
            &project_dir,
            config_root,
            chapter,
            mode,
            &mut state,
            consistency_passed,
            has_blocking,
            await_human,
            banned_blocking,
            ran_summarizer,
            pending_plot_accept.as_deref(),
            llm.clone(),
            &tx,
        )
        .await?;
        report_parts.extend(fin.report_parts);
        run.published = fin.published;
        run.plots_completed = fin.plots_completed;
        run.plot_setting_blocker = fin.plot_setting_blocker;
        // Keep handler-set accept fail when publish was blocked (finalize returns early).
        if fin.plot_accept_passed.is_some() {
            run.plot_accept_passed = fin.plot_accept_passed;
        }
        if fin.plot_accept_rationale.is_some() {
            run.plot_accept_rationale = fin.plot_accept_rationale;
        }
        // Track consecutive SoftShort for longform length discipline.
        if fin.published {
            if length_ok {
                novelx_harness::set_consecutive_soft_short(&mut state.meta, 0);
                let _ = save_project_state(&project_dir, &state);
            } else if length_soft_short {
                let n = novelx_harness::consecutive_soft_short_from_meta(&state.meta)
                    .saturating_add(1);
                novelx_harness::set_consecutive_soft_short(&mut state.meta, n);
                let _ = save_project_state(&project_dir, &state);
            }
        }
        if fin.await_human {
            await_human = true;
        }
        if let Some(vi) = fin.volume_ended {
            run.volume_ended = Some(vi);
            run.volume_ended_name = fin.volume_ended_name;
            run.volume_ended_start = fin.volume_ended_start;
            run.volume_ended_end = fin.volume_ended_end;
            run.needs_user_choice = true;
        }
        if audit_only && state.published_count > before_published {
            report_parts.push(format!(
                "## 发布\n复审通过，已计入已写章节（published_count → {}，next_chapter → {}）。",
                state.published_count, state.next_chapter
            ));
        }
        if !fin.published && matches!(mode, RunMode::Continue) {
            report_parts.push(
                "## 发布门控\n未推进 next_chapter：一致性未通过、存在 P0/待确认，或禁名未清。可 audit 复审或 steer_run。"
                    .into(),
            );
        }
        // Content-rule block with clean consistency: ask user to revise (not "继续创作").
        if !fin.published && banned_blocking && consistency_passed && !await_human {
            run.needs_user_choice = true;
        }
    }

    run.status = if await_human {
        "awaiting_human".into()
    } else {
        "completed".into()
    };
    if await_human {
        run.needs_user_choice = true;
    }
    run.report = report_parts.join("\n\n---\n\n");
    // Per-agent cost visibility (approx): log each step, then a pipeline rollup.
    {
        use crate::cost_log::{append_cost_entry, approx_tokens_from_chars, CostEntry};
        let draft_chars = draft.chars().count();
        let total_ms = pipeline_started.elapsed().as_millis() as u64;
        let step_n = run.steps.len().max(1) as u64;
        let per_step_ms = total_ms / step_n;
        let mut sum_tok = 0u32;
        for step in &run.steps {
            let summary_chars = step.summary.chars().count();
            let base = if step.agent == "writer" || step.agent.contains("writer") {
                draft_chars.saturating_add(summary_chars)
            } else if step.agent == "lore_librarian" {
                // Deterministic query — tiny.
                summary_chars.max(64)
            } else {
                // Context in + report out (rough).
                summary_chars.saturating_add(1200)
            };
            let tok = approx_tokens_from_chars(base);
            sum_tok = sum_tok.saturating_add(tok);
            let _ = append_cost_entry(
                &project_dir,
                &CostEntry {
                    chapter,
                    agent: step.agent.clone(),
                    approx_tokens: tok,
                    elapsed_ms: per_step_ms,
                    note: step.status.clone(),
                },
            );
        }
        let _ = append_cost_entry(
            &project_dir,
            &CostEntry {
                chapter,
                agent: "pipeline".into(),
                approx_tokens: sum_tok,
                elapsed_ms: total_ms,
                note: format!("steps={} (sum of agent rows)", run.steps.len()),
            },
        );
    }
    if audit_only {
        let base = if run.report.is_empty() {
            format!("第{chapter}章审校完成（无报告正文）")
        } else {
            format!("第{chapter}章审校报告\n\n{}", run.report)
        };
        // Options are shown in the chat UI — avoid duplicating the menu in tool output.
        run.message = if run.content_rule_blocked && run.consistency_passed != Some(false) {
            format!("{base}\n\n（硬规则未通过 — 请在下方选择修正本章）")
        } else if run.needs_user_choice {
            format!("{base}\n\n（审校未通过 — 请在下方选项中选择下一步）")
        } else {
            base
        };
    } else {
        if run.message.is_empty() {
            let gate_note = if !publish_ok && matches!(mode, RunMode::Continue) {
                "（未发布：待修/待确认）"
            } else if run.revision_applied_via_patch {
                "（局部补丁）"
            } else {
                ""
            };
            run.message = format!("第{chapter}章流水线完成{gate_note}");
        }
        // Always surface report sections in tool output. Previously this only ran when
        // message was empty — hard-rule warnings set message first and hid ## 硬规则 details.
        if !run.report.is_empty() {
            let msg_has_rules = run.message.contains("## 硬规则")
                || run.message.contains("条硬规则警告");
            let report_only_rules = run.report.starts_with("## 硬规则")
                && !run.report.contains("\n\n---\n\n");
            if !(msg_has_rules && report_only_rules) && !run.message.contains(&run.report) {
                // If message already listed hard-rule lines, still append other sections.
                if msg_has_rules && run.report.contains("## 硬规则") {
                    let rest = run
                        .report
                        .split("\n\n---\n\n")
                        .filter(|part| !part.trim_start().starts_with("## 硬规则"))
                        .collect::<Vec<_>>()
                        .join("\n\n---\n\n");
                    if !rest.trim().is_empty() {
                        run.message = format!("{}\n\n{}", run.message, rest);
                    }
                } else {
                    run.message = format!("{}\n\n{}", run.message, run.report);
                }
            }
        }
    }

    // Surface non-pipeline activation (e.g. volume_auditor) in tool output for Studio.
    let state_now = load_project_state(&project_dir).unwrap_or(state);
    let hints = crate::activation_hints::collect_studio_activation_hints(
        config_root,
        &project_dir,
        &state_now,
        chapter,
        &draft,
        project,
    );
    if let Some(block) = crate::activation_hints::format_studio_activation_hints_block(&hints) {
        if !run.message.contains("## 长程 QA 建议") {
            run.message = format!("{}\n\n{block}", run.message);
        }
        if !run.report.contains("## 长程 QA 建议") {
            if run.report.is_empty() {
                run.report = block;
            } else {
                run.report = format!("{}\n\n---\n\n{block}", run.report);
            }
        }
    }

    emit(
        &tx,
        PipelineEvent::RunCompleted {
            run_id: run_id.clone(),
            message: run.message.clone(),
        },
    );
    Ok(run)
}

async fn run_chapter_planner(
    llm: &LlmClient,
    skill: &str,
    state: &ProjectState,
    chapter: u32,
    canon: &str,
    naming_block: &str,
) -> Result<String> {
    // Must match chapters/NNN/outline.json schema (not Markdown — that caused empty-parse failures).
    let user = format!(
        "为小说《{}》（题材：{}）撰写第{chapter}章章纲。\n\
         只输出一个 JSON 对象（可包在 ```json 代码块中），不要输出 Markdown 散文或解释。\n\
         必填字段：title, pov, time_location, goal, conflict, emotion_curve,\n\
         plot_includes(数组), plot_defers(数组),\n\
         key_events(数组 2–4 条), characters(数组), items(数组), locations(数组),\n\
         scene_tags(数组), cliffhanger, lore_queries(数组)。\n\
         篇幅预算：plot_includes 合计按可写成 5000–6000 字估量（通常 1 主推进+至多 1 加压/揭示）；\n\
         key_events 只展开 includes，禁止 5 条及以上。\n\
         若有进行中剧情卡：只切片本章场面，禁止把卡的完整走向/收束一次写进本章；\n\
         plot_defers 至少 1 条（含「本章不兑现收束条件」类声明，除非 includes 显式触及落点）。\n\
         items/locations：本章要用的物品卡/地点卡规范名（可空数组）；系统据此加载设定卡。\n\
         JSON 硬约束：字符串内禁止未转义的英文双引号；对话/强调请用「」或『』；不要尾逗号。\n\
         必须与 CanonContext 卷幕目标、未收线与剧情卡一致，不得引入未定义设定。\n\
         若有【设定缺口】：lore_queries 优先覆盖缺口实体；勿另起缺口外新主要实体；\
         勿安排 status=exited/consumed 角色/物品常规出场（闪回须注明）。\n\
         人物/组织命名遵守取名硬约束。\n\n{naming_block}\n\n{canon}",
        state.name, state.genre
    );
    llm.complete_for_agent("chapter_planner", skill, &user).await
}

/// One-shot repair when planner JSON fails to parse / validate.
async fn run_chapter_planner_repair(
    llm: &LlmClient,
    skill: &str,
    chapter: u32,
    broken: &str,
    error: &str,
) -> Result<String> {
    let user = format!(
        "第{chapter}章章纲 JSON 无法解析或不合预算，请输出修正后的完整 JSON 对象（可包在 ```json 中），不要解释。\n\
         错误：{error}\n\
         要求：保留原情节要点；字符串内不要用未转义英文双引号（改用「」）；去掉尾逗号；\
         字段齐全：title/pov/time_location/goal/conflict/emotion_curve/\
         plot_includes/plot_defers/key_events(2–4 条)/\
         characters/items/locations/scene_tags/cliffhanger/lore_queries；\
         key_events 不得超过 4 条；有进行中剧情卡时 plot_defers 至少 1 条。\n\n\
         # 待修正原文\n{broken}"
    );
    llm.complete_for_agent("chapter_planner", skill, &user).await
}

/// Light LLM pass: fix draft Markdown shape only (title / fence / min length). Keep plot.
async fn run_draft_shape_fix(
    llm: &LlmClient,
    chapter: u32,
    draft: &str,
    issues: &[String],
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<String> {
    let skill = "你是正文形状修正器。只修正 Markdown 形状，保留全部情节与措辞；不要改写故事。";
    let issue_block = if issues.is_empty() {
        "（未列出具体问题，请按下方要求自检）".into()
    } else {
        issues
            .iter()
            .map(|i| format!("- {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let user = format!(
        "第{chapter}章正文不合 schema，请只输出修正后的完整 Markdown（不要解释）。\n\
         硬性要求：\n\
         1. 首行必须是 `# 第{chapter}章 标题`（阿拉伯数字章号，禁止「第一章」）\n\
         2. 不要用 ``` 代码块包裹全文\n\
         3. 标题行之后尽量保留原文；若篇幅不足 {MIN_DRAFT_BODY_CHARS} 字，在章末自然补足，勿删已有情节\n\
         4. 禁止输出创作说明、JSON、章纲\n\n\
         # 问题\n{issue_block}\n\n# 原文\n{draft}"
    );
    // Use writer model so max_tokens can cover a full chapter if length must be topped up.
    let model = llm.model_for_agent("writer");
    stream_agent_llm(llm, skill, &user, &model, "writer", tx, true, None).await
}

async fn run_writer(
    llm: &LlmClient,
    skill: &str,
    state: &ProjectState,
    chapter: u32,
    project_dir: &Path,
    outline: &str,
    existing_draft: &str,
    revision: &RevisionOptions,
    prefer_local: bool,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
    naming_block: &str,
    budget: &ChapterBudget,
    content_rules: &ContentRulesConfig,
) -> Result<(String, bool)> {
    run_writer_ex(
        llm,
        skill,
        state,
        chapter,
        project_dir,
        outline,
        existing_draft,
        revision,
        prefer_local,
        tx,
        canon,
        naming_block,
        budget,
        content_rules,
        true,
    )
    .await
}

async fn run_writer_ex(
    llm: &LlmClient,
    skill: &str,
    state: &ProjectState,
    chapter: u32,
    project_dir: &Path,
    outline: &str,
    existing_draft: &str,
    revision: &RevisionOptions,
    prefer_local: bool,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
    naming_block: &str,
    budget: &ChapterBudget,
    content_rules: &ContentRulesConfig,
    flush_draft: bool,
) -> Result<(String, bool)> {
    let revision_mode = revision.revision_mode
        || !existing_draft.is_empty()
            && (prefer_local || revision.user_instructions.is_some() || !revision.audit_issues.is_empty());

    // User asked for expansion/rewrite → skip local patch entirely.
    // TIMELINE P0 needs whole-chapter monotonic clocks — local multi-span patches
    // repeatedly fail and loop; escalate to full revise.
    // Only trust priorities after soft-P0 demotion (e.g. 「不构成硬性回跳」).
    let (_, audit_normalized) =
        novelx_harness::normalize_consistency_issues(revision.audit_issues.clone());
    let timeline_p0 = has_timeline_p0(&audit_normalized);
    let force_full = revision
        .user_instructions
        .as_deref()
        .map(needs_full_rewrite)
        .unwrap_or(false)
        || timeline_p0;
    if force_full && timeline_p0 {
        if let Some(tx) = tx {
            let _ = tx.send(PipelineEvent::LlmDelta {
                agent: "writer".into(),
                delta: "（检测到时间线 P0：改为整章修订以统一倒计时）\n".into(),
            });
        }
    }

    if revision_mode && prefer_local && !force_full && !existing_draft.is_empty() {
        let (targets, ok) = plan_revision(
            existing_draft,
            revision.user_instructions.as_deref(),
            &revision.audit_issues,
            &revision.pacing_suggestions,
        );
        if ok && !targets.is_empty() {
            if let Some(tx) = tx {
                let _ = tx.send(PipelineEvent::LlmDelta {
                    agent: "local_reviser".into(),
                    delta: format!(
                        "（局部修订 {} 处：{}）\n",
                        targets.len(),
                        targets
                            .iter()
                            .take(6)
                            .map(|t| t.para_label())
                            .collect::<Vec<_>>()
                            .join("、")
                    ),
                });
            }
            match revise_by_local_patches(
                llm,
                skill,
                existing_draft,
                &targets,
                tx,
                canon,
                naming_block,
            )
            .await
            {
                Ok((draft, true)) => return Ok((scrub_writer_draft(draft, content_rules), true)),
                Ok((draft, false)) if !draft.is_empty() => {
                    // partial — still better than full if we got something; fall through only if empty
                    if draft != existing_draft {
                        return Ok((scrub_writer_draft(draft, content_rules), true));
                    }
                }
                _ => {}
            }
        }
        if let Some(tx) = tx {
            let _ = tx.send(PipelineEvent::LlmDelta {
                agent: "local_reviser".into(),
                delta: "（局部补丁未落地，改为整章修订…）\n".into(),
            });
        }
        // full fallback — inject concrete audit issues so rewrite is not blind
        let enriched = enrich_revision_with_issues(revision);
        let draft = revise_full(
            llm,
            skill,
            state,
            chapter,
            project_dir,
            outline,
            existing_draft,
            &enriched,
            canon,
            naming_block,
            budget,
            tx,
            content_rules,
            flush_draft,
        )
        .await?;
        return Ok((draft, false));
    }

    if !existing_draft.is_empty() && revision_mode {
        let enriched = enrich_revision_with_issues(revision);
        let draft = revise_full(
            llm,
            skill,
            state,
            chapter,
            project_dir,
            outline,
            existing_draft,
            &enriched,
            canon,
            naming_block,
            budget,
            tx,
            content_rules,
            flush_draft,
        )
        .await?;
        return Ok((draft, false));
    }

    // new chapter generation — stream so UI doesn't look stuck
    let mode_key = crate::project::resolve_project_mode(project_dir).as_str();
    let short = crate::project::is_short_drama(project_dir);
    let target_line = budget.writer_target_line_for_mode(mode_key);
    let unit_label = if short { "集" } else { "章" };
    let outline_label = if short { "集纲" } else { "章纲" };
    let empty_outline = if short {
        "（无集纲，按题材与 CanonContext 规划一集漫剧剧本）"
    } else {
        "（无章纲，按题材与 CanonContext 规划一章）"
    };
    let user = format!(
        "作品《{}》第{chapter}{unit_label}。\n\
         正文必须服从 CanonContext（尤其【身体与能力状态板】、人物状态、名词、世界观、剧情走向）。\n\
         写前先扫一眼状态板：伤势侧别/部位与能力寄宿点抄错即属 P0。\n\
         命名遵守取名硬约束，禁止语料脸谱名。\n\
         {WRITER_HARD_CONSTRAINTS}\n\n\
         {naming_block}\n\n{canon}\n\n# {outline_label}\n{}\n\n{target_line}",
        state.name,
        if outline.is_empty() { empty_outline } else { outline }
    );
    let writer_agent = if short { "script_writer" } else { "writer" };
    let model = llm.model_for_agent(writer_agent);
    // Codex-like: stream tool output deltas (coalesced in core) + intermittent draft.md flush.
    let draft = stream_agent_llm(
        llm,
        skill,
        &user,
        &model,
        writer_agent,
        tx,
        true,
        Some(DraftFlushSink {
            project_dir: project_dir.to_path_buf(),
            chapter,
        }),
    )
    .await?;
    Ok((scrub_writer_draft(draft, content_rules), false))
}

/// Hard constraints duplicated into the user message (skills alone are easy to ignore).
const WRITER_HARD_CONSTRAINTS: &str = "\
硬约束（违反会被系统拦截或一致性 P0 打回）：\n\
1. 章号元叙述：除首行标题 `# 第N章 …` 外，叙述/独白/对话禁止出现「第N章」「第八章」等连载章号；\
回溯用故事内时间/事件（「上次会面时」「那次事故之后」）。\n\
2. 明确时间锚：若写钟点/倒计时/还剩时长，全章只维护一条当前读数且单调（倒计时只减不增）；\
具体读数宜 ≤4 次；禁止假回跳；回忆初始值须写「最初/原先」。\n\
3. 时段词：凌晨/清晨/上午/傍晚/夜里/夜晚/深夜等会被当成叙事时刻扫描；全章只沿一条时段线前进；\
回到更早时段必须交代跨日（翌日/天亮/过了一夜）。\
**禁止把时段词当修辞**：当前是凌晨时勿写「……的夜晚/夜里」；氛围改用「黑暗/夜色/未亮的天」。\n\
4. 伤势侧别/部位：若 CanonContext 有【身体与能力状态板】或近章事实——脑中锁定侧别/部位；\
禁止无交代左右对调或肩臂挪移。承接默认用症状/动作限制（颤抖、冷汗、握力发虚），\
非分侧剧情勿反复点名「左/右」；中性「手/指尖」不算挪伤。首发或变更须写清侧别与可见过程。\n\
5. 能力寄宿/附着/载体：状态板与近章事实中的所在肢体/器物/印记点必须沿用；\
更换须写可见转移过程，禁止默默换位。\n\
6. 作者旁白/管线泄露（绝对禁止）：正文禁止「章纲里/章纲写/按大纲/设定上」等对读；\
禁止「不是A，是B的意思」类纠错释义；禁止机制说明书腔。写前对齐章纲，正文只留场面。";

/// Strip accidental body「第N章」right after generation (title line kept).
fn scrub_writer_draft(draft: String, content_rules: &ContentRulesConfig) -> String {
    novelx_harness::rewrite_meta_chapter_refs_with(content_rules, &draft).unwrap_or(draft)
}

const LOCAL_REVISER_SKILL: &str = "你是小说局部修订编辑。只按用户指令改指定段落；\
输出替换正文或要求的 JSON，不要写作说明，不要扩写全章。\
涉及倒计时/钟点时：只保留一条单调当前读数，禁止回跳；回忆初始值须标明「最初/原先」，勿伪装成当前值。\
涉及时段词时：勿把「夜晚/夜里」当修辞与「凌晨」混用；冲突时段须理顺或补跨日。\
涉及伤势/能力位置时：保持寄宿/载体与改前及 CanonContext 状态板一致，禁止默默挪位；\
伤势承接优先改为症状/动作限制句，非分侧剧情勿堆砌左右；点名侧别时不得写反侧。\
禁止写入章纲对读、作者纠错释义、机制讲义。";

async fn revise_by_local_patches(
    llm: &LlmClient,
    _skill: &str,
    draft: &str,
    targets: &[RevisionTarget],
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
    naming_block: &str,
) -> Result<(String, bool)> {
    let (draft_cur, applied) =
        revise_by_local_patches_collect(llm, draft, targets, tx, canon, naming_block).await?;
    Ok((draft_cur, !applied.is_empty()))
}

/// Local revise that also returns applied before/after spans (for confirm preview).
async fn revise_by_local_patches_collect(
    llm: &LlmClient,
    draft: &str,
    targets: &[RevisionTarget],
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
    naming_block: &str,
) -> Result<(String, Vec<novelx_draft_patch::AppliedPatch>)> {
    let skill = LOCAL_REVISER_SKILL;
    let canon_short = crate::cards::truncate_chars(canon, 1800);
    let naming_short = crate::cards::truncate_chars(naming_block, 600);
    let mut draft_cur = draft.to_string();
    let mut applied_all = Vec::new();

    for target in targets {
        if target.source == "timeline_batch" {
            let (next, batch_applied) = revise_timeline_batch_collect(
                llm,
                skill,
                &draft_cur,
                target,
                tx,
                &canon_short,
                &naming_short,
            )
            .await?;
            if !batch_applied.is_empty() {
                draft_cur = next;
                applied_all.extend(batch_applied);
            }
            continue;
        }

        let paragraphs = segment_paragraphs(&draft_cur);
        let slice = slice_span(&paragraphs, target.start, target.end);
        let user = format!(
            "只改下列段落，输出替换后的段落全文（不要输出其它说明）。\n\
             修改不得违反 CanonContext 设定，且遵守取名硬约束。\n\n\
             {naming_short}\n\n{canon_short}\n\n指令：{}\n定位：{}\n\n# 原段落\n{}",
            target.instruction,
            target.para_label(),
            slice
        );
        let repl = llm.complete_for_agent("local_reviser", skill, &user).await?;
        // Reject oversized outputs (likely full chapter)
        if repl.chars().count() > slice.chars().count().saturating_mul(3).max(2000) {
            continue;
        }
        let one = [target.clone()];
        let result = apply_local_patches(&draft_cur, &one, &[(0, repl.clone())]);
        if result.used_local_patch {
            draft_cur = result.draft;
            for ap in &result.applied {
                if let Some(tx) = tx {
                    let _ = tx.send(PipelineEvent::LlmDelta {
                        agent: "local_reviser".into(),
                        delta: format!("✓ 已改 {}\n", target.para_label()),
                    });
                    let _ = tx.send(PipelineEvent::DraftPatched {
                        start_para: ap.start + 1,
                        end_para: ap.end + 1,
                        before: ap.before.chars().take(2000).collect(),
                        after: ap.after.chars().take(2000).collect(),
                    });
                }
            }
            applied_all.extend(result.applied);
        }
    }

    Ok((draft_cur, applied_all))
}

/// One LLM call for all discrete time-anchor paragraphs (avoids 8× serial patch latency).
async fn revise_timeline_batch_collect(
    llm: &LlmClient,
    skill: &str,
    draft: &str,
    target: &RevisionTarget,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon_short: &str,
    naming_short: &str,
) -> Result<(String, Vec<novelx_draft_patch::AppliedPatch>)> {
    let paragraphs = segment_paragraphs(draft);
    let idxs = target.batch_para_indices();
    if idxs.is_empty() {
        return Ok((draft.to_string(), vec![]));
    }
    if let Some(tx) = tx {
        let _ = tx.send(PipelineEvent::LlmDelta {
            agent: "local_reviser".into(),
            delta: format!("（批量修订 {}）\n", target.para_label()),
        });
    }
    let mut blocks = String::new();
    for &idx in &idxs {
        if idx >= paragraphs.len() {
            continue;
        }
        blocks.push_str(&format!(
            "### 第{}段\n{}\n\n",
            idx + 1,
            paragraphs[idx]
        ));
    }
    let user = format!(
        "统一修订下列含倒计时/钟点的段落，消除时间互斥。\n\
         指令：{}\n\
         规则：只改钟点/倒计时/子夜相关表述，其余句子尽量不动；\
         全章只维护一条当前读数且单调（倒计时只减不增）；禁止无故回跳；\
         禁止重复写出已出现过的更早读数当作当前值；\
         回忆总额/初始值必须写成「最初是…」；尽量删去多余的 A→B→C 递减串，只留当前值。\n\
         只输出 JSON（不要 markdown）：{{\"patches\":[{{\"para\":段号从1起,\"text\":\"该段全文\"}}]}}\n\
         每个列出的段都必须给出补丁。\n\n\
         {naming_short}\n\n{canon_short}\n\n# 待改段落\n{blocks}",
        target.instruction
    );
    let raw = llm.complete_for_agent("local_reviser", skill, &user).await?;
    let Some(v) = extract_json(&raw) else {
        tracing::warn!("timeline_batch: no JSON in model output");
        return Ok((draft.to_string(), vec![]));
    };
    let Some(arr) = v.get("patches").and_then(|x| x.as_array()) else {
        tracing::warn!("timeline_batch: missing patches array");
        return Ok((draft.to_string(), vec![]));
    };

    let mut sub_targets = Vec::new();
    let mut sub_repls = Vec::new();
    for item in arr {
        let para_1 = item
            .get("para")
            .and_then(|x| x.as_u64())
            .or_else(|| {
                item.get("para")
                    .and_then(|x| x.as_str())
                    .and_then(|s| s.parse().ok())
            })
            .unwrap_or(0) as usize;
        if para_1 == 0 {
            continue;
        }
        let idx = para_1 - 1;
        if !idxs.contains(&idx) || idx >= paragraphs.len() {
            continue;
        }
        let text = item
            .get("text")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let orig_len = paragraphs[idx].chars().count();
        if text.chars().count() > orig_len.saturating_mul(3).max(2000) {
            continue;
        }
        let j = sub_targets.len();
        sub_targets.push(RevisionTarget {
            instruction: target.instruction.clone(),
            start: idx,
            end: idx,
            quote: String::new(),
            source: "timeline_batch".into(),
        });
        sub_repls.push((j, text.to_string()));
    }
    if sub_targets.is_empty() {
        return Ok((draft.to_string(), vec![]));
    }
    let result = apply_local_patches(draft, &sub_targets, &sub_repls);
    if result.used_local_patch {
        if let Some(tx) = tx {
            let _ = tx.send(PipelineEvent::LlmDelta {
                agent: "local_reviser".into(),
                delta: format!("✓ 已改 {}（{} 段）\n", target.para_label(), sub_targets.len()),
            });
        }
    }
    Ok((result.draft, result.applied))
}

fn enrich_revision_with_issues(revision: &RevisionOptions) -> RevisionOptions {
    let mut out = revision.clone();
    let brief = format_audit_issues_brief(&revision.audit_issues);
    if brief.is_empty() {
        return out;
    }
    let base = out
        .user_instructions
        .as_deref()
        .unwrap_or("按审校意见修订")
        .trim();
    // Avoid duplicating the same brief if already present.
    if base.contains("必须逐条修复下列审校问题") {
        return out;
    }
    out.user_instructions = Some(if base.is_empty() {
        brief
    } else {
        format!("{base}\n\n{brief}")
    });
    out
}

/// Length constraints appended to full-chapter revise user prompts.
pub(crate) fn revise_full_length_note(
    instr: &str,
    audit_issues: &[Value],
    budget: &ChapterBudget,
) -> String {
    let (_, normalized) = novelx_harness::normalize_consistency_issues(audit_issues.to_vec());
    if needs_full_rewrite(instr) || has_timeline_p0(&normalized) {
        format!(
            "\n{}硬上限 {} 字（不得超过）；修一致性/时间线时禁止借机扩写超上限。\
不得只改几句或原样返回短稿；输出完整正文 Markdown。",
            budget.revise_length_hint(),
            budget.word_max
        )
    } else {
        format!(
            "\n输出修订后的完整正文 Markdown；篇幅宜在 {} 字内，不得超过 {} 字。",
            budget.range_label(),
            budget.word_max
        )
    }
}

async fn revise_full(
    llm: &LlmClient,
    skill: &str,
    state: &ProjectState,
    chapter: u32,
    project_dir: &Path,
    outline: &str,
    draft: &str,
    revision: &RevisionOptions,
    canon: &str,
    naming_block: &str,
    budget: &ChapterBudget,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    content_rules: &ContentRulesConfig,
    flush_draft: bool,
) -> Result<String> {
    let instr = revision
        .user_instructions
        .clone()
        .unwrap_or_else(|| "按审校意见修订".into());
    let issues_block = format_audit_issues_brief(&revision.audit_issues);
    let issues_section = if issues_block.is_empty() || instr.contains(&issues_block) {
        String::new()
    } else {
        format!("\n\n# 审校问题清单\n{issues_block}")
    };
    let length_note = revise_full_length_note(&instr, &revision.audit_issues, budget);
    let user = format!(
        "修订《{}》第{chapter}章。\n指令：{instr}{issues_section}{length_note}\n\
         修订必须服从 CanonContext，并遵守取名硬约束；清单中的问题必须在正文中可见地消除。\n\
         {WRITER_HARD_CONSTRAINTS}\n\n\
         {naming_block}\n\n{canon}\n\n# 章纲\n{outline}\n\n# 现有正文\n{draft}",
        state.name
    );
    // Live pipeline: stream into reader. Preview path keeps disk untouched.
    let model = llm.model_for_agent("writer");
    let sink = if flush_draft {
        Some(DraftFlushSink {
            project_dir: project_dir.to_path_buf(),
            chapter,
        })
    } else {
        None
    };
    let rewritten = stream_agent_llm(
        llm,
        skill,
        &user,
        &model,
        "writer",
        tx,
        true,
        sink,
    )
    .await?;
    let rewritten = scrub_writer_draft(rewritten, content_rules);
    let new_len = rewritten.chars().count();
    let old_len = draft.chars().count();
    // Guard: truncated flash outputs used to wipe a longer draft (e.g. 900 → 373).
    if needs_full_rewrite(&instr) && new_len < 1500 && new_len < old_len {
        tracing::warn!(
            chapter,
            old_len,
            new_len,
            "rewrite looks truncated; keeping previous draft"
        );
        return Ok(draft.to_string());
    }
    if needs_full_rewrite(&instr) && new_len < 1500 {
        tracing::warn!(chapter, new_len, "rewrite still short; returning model output anyway");
    }
    Ok(rewritten)
}

/// Ask nomenclature curator for old→new map and apply replacements in draft.
async fn run_nomenclature_rename(
    llm: &LlmClient,
    skill: &str,
    project_dir: &Path,
    draft: &str,
    banned: &[String],
    naming_block: &str,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<String> {
    if banned.is_empty() {
        return Ok(draft.to_string());
    }
    let excerpt: String = draft.chars().take(5000).collect();
    let user = format!(
        "正文出现禁名：{}。\n\
         输出 JSON 对象：{{\"replacements\":[{{\"old\":\"禁名\",\"new\":\"合规新名\",\"rationale\":\"名→能力/本质\"}}]}}\n\
         要求：new 不得仍在禁名表中；保持可朗读；人名优先 2–3 字。\n\n\
         {naming_block}\n\n# 正文节选\n{excerpt}",
        banned.join("、")
    );
    let agent = "nomenclature_curator";
    let model = llm.model_for_agent(agent);
    // Chapter planner JSON — keep UI light (heartbeats only).
    let raw = stream_agent_llm(llm, skill, &user, &model, agent, tx, false, None).await?;
    let mut out = draft.to_string();
    let mut applied: Vec<(String, String, String)> = Vec::new();
    if let Some(v) = extract_json(&raw) {
        if let Some(arr) = v.get("replacements").and_then(|x| x.as_array()) {
            for item in arr {
                let old = item.get("old").and_then(|x| x.as_str()).unwrap_or("");
                let new = item.get("new").and_then(|x| x.as_str()).unwrap_or("");
                let rationale = item
                    .get("rationale")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if old.is_empty() || new.is_empty() || old == new {
                    continue;
                }
                if banned.iter().any(|b| b == new) {
                    continue;
                }
                out = out.replace(old, new);
                applied.push((old.to_string(), new.to_string(), rationale));
                tracing::info!(%old, %new, "nomenclature rename applied");
            }
        }
    }
    if !applied.is_empty() {
        let _ = register_nomenclature(project_dir, &applied);
    }
    Ok(out)
}

fn register_nomenclature(project_dir: &Path, pairs: &[(String, String, String)]) -> Result<()> {
    let path = project_dir.join("lore/nomenclature.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut root: Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?).unwrap_or(json!({"entities": []}))
    } else {
        json!({"entities": []})
    };
    let entities = root
        .as_object_mut()
        .map(|o| o.entry("entities".to_string()).or_insert(json!([])))
        .ok_or_else(|| anyhow::anyhow!("nomenclature root"))?;
    let arr = entities.as_array_mut().ok_or_else(|| anyhow::anyhow!("entities"))?;
    for (old, new, rationale) in pairs {
        if arr.iter().any(|e| e.get("canonical_name").and_then(|x| x.as_str()) == Some(new.as_str()))
        {
            continue;
        }
        arr.push(json!({
            "canonical_name": new,
            "aliases": [old],
            "ability_or_trait": rationale,
            "source": "nomenclature_curator",
        }));
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

async fn run_specialist_rewrite(
    llm: &LlmClient,
    skill: &str,
    agent: &str,
    focus: &str,
    draft: &str,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<String> {
    let user = format!("{focus}\n只输出修改后的完整正文 Markdown。\n\n{draft}");
    let model = llm.model_for_agent(agent);
    // Codex-like: stream into ToolCallOutputDelta (core coalesces ~40ms / 800B).
    let polished = stream_agent_llm(llm, skill, &user, &model, agent, tx, true, None).await?;
    let cleaned = strip_md_fence(&polished);
    let out = if cleaned.trim().is_empty() {
        draft.to_string()
    } else {
        cleaned
    };
    Ok(out)
}

fn load_previous_audit_issues(audit_path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(audit_path).unwrap_or_default();
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("issues").cloned())
        .and_then(|x| x.as_array().cloned())
        .map(with_issue_ids)
        .unwrap_or_default()
}

/// Verify-mode auditor: judge previous issues fixed/still_open; allow few new hard finds.
async fn run_consistency_verify(
    llm: &LlmClient,
    skill: &str,
    draft: &str,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
    naming_block: &str,
    previous: &[Value],
) -> Result<(Vec<Value>, Vec<Value>, String)> {
    let excerpt: String = draft.chars().take(24000).collect();
    let skill_short = trim_skill(skill, 1800);
    let prev_brief = previous
        .iter()
        .enumerate()
        .map(|(i, issue)| {
            let id = issue
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let pri = issue
                .get("priority")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let ty = issue.get("type").and_then(|v| v.as_str()).unwrap_or("?");
            let msg = issue
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let quote = issue
                .get("quote")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let loc = issue
                .get("location")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!(
                "{}. id={id} [{pri}/{ty}] {msg}\n   location={loc}\n   quote={quote}",
                i + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let user = format!(
        "这是修订后的复审（verify previous），不是全章重新挑刺。\n\
         任务：\n\
         1) 逐条判定「上一轮问题」是否已修好：status=fixed 或 still_open（必须带原 id）。\n\
         2) 仅当发现**新的硬冲突**（时间互斥/倒计时回跳、伤部位矛盾、能力位置矛盾、设定硬冲突、章号元叙述）才写入 new_issues，最多 3 条。\n\
         3) 禁止把可补交代、动机略跳、风格/笔误写成 new_issues 的 P0；禁止重复罗列已在 previous 里的同一问题。\n\
         只输出 JSON：{{\"verdicts\":[{{\"id\":\"\",\"status\":\"fixed|still_open\",\"note\":\"\"}}],\
         \"new_issues\":[{{\"type\":\"TIMELINE|INJURY|ABILITY_LOC|LORE|CHARACTER|POV|CONTINUITY|OUTLINE|META|PLOT\",\
         \"priority\":\"P0|P1|P2\",\"message\":\"\",\"location\":\"第N段\",\"quote\":\"\"}}],\
         \"report\":\"简短复审说明\"}}\n\n\
         {naming_block}\n\n{canon}\n\n# 上一轮问题\n{prev_brief}\n\n# 正文\n{excerpt}"
    );
    let agent = "consistency_auditor";
    let model = llm.model_for_agent(agent);
    let mut raw =
        stream_agent_llm(llm, &skill_short, &user, &model, agent, tx, false, None).await?;
    if raw.trim().is_empty() {
        tracing::warn!(%model, "consistency verify empty; retrying once");
        raw = stream_agent_llm(llm, &skill_short, &user, &model, agent, tx, false, None).await?;
    }
    if let Some(v) = extract_json(&raw) {
        let verdicts = v
            .get("verdicts")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let new_issues = v
            .get("new_issues")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let report = v
            .get("report")
            .and_then(|x| x.as_str())
            .unwrap_or("复审完成")
            .to_string();
        return Ok((verdicts, new_issues, report));
    }
    // Unparseable verify → keep previous open (conservative), no free pass / no flood.
    let verdicts = previous
        .iter()
        .filter_map(|i| {
            let id = i.get("id").and_then(|v| v.as_str())?;
            Some(json!({"id": id, "status": "still_open", "note": "复审输出无法解析"}))
        })
        .collect();
    Ok((
        verdicts,
        vec![],
        "复审输出无法解析，保留上一轮未关闭问题".into(),
    ))
}

async fn run_consistency_auditor(
    llm: &LlmClient,
    skill: &str,
    draft: &str,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
    naming_block: &str,
) -> Result<(bool, Vec<Value>, String)> {
    // Full draft when possible — clock/injury/ability locus often appear mid/late chapter.
    let excerpt: String = draft.chars().take(24000).collect();
    let skill_short = trim_skill(skill, 2200);
    let user = format!(
        "对照 CanonContext 审计下列正文。\n\
         优先级（强制）：\n\
         - P0（阻断）：设定库/禁名硬冲突；**已出现**的章号元叙述；\
         **作者旁白/章纲对读/纠错解说**（「章纲里…」「不是A是B的意思」、机制说明书腔，type=META）；\
         能力破设定；**明确时间互斥/倒计时回跳**；\
         **章内时段无过渡回跳**（含把时段词当修辞：当前凌晨却写「……的夜晚」，或深夜后又写上午且无翌日/天亮）；\
         **受伤部位**矛盾；**能力所在位置/载体**矛盾；开篇**无视/未承接/断档/另起**钩子；\
         章纲关键事件**完全缺失**（type=OUTLINE）。有任一真正 P0 → passed=false。\n\
         - P1（不阻断）：动机/铺垫不足、可补交代、剧情卡尚未收束、钩子略跳但已接上、次要状态含糊、\
         时间压缩感但无回跳、能力描写略糊但建议写清。仅有 P1/P2 → passed=true。\n\
         - P2：风格/笔误。\n\
         禁止：把「无违规/未检出/无实质断裂/可补交代/不算硬冲突/略有压缩感/建议强化」写成 P0；\
         未检出章号/旁白问题时不要输出 META issue。\n\
         章号元叙述硬禁：叙述/独白/对话中不得用「第N章」「第八章」指称情节；标题行除外。\n\
         时间线核对：列出本章全部「还剩/剩余/钟点」当前读数是否单调；再核对凌晨/夜晚/上午/傍晚/深夜等时段是否随叙事前进（勿把时段词当修辞）。\n\
         只输出 JSON：{{\"passed\":bool,\"issues\":[{{\"type\":\"TIMELINE|INJURY|ABILITY_LOC|LORE|CHARACTER|POV|CONTINUITY|OUTLINE|META|PLOT\",\"priority\":\"P0|P1|P2\",\"message\":\"\",\"location\":\"第N段\",\"quote\":\"\"}}],\"report\":\"可读报告（须说明是否核对了明确时间、伤势部位、能力位置、设定、章号与作者旁白）\"}}\n\n\
         {naming_block}\n\n{canon}\n\n# 正文\n{excerpt}"
    );
    let agent = "consistency_auditor";
    let model = llm.model_for_agent(agent);
    // Never mirror JSON tokens into the tool card — floods WS and drops ItemCompleted.
    let mut raw =
        stream_agent_llm(llm, &skill_short, &user, &model, agent, tx, false, None).await?;
    // One silent retry — empty content used to be common when thinking ate max_tokens.
    if raw.trim().is_empty() {
        tracing::warn!(%model, "consistency_auditor empty; retrying once");
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(PipelineEvent::LlmDelta {
                agent: agent.into(),
                delta: "（审计返回为空，正在重试…）\n".into(),
            });
        }
        raw = stream_agent_llm(llm, &skill_short, &user, &model, agent, tx, false, None).await?;
    }
    if raw.trim().is_empty() {
        return Ok((
            false,
            vec![json!({
                "type": "META",
                "priority": "P0",
                "message": "一致性审计模型返回为空，请重试 audit_chapter",
                "location": "",
                "quote": ""
            })],
            "一致性审计失败：模型返回为空".into(),
        ));
    }
    if let Some(v) = extract_json(&raw) {
        let passed = v.get("passed").and_then(|x| x.as_bool()).unwrap_or(true);
        let issues = v
            .get("issues")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let report = v
            .get("report")
            .and_then(|x| x.as_str())
            .unwrap_or(&raw)
            .to_string();
        return Ok((passed, issues, report));
    }
    // Unparseable JSON is not a free pass — surface as blocking so the user can retry.
    Ok((
        false,
        vec![json!({
            "type": "META",
            "priority": "P0",
            "message": "一致性审计未返回可解析 JSON，请重试",
            "location": "",
            "quote": ""
        })],
        if raw.trim().is_empty() {
            "一致性审计失败：无输出".into()
        } else {
            format!("一致性审计输出无法解析：\n{}", raw.chars().take(800).collect::<String>())
        },
    ))
}

async fn run_pacing_reviewer(
    llm: &LlmClient,
    skill: &str,
    draft: &str,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    canon: &str,
) -> Result<(Vec<Value>, String)> {
    let paras = segment_paragraphs(draft);
    let numbered: String = paras
        .iter()
        .enumerate()
        .take(40)
        .map(|(i, p)| format!("[{}] {}", i + 1, p.chars().take(60).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");
    // Keep skill contract (fields + non-blocking P0) — do not truncate below ~2k.
    let skill_short = trim_skill(skill, 2400);
    let user = format!(
        "节奏审查：结合卷幕目标与未收线，检查是否拖沓/信息过载。\n\
         门控：节奏 P0/P1 **不阻断发布**；勿把时间线/伤部位/设定硬冲突写成节奏问题。\n\
         只输出 JSON：{{\"pacing_passed\":bool,\"suggestions\":[{{\
         \"priority\":\"P0|P1|P2\",\"location\":\"第N段\",\"quote\":\"原文\",\
         \"action\":\"删减|拆分|补对话|补互动|压缩环境|加强冲突\",\
         \"detail\":\"具体改法\",\"suggestion\":\"与detail相同\"}}],\"report\":\"总评\"}}\n\n\
         {canon}\n\n# 段落索引\n{numbered}"
    );
    let agent = "pacing_reviewer";
    let model = llm.model_for_agent(agent);
    let raw = stream_agent_llm(llm, &skill_short, &user, &model, agent, tx, false, None).await?;
    if let Some(v) = extract_json(&raw) {
        let mut suggestions = v
            .get("suggestions")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        for s in &mut suggestions {
            normalize_pacing_suggestion(s);
        }
        let report = v
            .get("report")
            .and_then(|x| x.as_str())
            .unwrap_or(&raw)
            .to_string();
        return Ok((suggestions, report));
    }
    Ok((vec![], raw))
}

/// Ensure `suggestion` is populated from `detail` for draft-patch / legacy consumers.
fn normalize_pacing_suggestion(s: &mut Value) {
    let detail = s
        .get("detail")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let suggestion = s
        .get("suggestion")
        .or_else(|| s.get("message"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let action = s
        .get("action")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let quote = s
        .get("quote")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let body = if !detail.is_empty() {
        detail
    } else if !suggestion.is_empty() {
        suggestion
    } else {
        "调整节奏".into()
    };
    let mut parts = Vec::new();
    if !action.is_empty() {
        parts.push(format!("动作：{action}"));
    }
    if !quote.is_empty() {
        parts.push(format!("锚点：「{quote}」"));
    }
    parts.push(body);
    let merged = parts.join("；");
    if let Some(obj) = s.as_object_mut() {
        obj.insert("suggestion".into(), Value::String(merged));
    }
}

fn trim_skill(skill: &str, max_chars: usize) -> String {
    let s: String = skill.chars().take(max_chars).collect();
    if skill.chars().count() > max_chars {
        format!("{s}\n…(skill truncated)")
    } else {
        s
    }
}

fn load_agent_skill_body(skills: &[novelx_skills::SkillMetadata], name: &str) -> String {
    let key = name.replace('_', "-");
    let injections = build_skill_injections(skills, &[key]);
    injections
        .into_iter()
        .next()
        .map(|i| i.body)
        .unwrap_or_else(|| {
            "你是取名专员。规避语料高频禁名，输出 replacements JSON（old→new）。".into()
        })
}

/// Optional intermittent `draft.md` flush while streaming (frontend looks live).
struct DraftFlushSink {
    project_dir: PathBuf,
    chapter: u32,
}

/// Flush cadence: first snapshot early, then every ~400 chars or ~1.2s.
const DRAFT_FLUSH_FIRST_CHARS: usize = 80;
const DRAFT_FLUSH_MIN_CHARS: usize = 400;
const DRAFT_FLUSH_MIN_MS: u128 = 1200;

/// Stream an agent LLM call with TTFT heartbeats; never block on UI channel.
///
/// Mirrors Codex item lifecycle for pipeline steps nested in a tool call:
/// progress chunks → `ToolCallOutputDelta` (coalesced in core) → `ItemCompleted`.
///
/// `mirror_deltas`:
/// - `true` for prose agents (writer / full revise) — UI streams like Codex agent output
/// - `false` for JSON-heavy auditors/planners — avoid flooding WS with structured blobs
///
/// `draft_flush`: when set, periodically rewrite `draft.md` and emit
/// [`PipelineEvent::DraftFlushed`] so the reader can refresh without waiting for turn end.
async fn stream_agent_llm(
    llm: &LlmClient,
    skill: &str,
    user: &str,
    model: &str,
    agent: &str,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    mirror_deltas: bool,
    draft_flush: Option<DraftFlushSink>,
) -> Result<String> {
    if let Some(tx) = tx.as_ref() {
        let _ = tx.send(PipelineEvent::LlmDelta {
            agent: agent.into(),
            delta: "（调用模型中…）\n".into(),
        });
    }

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_hb = stop.clone();
    let agent_hb = agent.to_string();
    let tx_hb = tx.clone();
    let heartbeat = tokio::spawn(async move {
        let mut n = 0u32;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            if stop_hb.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            n += 15;
            if let Some(tx) = tx_hb.as_ref() {
                let hint = if n >= 45 {
                    format!("（仍在等待首包… {n}s；超过 60s 将超时，可停止后重试）\n")
                } else {
                    format!("（仍在等待首包… {n}s）\n")
                };
                let _ = tx.send(PipelineEvent::LlmDelta {
                    agent: agent_hb.clone(),
                    delta: hint,
                });
            }
        }
    });

    let got_content = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_c = got_content.clone();
    let stop_c = stop.clone();
    let acc = Arc::new(Mutex::new(String::new()));
    let flush_state = Arc::new(Mutex::new((Instant::now(), 0usize)));
    let flush_sink = draft_flush.map(Arc::new);
    let retry_n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let max_tokens = llm.max_tokens_for_agent(agent);
    let result = llm
        .complete_stream_limited_restart(
            skill,
            user,
            Some(model),
            Some(max_tokens),
            |delta| {
                let tx = tx.clone();
                let agent = agent.to_string();
                let got_c = got_c.clone();
                let stop_c = stop_c.clone();
                let acc = acc.clone();
                let flush_state = flush_state.clone();
                let flush_sink = flush_sink.clone();
                async move {
                    let first = !got_c.swap(true, std::sync::atomic::Ordering::Relaxed);
                    if first {
                        stop_c.store(true, std::sync::atomic::Ordering::Relaxed);
                        if !mirror_deltas {
                            if let Some(tx) = tx.as_ref() {
                                let _ = tx.send(PipelineEvent::LlmDelta {
                                    agent: agent.clone(),
                                    delta: "（生成中…）\n".into(),
                                });
                            }
                        }
                    }
                    if mirror_deltas {
                        if let Some(tx) = tx.as_ref() {
                            let _ = tx.send(PipelineEvent::LlmDelta {
                                agent: agent.clone(),
                                delta: delta.clone(),
                            });
                        }
                    }
                    if let Some(sink) = flush_sink.as_ref() {
                        let snapshot = {
                            let mut buf = acc.lock().unwrap_or_else(|e| e.into_inner());
                            buf.push_str(&delta);
                            let chars = buf.chars().count();
                            let mut st = flush_state.lock().unwrap_or_else(|e| e.into_inner());
                            let elapsed = st.0.elapsed().as_millis();
                            let grown = chars.saturating_sub(st.1);
                            let first_flush = st.1 == 0 && chars >= DRAFT_FLUSH_FIRST_CHARS;
                            let cadence =
                                grown >= DRAFT_FLUSH_MIN_CHARS || elapsed >= DRAFT_FLUSH_MIN_MS;
                            if chars > 0 && (first_flush || (st.1 > 0 && cadence)) {
                                st.0 = Instant::now();
                                st.1 = chars;
                                Some((buf.clone(), chars))
                            } else {
                                None
                            }
                        };
                        if let Some((text, chars)) = snapshot {
                            if let Err(e) =
                                write_chapter_draft(&sink.project_dir, sink.chapter, &text)
                            {
                                tracing::warn!(
                                    chapter = sink.chapter,
                                    error = %e,
                                    "intermittent draft flush failed"
                                );
                            } else if let Some(tx) = tx.as_ref() {
                                // Reader sync marker; prose itself already streams when mirror_deltas.
                                let _ = tx.send(PipelineEvent::DraftFlushed { chars });
                                if !mirror_deltas {
                                    let _ = tx.send(PipelineEvent::LlmDelta {
                                        agent: agent.clone(),
                                        delta: format!("（已生成约 {chars} 字…）\n"),
                                    });
                                }
                            }
                        }
                    }
                }
            },
            || {
                let tx = tx.clone();
                let agent = agent.to_string();
                let got_c = got_content.clone();
                let acc = acc.clone();
                let flush_state = flush_state.clone();
                let flush_sink = flush_sink.clone();
                let retry_n = retry_n.clone();
                async move {
                    let n = retry_n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    // Drop partial stream so the next attempt doesn't concatenate.
                    {
                        let mut buf = acc.lock().unwrap_or_else(|e| e.into_inner());
                        buf.clear();
                    }
                    {
                        let mut st = flush_state.lock().unwrap_or_else(|e| e.into_inner());
                        *st = (Instant::now(), 0);
                    }
                    got_c.store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Some(sink) = flush_sink.as_ref() {
                        // Keep prior draft on disk until new tokens arrive; UI note only.
                        let _ = sink;
                    }
                    // LLM layer already emits a retry marker via on_delta; only log here.
                    tracing::info!(agent = %agent, retry = n, "stream_agent_llm cleared partial buffer for retry");
                    let _ = tx;
                }
            },
        )
        .await;

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    heartbeat.abort();
    if let Ok(text) = &result {
        if let Some(sink) = flush_sink.as_ref() {
            // Final flush so reader catches the last tokens before post-processing.
            if let Err(e) = write_chapter_draft(&sink.project_dir, sink.chapter, text) {
                tracing::warn!(
                    chapter = sink.chapter,
                    error = %e,
                    "final stream draft flush failed"
                );
            } else if let Some(tx) = tx.as_ref() {
                let _ = tx.send(PipelineEvent::DraftFlushed {
                    chars: text.chars().count(),
                });
            }
        }
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(PipelineEvent::LlmDelta {
                agent: agent.into(),
                delta: format!("（模型返回完成，{} 字）\n", text.chars().count()),
            });
        }
    }
    result
}

/// Sample head + middle + tail so long chapters don't lose ending facts/hooks.
/// Prefer full draft when within max_chars; otherwise head/mid/tail sample.
fn summarizer_excerpt(draft: &str, max_chars: usize) -> String {
    let chars: Vec<char> = draft.chars().collect();
    if chars.len() <= max_chars {
        return draft.to_string();
    }
    let head_n = max_chars * 2 / 5;
    let mid_n = max_chars / 5;
    let tail_n = max_chars.saturating_sub(head_n + mid_n + 24);
    let mid_start = chars.len().saturating_sub(mid_n) / 2;
    let head: String = chars.iter().take(head_n).collect();
    let mid: String = chars.iter().skip(mid_start).take(mid_n).collect();
    let tail: String = chars.iter().skip(chars.len().saturating_sub(tail_n)).collect();
    format!("{head}\n\n…[中段]…\n\n{mid}\n\n…[章末]…\n\n{tail}")
}

async fn run_summarizer(llm: &LlmClient, skill: &str, draft: &str) -> Result<String> {
    let excerpt = summarizer_excerpt(draft, 7000);
    let raw = llm
        .complete_for_agent(
            "summarizer",
            skill,
            &format!(
                "为正文生成 JSON 摘要。字段：event_summary, relationship_changes, new_facts[], \
                 body_state:{{injuries:[],ability_loci:[]}}, foreshadow_updates[], ending_hook, \
                 plot_progress（可选）。\
                 body_state 必填：当前有效伤势写清侧别/部位与行动限制（每角色每部位一条）；\
已愈合/仅残迹须显式标明；能力写清寄宿/附着/载体；无变化则空数组，勿复抄上章长伤势。\
                 注意正文可能含中段/章末切片，须覆盖结尾钩子与后半关键事实。只输出 JSON。\
                 剧情卡是否完结由后续 plot_acceptor 判定，本步不要输出 plot_exit_met。\n\n\
                 # 正文\n{excerpt}"
            ),
        )
        .await?;
    if let Some(v) = extract_json(&raw) {
        Ok(v.to_string())
    } else {
        Ok(json!({
            "event_summary": raw.chars().take(400).collect::<String>(),
            "relationship_changes": "",
            "new_facts": [],
            "body_state": { "injuries": [], "ability_loci": [] },
            "foreshadow_updates": [],
            "ending_hook": "",
        })
        .to_string())
    }
}

async fn run_plot_acceptor(
    llm: &LlmClient,
    skill: &str,
    project_dir: &std::path::Path,
    chapter: u32,
    draft: &str,
    summary: &str,
) -> Result<String> {
    if let Some((title, _)) = crate::plots::in_progress_plot_missing_exit(project_dir) {
        return Ok(json!({
            "pass": false,
            "skipped": true,
            "rationale": format!(
                "进行中剧情卡「{title}」缺少收束条件，无法验收；请先补全卡面「收束条件」"
            ),
        })
        .to_string());
    }
    let Some((entry, exit, _)) = crate::plots::active_plot_exit_context(project_dir) else {
        return Ok(json!({
            "pass": false,
            "skipped": true,
            "rationale": "无进行中剧情卡",
        })
        .to_string());
    };
    let excerpt = summarizer_excerpt(draft, 6000);
    let summary_excerpt: String = summary.chars().take(1200).collect();
    let raw = llm
        .complete_for_agent(
            "plot_acceptor",
            skill,
            &format!(
                "验收第{chapter}章是否兑现当前剧情卡收束条件。只输出 JSON。\n\n\
                 # 当前剧情卡\n标题：{}\n收束条件（唯一标准，禁止改写）：{exit}\n\n\
                 规则：必须对照上述收束原文；不得换成「本章另外发生的事」或「下一张卡的伏笔」来判 pass；\
                 未全部兑现则 pass=false 并列出 gaps。\n\n\
                 # 本章摘要（辅证）\n{summary_excerpt}\n\n\
                 # 本章正文（证据主源）\n{excerpt}",
                entry.title
            ),
        )
        .await?;
    if let Some(mut v) = extract_json(&raw) {
        // Validate against the model's reported exit BEFORE we pin the card text.
        let soft_pass = crate::plots::accept_verdict_is_pass_against(&v.to_string(), Some(&exit));
        if let Some(obj) = v.as_object_mut() {
            obj.insert("plot_title".into(), json!(entry.title.clone()));
            obj.insert("exit_condition".into(), json!(exit.clone()));
            if !soft_pass {
                obj.insert("pass".into(), json!(false));
                let gaps_empty = obj
                    .get("gaps")
                    .and_then(|g| g.as_array())
                    .map(|a| a.is_empty())
                    .unwrap_or(true);
                if gaps_empty {
                    obj.insert(
                        "gaps".into(),
                        json!(["收束条件未按卡面原文全部兑现（或模型改写了收束）"]),
                    );
                }
                let note = "系统校验：未认可本次 pass（收束须与卡面一致且 gaps 为空）";
                match obj.get("rationale").and_then(|x| x.as_str()) {
                    Some(r) if !r.is_empty() => {
                        obj.insert("rationale".into(), json!(format!("{r}；{note}")));
                    }
                    _ => {
                        obj.insert("rationale".into(), json!(note));
                    }
                }
            }
        }
        Ok(v.to_string())
    } else {
        Ok(json!({
            "pass": false,
            "plot_title": entry.title,
            "exit_condition": exit,
            "rationale": raw.chars().take(300).collect::<String>(),
            "gaps": ["模型未返回合法 JSON，默认不通过"],
            "evidence": [],
        })
        .to_string())
    }
}

async fn run_foreshadow_tracker(
    llm: &LlmClient,
    skill: &str,
    project_dir: &Path,
    chapter: u32,
    draft: &str,
    outline: &str,
) -> Result<String> {
    let head: String = draft.chars().take(6000).collect();
    let tail = {
        let n = draft.chars().count();
        if n > 9000 {
            draft.chars().skip(n.saturating_sub(4000)).collect::<String>()
        } else {
            String::new()
        }
    };
    let history = crate::foreshadow::format_dangling_for_tracker(project_dir, 20);
    let bridge = crate::context::format_chapter_bridge(project_dir, chapter);
    let mut body = format!("## 正文·开篇\n{head}");
    if !tail.is_empty() {
        body.push_str("\n\n## 正文·章末\n");
        body.push_str(&tail);
    }
    let raw = llm
        .complete_for_agent(
            "foreshadow_tracker",
            skill,
            &format!(
                "对照章纲与正文追踪伏笔，只输出 JSON（buried/resolved/dangling/warnings）。\n\
                 必须先核「既有未收伏笔」：已兑现的写入 resolved（带原 id），仍未收的保留在 dangling；\
                 新埋入的写入 buried。禁止无故清空历史未收线。\n\n\
                 ## 既有未收伏笔\n{history}\n\n\
                 ## 章间衔接\n{}\n\n\
                 ## 章纲\n{outline}\n\n{body}",
                if bridge.is_empty() {
                    "（无）"
                } else {
                    bridge.as_str()
                },
            ),
        )
        .await?;
    if let Some(v) = extract_json(&raw) {
        Ok(v.to_string())
    } else {
        Ok(json!({
            "buried": [],
            "resolved": [],
            "dangling": [],
            "warnings": [raw.chars().take(200).collect::<String>()],
        })
        .to_string())
    }
}

fn apply_foreshadow_report(project_dir: &Path, chapter: u32, report: &str) -> Result<usize> {
    let v: Value = serde_json::from_str(report).unwrap_or(json!({}));
    let mut mem = load_memory(project_dir);
    let mut n = 0usize;

    if let Some(arr) = v.get("buried").and_then(|x| x.as_array()) {
        for item in arr {
            let text = item
                .get("description")
                .or_else(|| item.get("text"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                continue;
            }
            let id = item
                .get("id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("fs_{}", &Uuid::new_v4().simple().to_string()[..10]));
            if crate::memory::foreshadow_already_known(
                project_dir,
                &mem,
                Some(id.as_str()),
                &text,
            ) {
                continue;
            }
            let urgency = item
                .get("urgency")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let horizon_raw = item
                .get("horizon")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let horizon =
                crate::foreshadow::normalize_foreshadow_horizon(horizon_raw, &urgency);
            mem.open_threads.push(OpenThread {
                id,
                text,
                status: "open".into(),
                planted_chapter: chapter,
                horizon,
                urgency,
                ..Default::default()
            });
            n += 1;
        }
    }

    if let Some(arr) = v.get("resolved").and_then(|x| x.as_array()) {
        let mut cold_ids = Vec::new();
        let mut cold_texts = Vec::new();
        for item in arr {
            let id = item.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let text = item
                .get("description")
                .or_else(|| item.get("plant_ref"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if id.is_empty() && text.is_empty() {
                continue;
            }
            if !id.is_empty() {
                cold_ids.push(id.to_string());
            }
            if !text.is_empty() {
                cold_texts.push(text.to_string());
            }
            for t in mem
                .open_threads
                .iter_mut()
                .chain(mem.archived_threads.iter_mut())
            {
                let id_hit = !id.is_empty() && t.id == id;
                let text_hit = !text.is_empty()
                    && (t.status == "open" || t.status.is_empty())
                    && (t.text.contains(text) || text.contains(&t.text));
                if (t.status == "open" || t.status.is_empty()) && (id_hit || text_hit) {
                    t.status = "resolved".into();
                    t.resolved_chapter = chapter;
                    n += 1;
                    let kw: String = t.text.chars().take(40).collect();
                    let _ = crate::lore_index::upsert_foreshadow_index_row(
                        project_dir,
                        &t.id,
                        t.planted_chapter,
                        "resolved",
                        &kw,
                    );
                }
            }
        }
        if let Ok(cold_n) = crate::memory::resolve_foreshadow_archive(project_dir, chapter, |t| {
            cold_ids.iter().any(|id| t.id == *id)
                || cold_texts
                    .iter()
                    .any(|text| t.text.contains(text) || text.contains(&t.text))
        }) {
            n += cold_n;
        }
    }

    if let Some(arr) = v.get("dangling").and_then(|x| x.as_array()) {
        for item in arr {
            let text = item
                .get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let id = item
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let urgency = item
                .get("urgency")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let horizon_raw = item
                .get("horizon")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let horizon =
                crate::foreshadow::normalize_foreshadow_horizon(horizon_raw, &urgency);
            // Refresh horizon/urgency on an already-known open thread.
            let mut updated = false;
            if !horizon.is_empty() || !urgency.is_empty() {
                for t in mem
                    .open_threads
                    .iter_mut()
                    .chain(mem.archived_threads.iter_mut())
                {
                    if !(t.status == "open" || t.status.is_empty()) {
                        continue;
                    }
                    let id_hit = !id.is_empty() && t.id == id;
                    let text_hit = !text.is_empty()
                        && (t.text.contains(&text) || text.contains(&t.text));
                    if id_hit || text_hit {
                        if !horizon.is_empty() {
                            t.horizon = horizon.clone();
                        }
                        if !urgency.is_empty() {
                            t.urgency = urgency.clone();
                        }
                        updated = true;
                        n += 1;
                        break;
                    }
                }
            }
            if updated
                || text.is_empty()
                || crate::memory::foreshadow_already_known(
                    project_dir,
                    &mem,
                    if id.is_empty() { None } else { Some(id) },
                    &text,
                )
            {
                continue;
            }
            mem.open_threads.push(OpenThread {
                id: if id.is_empty() {
                    format!("fs_{}", &Uuid::new_v4().simple().to_string()[..10])
                } else {
                    id.to_string()
                },
                text,
                status: "open".into(),
                planted_chapter: chapter,
                horizon,
                urgency,
                ..Default::default()
            });
            n += 1;
        }
    }

    let _ = crate::memory::prune_open_threads_into_archive(Some(project_dir), &mut mem);
    save_memory(project_dir, &mem)?;
    let _ = crate::foreshadow::rebuild_foreshadow_index(project_dir);
    Ok(n)
}

fn strip_md_fence(text: &str) -> String {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```markdown") {
        return rest
            .strip_suffix("```")
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        return rest
            .strip_suffix("```")
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    t.to_string()
}

fn extract_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// Steer revision options. Expansion / rewrite intents force full writer rewrite
/// (local patches cannot grow chapter length).
pub fn steer_revision_options(message: &str) -> RevisionOptions {
    let full = needs_full_rewrite(message);
    RevisionOptions {
        prefer_local_patch: !full,
        revision_mode: true,
        user_instructions: Some(message.to_string()),
        audit_issues: vec![],
        pacing_suggestions: vec![],
        verify_previous: false,
        full_rescan: false,
        defer_soft_long_publish: false,
    }
}

/// True when the user wants length growth or a full rewrite (not a local patch).
/// Keywords live in `config/policies.yaml` (`full_rewrite`).
pub fn needs_full_rewrite(message: &str) -> bool {
    novelx_harness::needs_full_rewrite(message)
}

/// Result of chapter publish + plot/volume side effects.
pub struct PublishFinalizeResult {
    pub published: bool,
    pub await_human: bool,
    pub report_parts: Vec<String>,
    pub volume_ended: Option<u32>,
    pub volume_ended_name: Option<String>,
    pub volume_ended_start: Option<u32>,
    pub volume_ended_end: Option<u32>,
    /// Plot titles that transitioned to `completed` this publish.
    pub plots_completed: Vec<String>,
    /// True when post-plot setting audit reported a BLOCKER.
    pub plot_setting_blocker: bool,
    pub plot_accept_passed: Option<bool>,
    pub plot_accept_rationale: Option<String>,
}

/// When publish-time draft fingerprint no longer matches chapter memory JSON,
/// re-run foreshadow_tracker / summarizer so hot memory is not silently skipped.
async fn refresh_stale_chapter_memory_for_publish(
    project_dir: &Path,
    config_root: &Path,
    chapter: u32,
    draft: &str,
    llm: &LlmClient,
) -> (bool, bool) {
    if draft.trim().is_empty() {
        return (false, false);
    }
    let mut foreshadow_ok =
        chapter_memory_artifact_matches_draft(project_dir, chapter, "foreshadow.json", draft);
    let mut summary_ok =
        chapter_memory_artifact_matches_draft(project_dir, chapter, "summary.json", draft);
    if foreshadow_ok && summary_ok {
        return (true, true);
    }
    let skill_outcome = load_skills(&[
        (SkillScope::Studio, config_root.join("skills")),
        (SkillScope::Agent, config_root.join("skills/agents")),
    ]);
    let outline = read_chapter_outline(project_dir, chapter).unwrap_or_default();
    if !foreshadow_ok {
        let skill = load_agent_skill_body(&skill_outcome.skills, "foreshadow-tracker");
        match run_foreshadow_tracker(llm, &skill, project_dir, chapter, draft, &outline).await {
            Ok(report) => {
                if let Err(e) = write_chapter_memory_artifact(
                    project_dir,
                    chapter,
                    "foreshadow.json",
                    &report,
                    draft,
                ) {
                    tracing::warn!(error = %e, chapter, "write regenerated foreshadow.json failed");
                } else {
                    foreshadow_ok = true;
                    tracing::info!(chapter, "regenerated foreshadow.json for publish");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, chapter, "regenerate foreshadow.json failed");
            }
        }
    }
    if !summary_ok {
        let skill = load_agent_skill_body(&skill_outcome.skills, "summarizer");
        match run_summarizer(llm, &skill, draft).await {
            Ok(summary) => {
                if let Err(e) = write_chapter_memory_artifact(
                    project_dir,
                    chapter,
                    "summary.json",
                    &summary,
                    draft,
                ) {
                    tracing::warn!(error = %e, chapter, "write regenerated summary.json failed");
                } else {
                    summary_ok = true;
                    tracing::info!(chapter, "regenerated summary.json for publish");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, chapter, "regenerate summary.json failed");
            }
        }
    }
    (foreshadow_ok, summary_ok)
}

/// Advance `next_chapter`, close bridge plots, apply plot_acceptor pass, evaluate volume end.
/// Call once after a **full** chapter pipeline (not after SPAWN single steps).
pub async fn finalize_chapter_publish(
    project_dir: &Path,
    config_root: &Path,
    chapter: u32,
    mode: RunMode,
    state: &mut ProjectState,
    consistency_passed: bool,
    has_blocking: bool,
    await_human: bool,
    banned_blocking: bool,
    ran_summarizer: bool,
    pending_plot_accept: Option<&str>,
    llm: Arc<LlmClient>,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<PublishFinalizeResult> {
    let mut out = PublishFinalizeResult {
        published: false,
        await_human: false,
        report_parts: Vec::new(),
        volume_ended: None,
        volume_ended_name: None,
        volume_ended_start: None,
        volume_ended_end: None,
        plots_completed: Vec::new(),
        plot_setting_blocker: false,
        plot_accept_passed: None,
        plot_accept_rationale: None,
    };
    let publish_ok =
        should_publish(consistency_passed, has_blocking || await_human) && !banned_blocking;
    if !publish_ok {
        tracing::warn!(
            chapter,
            consistency_passed,
            has_blocking,
            await_human,
            banned_blocking,
            "chapter not published (gate)"
        );
        return Ok(out);
    }

    out.published = true;
    // Hot memory only after publish, and only when chapter JSON matches current draft.
    // If fingerprint mismatch / missing: regenerate before apply (revise-only paths often skip).
    let draft_now = read_chapter_draft(project_dir, chapter).unwrap_or_default();
    let ch_dir = project_dir
        .join("chapters")
        .join(format!("{chapter:03}"));
    let (foreshadow_fresh, summary_fresh) = refresh_stale_chapter_memory_for_publish(
        project_dir,
        config_root,
        chapter,
        &draft_now,
        llm.as_ref(),
    )
    .await;
    if foreshadow_fresh {
        if let Ok(report) = std::fs::read_to_string(ch_dir.join("foreshadow.json")) {
            match apply_foreshadow_report(project_dir, chapter, &report) {
                Ok(n) => tracing::info!(chapter, n, "foreshadow applied on publish"),
                Err(e) => tracing::warn!(error = %e, chapter, "foreshadow apply on publish failed"),
            }
        }
    } else {
        tracing::warn!(chapter, "foreshadow.json unavailable or stale after refresh; skip apply");
    }
    if summary_fresh {
        if let Ok(summary) = std::fs::read_to_string(ch_dir.join("summary.json")) {
            match apply_summary_json(project_dir, chapter, &summary) {
                Ok(n) => tracing::info!(chapter, n, "summary digest applied on publish"),
                Err(e) => tracing::warn!(error = %e, chapter, "summary apply on publish failed"),
            }
            match lore_assert_from_summary(project_dir, chapter, &summary) {
                Ok((asserted, conflicts)) => {
                    if !conflicts.is_empty() {
                        out.report_parts.push(format!(
                            "## Lore 冲突（未覆盖）\n{}",
                            conflicts.join("\n")
                        ));
                    }
                    tracing::info!(chapter, asserted, "lore assert on publish");
                }
                Err(e) => tracing::warn!(error = %e, chapter, "lore assert on publish failed"),
            }
        }
    } else {
        tracing::warn!(chapter, "summary.json unavailable or stale after refresh; skip apply");
        out.report_parts.push(
            "## 热记忆\n摘要与当前正文不一致且重生成失败，已跳过入库".into(),
        );
    }
    if state.next_chapter <= chapter {
        state.next_chapter = chapter + 1;
        state.published_count = state.published_count.max(chapter);
        save_project_state(project_dir, state)?;
    }
    let _ = crate::project::refresh_meta_flags(project_dir);
    let _ = crate::project::sync_records_novel_json(project_dir, state);
    // Longform: every 10 published chapters, append a cheap drift sample note.
    if chapter > 0 && chapter % 10 == 0 {
        let _ = append_drift_sample(project_dir, chapter);
    }

    let mut plot_events = Vec::new();
    match crate::plots::advance_plots_for_published_chapter(project_dir, chapter) {
        Ok(events) => plot_events.extend(events),
        Err(e) => tracing::warn!(error = %e, chapter, "plot index sync failed"),
    }
    match crate::plots::complete_bridging_plots_after_publish(project_dir) {
        Ok(events) => plot_events.extend(events),
        Err(e) => tracing::warn!(error = %e, chapter, "bridge plot complete failed"),
    }
    let accept_raw = pending_plot_accept.map(|s| s.to_string()).or_else(|| {
        std::fs::read_to_string(
            project_dir
                .join("chapters")
                .join(format!("{chapter:03}"))
                .join("plot_accept.json"),
        )
        .ok()
    });
    if let Some(verdict) = accept_raw {
        let v = serde_json::from_str::<serde_json::Value>(&verdict).unwrap_or_default();
        let skipped = v
            .get("skipped")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if !skipped {
            let passed = crate::plots::accept_verdict_is_pass(&verdict);
            out.plot_accept_passed = Some(passed);
            if !passed {
                if let Some(r) = v.get("rationale").and_then(|x| x.as_str()) {
                    let r = r.trim();
                    if !r.is_empty() {
                        out.plot_accept_rationale = Some(r.to_string());
                    }
                }
            }
        }
        match crate::plots::complete_active_plot_on_accept(project_dir, &verdict) {
            Ok(Some(ev)) => plot_events.push(ev),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "plot accept complete failed"),
        }
    }
    out.plots_completed = plot_events
        .iter()
        .filter(|e| e.to == "completed")
        .map(|e| e.title.clone())
        .collect();
    if !plot_events.is_empty() {
        let detail = plot_events
            .iter()
            .map(|e| format!("「{}」{}→{}", e.title, e.from, e.to))
            .collect::<Vec<_>>()
            .join("；");
        tracing::info!(chapter, %detail, "plot lifecycle advanced");
        out.report_parts
            .push(format!("## 剧情卡进度\n{detail}"));
    }

    let summary_ready = ran_summarizer
        || project_dir
            .join("chapters")
            .join(format!("{chapter:03}"))
            .join("summary.json")
            .exists();

    // Evaluate volume end before plot setting pass so we can skip light sync when
    // the full volume_sync gate is about to open.
    let mut volume_decision = None;
    if chapter == state.published_count && summary_ready {
        match crate::volume::evaluate_volume_end(project_dir, chapter, llm.as_ref()).await {
            Ok(Some(decision)) => volume_decision = Some(decision),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, chapter, "volume end evaluation failed");
            }
        }
    }

    let volume_ending = volume_decision.is_some();
    let pass_flags = crate::setting_pass::PlotSettingPassFlags::load(config_root);
    if pass_flags.enabled && !out.plots_completed.is_empty() {
        for title in out.plots_completed.clone() {
            match crate::setting_pass::run_plot_setting_pass(
                project_dir,
                config_root,
                &title,
                chapter,
                volume_ending,
                llm.clone(),
                tx.clone(),
            )
            .await
            {
                Ok(pass) => {
                    if pass.audit.blocker {
                        out.plot_setting_blocker = true;
                    }
                    out.report_parts.push(pass.report_markdown);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        plot = %title,
                        "plot setting pass failed"
                    );
                }
            }
        }
    } else if !volume_ending
        && out.plots_completed.is_empty()
        && crate::setting_pass::ChapterSettingPassFlags::load(config_root).enabled
        && summary_ready
    {
        // Plot still open: keep entity status/holdings current for the next chapter.
        match crate::setting_pass::run_chapter_setting_pass(
            project_dir,
            chapter,
            llm.clone(),
            tx.clone(),
        )
        .await
        {
            Ok(report) => {
                out.report_parts
                    .push(format!("## 章后设定同步\n{}", report.message));
            }
            Err(e) => {
                tracing::warn!(error = %e, chapter, "chapter setting pass failed");
            }
        }
    }

    if let Some(decision) = volume_decision {
        let vol = &decision.volume;
        let _ = crate::volume::mark_volume_completed(project_dir, vol, chapter);
        let _ = crate::phases::set_volume_phase(
            project_dir,
            crate::phases::VolumePhase::AwaitingSync,
        );
        // Same-turn safety net: planned/next_plot work must not leave us in handoff.
        if crate::phases::recover_false_volume_end(project_dir).is_some() {
            tracing::info!(
                chapter,
                volume = vol.volume_index,
                "volume end rolled back: open plot ladder remains"
            );
            out.report_parts.push(
                "## 卷末判定\n检测到本卷仍有未完成剧情卡/下一张未设计，已取消卷末交接，继续本卷写作。"
                    .into(),
            );
        } else {
            out.volume_ended = Some(vol.volume_index);
            out.volume_ended_name = Some(vol.name.clone());
            out.volume_ended_start = Some(vol.start_chapter.max(1));
            out.volume_ended_end = Some(chapter);
            out.await_human = true;
            let matched = if decision.matched.is_empty() {
                decision.reason.clone()
            } else {
                decision.matched.join("；")
            };
            if let Some(tx) = tx {
                let _ = tx.send(PipelineEvent::AwaitingHuman {
                    prompt: format!(
                        "第{}卷「{}」终止条件已达成（至第{}章），是否同步设定库？",
                        vol.volume_index, vol.name, chapter
                    ),
                    options: vec!["同步设定库".into(), "跳过".into()],
                });
            }
            out.report_parts.push(format!(
                "## 卷末\n第{}卷「{}」至第{}章：终止条件命中。\n- {}\n- 下一章将写第{}章（下卷）。待确认是否同步设定库。",
                vol.volume_index,
                vol.name,
                chapter,
                matched,
                chapter + 1
            ));
        }
    }
    let _ = mode;
    Ok(out)
}

/// One planned local patch for confirm UI / cached apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalPatchPreviewItem {
    pub start_para: usize,
    pub end_para: usize,
    pub before: String,
    pub after: String,
    #[serde(default)]
    pub instruction: String,
}

#[derive(Debug, Clone)]
pub struct LocalRevisionPreview {
    pub patches: Vec<LocalPatchPreviewItem>,
    pub summary_markdown: String,
}

/// Plan local patches with LLM but do **not** write draft.md.
pub async fn plan_local_revision_preview(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    revision: RevisionOptions,
    llm: Arc<LlmClient>,
) -> Result<LocalRevisionPreview> {
    let project_dir = projects_root.join(project);
    let existing = read_chapter_draft(&project_dir, chapter).unwrap_or_default();
    if existing.trim().is_empty() {
        anyhow::bail!("第{chapter}章无正文，无法规划局部修订");
    }
    let (_, audit_normalized) =
        novelx_harness::normalize_consistency_issues(revision.audit_issues.clone());
    if revision
        .user_instructions
        .as_deref()
        .map(needs_full_rewrite)
        .unwrap_or(false)
        || has_timeline_p0(&audit_normalized)
    {
        return Ok(LocalRevisionPreview {
            patches: vec![],
            summary_markdown: "该指令更适合整章修订，无局部补丁预览。".into(),
        });
    }
    let (targets, ok) = plan_revision(
        &existing,
        revision.user_instructions.as_deref(),
        &revision.audit_issues,
        &revision.pacing_suggestions,
    );
    if !ok || targets.is_empty() {
        return Ok(LocalRevisionPreview {
            patches: vec![],
            summary_markdown: "未能定位可局部修改的段落。".into(),
        });
    }
    let naming = NamingRules::load_from_config_root(config_root);
    let outline = read_chapter_outline(&project_dir, chapter).unwrap_or_default();
    let pack = build_chapter_context(
        &project_dir,
        chapter,
        &existing,
        &outline,
        ContextProfile::Full,
    );
    let (draft_after, applied) = revise_by_local_patches_collect(
        llm.as_ref(),
        &existing,
        &targets,
        &None,
        &pack.markdown,
        &naming.prompt_block(),
    )
    .await?;
    if applied.is_empty() || draft_after == existing {
        return Ok(LocalRevisionPreview {
            patches: vec![],
            summary_markdown: "局部修订未产生有效差异。".into(),
        });
    }
    let mut patches: Vec<LocalPatchPreviewItem> = applied
        .into_iter()
        .map(|ap| LocalPatchPreviewItem {
            start_para: ap.start + 1,
            end_para: ap.end + 1,
            before: ap.before.chars().take(2000).collect(),
            after: ap.after.chars().take(2000).collect(),
            instruction: ap.instruction,
        })
        .collect();
    // Cache full post-patch draft so apply does not re-run LLM.
    patches.insert(
        0,
        LocalPatchPreviewItem {
            start_para: 0,
            end_para: 0,
            before: String::new(),
            after: draft_after,
            instruction: "__full_draft__".into(),
        },
    );
    let visible = patches.len().saturating_sub(1);
    let mut summary = format!("## 局部修订预览\n共 {visible} 处可见差异。\n");
    for p in patches.iter().filter(|p| p.instruction != "__full_draft__") {
        summary.push_str(&format!(
            "\n### 第{}–{}段\n**−**\n{}\n\n**+**\n{}\n",
            p.start_para,
            p.end_para,
            p.before.chars().take(400).collect::<String>(),
            p.after.chars().take(400).collect::<String>()
        ));
    }
    Ok(LocalRevisionPreview {
        patches,
        summary_markdown: summary,
    })
}

/// Run Writer LLM for a full-chapter rewrite preview without writing `draft.md`.
/// Result is cached as `__full_draft__` for the mutation confirm apply step.
pub async fn plan_full_revision_preview(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    mut revision: RevisionOptions,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<LocalRevisionPreview> {
    revision.prefer_local_patch = false;
    revision.revision_mode = true;
    let project_dir = projects_root.join(project);
    let existing = read_chapter_draft(&project_dir, chapter).unwrap_or_default();
    if existing.trim().is_empty() {
        anyhow::bail!("第{chapter}章无正文，无法规划整章修订");
    }
    let state = load_project_state(&project_dir)?;
    let outline = read_chapter_outline(&project_dir, chapter).unwrap_or_default();
    let skill_outcome = load_skills(&[
        (SkillScope::Studio, config_root.join("skills")),
        (SkillScope::Agent, config_root.join("skills/agents")),
    ]);
    let skill_body = load_agent_skill_body(&skill_outcome.skills, "writer");
    let naming = NamingRules::load_from_config_root(config_root);
    let naming_block = naming.prompt_block();
    let content_rules = ContentRulesConfig::load_from_config_root(config_root);
    let mode_key = crate::project::resolve_project_mode(&project_dir).as_str();
    let chapter_budget = ChapterBudget::load_for_mode(config_root, mode_key);
    let script_shape = ScriptShapeConfig::load_from_config_root(config_root);
    let pack = build_chapter_context(
        &project_dir,
        chapter,
        &existing,
        &outline,
        ContextProfile::Full,
    );
    if let Some(tx) = &tx {
        let _ = tx.send(PipelineEvent::StepStarted {
            agent: "writer".into(),
        });
        let _ = tx.send(PipelineEvent::LlmDelta {
            agent: "writer".into(),
            delta: format!("（整章修订预览：第{chapter}章，不落盘）\n"),
        });
    }
    let (draft_after, _) = run_writer_ex(
        llm.as_ref(),
        &skill_body,
        &state,
        chapter,
        &project_dir,
        &outline,
        &existing,
        &revision,
        false,
        &tx,
        &pack.markdown,
        &naming_block,
        &chapter_budget,
        &content_rules,
        false,
    )
    .await?;
    let (shaped, _) =
        normalize_unit_body_best_effort(&project_dir, chapter, &draft_after, &script_shape);
    if shaped.trim().is_empty() {
        anyhow::bail!("整章修订预览产出为空");
    }
    if shaped == existing {
        anyhow::bail!("整章修订未产生有效差异");
    }
    // Refuse instruction-leak placeholders (legacy bug: instructions pasted as after).
    if looks_like_revision_instruction_leak(&shaped) {
        anyhow::bail!("整章修订预览疑似指令文本而非正文，已拒绝出卡");
    }
    if let Some(tx) = &tx {
        let _ = tx.send(PipelineEvent::StepCompleted {
            agent: "writer".into(),
            summary: format!("预览已生成（{} 字）", shaped.chars().count()),
        });
    }
    let summary = format!(
        "## 整章修订预览\n第{chapter}章全文重写对照（尚未写入磁盘）。确认「应用修改」后落盘。"
    );
    Ok(LocalRevisionPreview {
        patches: vec![LocalPatchPreviewItem {
            start_para: 0,
            end_para: 0,
            before: existing,
            after: shaped,
            instruction: "__full_draft__".into(),
        }],
        summary_markdown: summary,
    })
}

/// True when model (or legacy preview) returned rewrite *instructions* instead of prose.
pub fn looks_like_revision_instruction_leak(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    t.contains("（确认后按指令全文重写")
        || t.contains("确认后按指令修订")
        || (t.starts_with("指令：") && t.contains("整章修订"))
        || (t.contains("非局部补丁") && t.contains("id=fp-") && !t.contains("\n\n# 第"))
}

/// Apply cached local patches from confirm step (uses `__full_draft__` if present).
pub fn apply_cached_local_patches(
    projects_root: &Path,
    project: &str,
    chapter: u32,
    patches: &Value,
) -> Result<usize> {
    let project_dir = projects_root.join(project);
    let Some(arr) = patches.as_array() else {
        anyhow::bail!("cached_patches 须为数组");
    };
    if let Some(full) = arr.iter().find(|p| {
        p.get("instruction").and_then(|x| x.as_str()) == Some("__full_draft__")
    }) {
        let after = full.get("after").and_then(|x| x.as_str()).unwrap_or("");
        if after.trim().is_empty() {
            anyhow::bail!("缓存全文为空");
        }
        write_chapter_draft(&project_dir, chapter, after)?;
        return Ok(arr.len().saturating_sub(1).max(1));
    }
    let mut draft = read_chapter_draft(&project_dir, chapter).unwrap_or_default();
    let mut applied = 0usize;
    for p in arr {
        let start = p
            .get("start_para")
            .and_then(|x| x.as_u64())
            .unwrap_or(1)
            .saturating_sub(1) as usize;
        let end = p
            .get("end_para")
            .and_then(|x| x.as_u64())
            .unwrap_or((start + 1) as u64)
            .saturating_sub(1) as usize;
        let after = p.get("after").and_then(|x| x.as_str()).unwrap_or("");
        if after.is_empty() {
            continue;
        }
        let target = RevisionTarget {
            instruction: p
                .get("instruction")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            start,
            end: end.max(start),
            quote: String::new(),
            source: String::new(),
        };
        let result = apply_local_patches(&draft, &[target], &[(0, after.to_string())]);
        if result.used_local_patch {
            draft = result.draft;
            applied += 1;
        }
    }
    if applied == 0 {
        anyhow::bail!("未能应用任何补丁");
    }
    write_chapter_draft(&project_dir, chapter, &draft)?;
    Ok(applied)
}

/// Cheap longform drift sample every N chapters (deterministic, no LLM).
fn append_drift_sample(project_dir: &Path, chapter: u32) -> anyhow::Result<()> {
    let mem = crate::memory::load_memory(project_dir);
    let open = mem
        .open_threads
        .iter()
        .filter(|t| t.status == "open" || t.status.is_empty())
        .count();
    let soft = novelx_harness::consecutive_soft_short_from_meta(
        &crate::project::load_project_state(project_dir)
            .map(|s| s.meta)
            .unwrap_or_default(),
    );
    let exit = crate::plots::active_plot_exit_context(project_dir)
        .map(|(_, e, _)| {
            e.chars().take(80).collect::<String>()
        })
        .unwrap_or_default();
    let body = crate::body_state::format_body_state_board(project_dir, chapter);
    let line = serde_json::json!({
        "chapter": chapter,
        "open_threads": open,
        "consecutive_soft_short": soft,
        "active_exit": exit,
        "body_state_chars": body.chars().count(),
        "facts_hot": mem.asserted_facts.len(),
    });
    let path = project_dir.join("lore/drift_samples.jsonl");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", serde_json::to_string(&line)?)?;
    Ok(())
}

#[cfg(test)]
mod instruction_leak_tests {
    use super::looks_like_revision_instruction_leak;

    #[test]
    fn detects_legacy_instruction_placeholder() {
        let fake = "（确认后按指令全文重写第2章）\n指令：整章修订（非局部补丁）：通读本章\n1. [CONTINUITY] id=fp-abc";
        assert!(looks_like_revision_instruction_leak(fake));
    }

    #[test]
    fn real_chapter_prose_is_not_leak() {
        let prose = "# 第2章 翕动的涟漪\n\n相纸在掌心下鼓动，像一只被压住的蛾子。\n\n闻喻没有松手。";
        assert!(!looks_like_revision_instruction_leak(prose));
    }
}

#[cfg(test)]
mod revise_full_length_note_tests {
    use super::*;
    use novelx_harness::ChapterBudget;
    use serde_json::json;

    #[test]
    fn force_full_timeline_note_caps_word_max() {
        let budget = ChapterBudget::load_from_config_root(std::path::Path::new("config"));
        let note = revise_full_length_note(
            "按一致性 P0 整章理顺时间线",
            &[json!({
                "type": "TIMELINE",
                "priority": "P0",
                "message": "倒计时回跳",
                "location": "段1",
                "quote": "还剩"
            })],
            &budget,
        );
        assert!(note.contains("不得超过"), "{note}");
        assert!(note.contains(&budget.word_max.to_string()), "{note}");
        assert!(note.contains("禁止借机扩写") || note.contains("硬上限"), "{note}");
    }

    #[test]
    fn soft_long_defer_flag_only_applies_to_soft_long() {
        let defer = RevisionOptions {
            defer_soft_long_publish: true,
            ..Default::default()
        };
        assert!(defer.defer_soft_long_publish && "soft_long" == "soft_long");
        assert!(!(defer.defer_soft_long_publish && "ok" == "soft_long"));
        let no_defer = RevisionOptions::default();
        assert!(!no_defer.defer_soft_long_publish);
    }
}

#[cfg(test)]
mod local_patch_confirm_tests {
    use super::*;
    use crate::project::{init_project, read_chapter_draft, write_chapter_draft};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "novelx-localpatch-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn apply_cached_full_draft_writes_disk() {
        let root = tmp("full");
        let dir = init_project(&root, "book", "未定", 50).unwrap();
        write_chapter_draft(&dir, 1, "旧正文段落甲。\n\n旧正文段落乙。").unwrap();
        let patches = json!([
            {
                "start_para": 0,
                "end_para": 0,
                "before": "",
                "after": "新正文整章。",
                "instruction": "__full_draft__"
            }
        ]);
        let n = apply_cached_local_patches(&root, "book", 1, &patches).unwrap();
        assert!(n >= 1);
        assert_eq!(read_chapter_draft(&dir, 1).unwrap().trim(), "新正文整章。");
        let _ = fs::remove_dir_all(&root);
    }
}
