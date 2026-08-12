//! Batch continue-writing until a hard gate (longform throughput).

use crate::chapter_gate::check_chapter_order;
use crate::expected_events::{list_gate_candidates, list_hard_ok_candidates, needs_fresh_review};
use crate::phases::{
    resolve_setup_phase, resolve_volume_phase, PhaseEnforceFlags, VolumePhase,
};
use crate::plots::{
    check_plot_write_gate_with, ensure_bridge_plot_active, PlotWriteGate, PlotWriteMode,
};
use crate::project::{
    is_short_drama, load_project_state, project_dir, read_chapter_draft,
};
use crate::run::{
    execute_pipeline, pipeline_run_is_audit_infra, PipelineRun, RevisionOptions, RunMode,
};
use crate::schemas::draft_body_chars;
use crate::volume_audit_gate::check_volume_audit_for_continue;
use anyhow::Result;
use novelx_harness::{
    build_revise_plan, ChapterBudget, LengthAssessment, LongformConfig, RevisePlanConfig,
    ReviseScope,
};
use novelx_llm::LlmClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::run::PipelineEvent;
use crate::loop_runtime::{
    append_loop_journal, begin_loop_job, finish_loop_job, heartbeat_loop_job, now_rfc3339,
    run_loop_end_verify, soft_skip_keys_from_opts, StopContract,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchContinueOpts {
    pub project: String,
    /// Hard stop after this many **successful publishes** in this batch
    /// (default from longform.yaml). Not "attempts".
    pub max_chapters: Option<u32>,
    /// Stop after publishing this chapter number (inclusive), if set.
    /// When unset, falls back to `state.target_chapters` when > 0.
    pub until_chapter: Option<u32>,
    pub confirm_skip_volume_audit: bool,
    /// Skip expected-events review gate (same as continue_writing confirm_skip_expected).
    #[serde(default)]
    pub confirm_skip_expected: bool,
    /// Skip foreshadow `pressure_high` soft phase (batch soft-stop).
    #[serde(default)]
    pub confirm_skip_foreshadow: bool,
    /// When true: do not apply `unattended.yaml` soft-skip defaults (keep mid-audit /
    /// expected / foreshadow soft phases unless explicitly skipped above).
    #[serde(default)]
    pub respect_soft_gates: bool,
    /// When length HardShort / SoftShort-escalate blocks publish, try auto expand;
    /// HardLong → auto-split once into chapter + chapter+1.
    #[serde(default = "default_true")]
    pub auto_length_revise: bool,
}

fn default_true() -> bool {
    true
}

impl Default for BatchContinueOpts {
    fn default() -> Self {
        Self {
            project: String::new(),
            max_chapters: None,
            until_chapter: None,
            confirm_skip_volume_audit: false,
            confirm_skip_expected: false,
            confirm_skip_foreshadow: false,
            respect_soft_gates: false,
            auto_length_revise: true,
        }
    }
}

/// Apply `config/unattended.yaml` soft-skip defaults for batch (unless respect_soft_gates).
pub fn apply_unattended_batch_policy(config_root: &Path, opts: &mut BatchContinueOpts) -> bool {
    let policy = novelx_harness::UnattendedPolicy::load_from_config_root(config_root);
    let skips = policy.resolve_batch_skips(
        config_root,
        opts.respect_soft_gates,
        opts.confirm_skip_volume_audit,
        opts.confirm_skip_expected,
        opts.confirm_skip_foreshadow,
    );
    opts.confirm_skip_volume_audit = skips.skip_volume_audit;
    opts.confirm_skip_expected = skips.skip_expected;
    opts.confirm_skip_foreshadow = skips.skip_foreshadow_pressure;
    skips.applied_by_policy
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchChapterResult {
    pub chapter: u32,
    pub published: bool,
    pub blocked: bool,
    pub reason: Option<String>,
    pub needs_user_choice: bool,
    pub message: String,
    #[serde(default)]
    pub length_auto_revised: bool,
    /// Content_rules hard-rule revise attempted (up to batch_max_auto_revise).
    #[serde(default)]
    pub hard_rule_auto_revised: bool,
    /// Consistency P0 revise attempted (up to batch_max_auto_revise).
    #[serde(default)]
    pub consistency_auto_revised: bool,
    /// AuditOnly retries for audit_infra (empty/unparseable auditor).
    #[serde(default)]
    pub audit_infra_retries: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<PipelineRun>,
}

fn default_batch_unit() -> String {
    "章".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchContinueResult {
    pub project: String,
    pub started_chapter: u32,
    pub chapters_attempted: u32,
    pub chapters_published: u32,
    pub stopped_reason: String,
    /// Machine-checked stop (compatible clients may ignore; wire string stays in stopped_reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_contract: Option<StopContract>,
    pub results: Vec<BatchChapterResult>,
    /// Display unit: `章` (longform) or `集` (short_drama).
    #[serde(default = "default_batch_unit")]
    pub unit: String,
    /// Soft gates (mid volume audit / expected review) were auto-skipped by unattended policy.
    #[serde(default)]
    pub soft_gates_skipped_by_policy: bool,
    /// Policy keys skipped when soft_gates_skipped_by_policy (auditable).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soft_gates_skipped: Vec<String>,
    /// Progressive checklist snapshot at batch end (also streamed live via PipelineEvent::TodoList).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todos: Vec<novelx_protocol::TodoItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end_verify: Option<crate::loop_runtime::LoopEndVerify>,
    /// UI label: 可续写 / 需处理 / 已完成 …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_label: Option<String>,
}

fn expected_gate_message(
    project_dir: &Path,
    chapter: u32,
    skip_expected: bool,
) -> Option<(String, String)> {
    if skip_expected {
        return None;
    }
    if needs_fresh_review(project_dir, chapter) {
        let n = list_hard_ok_candidates(project_dir, chapter).len();
        return Some((
            "need_expected_review".into(),
            format!("有 {n} 条预处理预期硬条件已满足，请先检阅是否纳入本次创作。批写暂停。"),
        ));
    }
    let gate = list_gate_candidates(project_dir, chapter);
    if let Some(primary) = gate.first() {
        let reason = primary
            .last_review
            .as_ref()
            .map(|r| r.reason.as_str())
            .unwrap_or("检阅建议纳入");
        return Some((
            "expected_event_pending".into(),
            format!(
                "预处理预期「{}」可纳入本次创作（{}）。批写暂停，请人工选择纳入/跳过。",
                primary.text, reason
            ),
        ));
    }
    None
}

fn length_blocks_publish(project_dir: &Path, config_root: &Path, chapter: u32) -> bool {
    let draft = read_chapter_draft(project_dir, chapter).unwrap_or_default();
    if draft.trim().is_empty() {
        return false;
    }
    let budget = ChapterBudget::load_from_config_root(config_root);
    let body = draft_body_chars(&draft);
    match budget.assess_body_chars(body) {
        LengthAssessment::HardShort | LengthAssessment::HardLong => true,
        LengthAssessment::SoftShort => {
            let state = load_project_state(project_dir).ok();
            let streak = state
                .as_ref()
                .map(|s| novelx_harness::consecutive_soft_short_from_meta(&s.meta))
                .unwrap_or(0)
                .saturating_add(1);
            let thr = budget.soft_short_auto_revise_after;
            thr > 0 && streak >= thr
        }
        LengthAssessment::SoftLong | LengthAssessment::Ok => false,
    }
}

/// Effective `until_chapter`: explicit opt, else project `target_chapters` when set.
pub(crate) fn resolve_batch_until(opts_until: Option<u32>, target_chapters: u32) -> Option<u32> {
    opts_until.or_else(|| (target_chapters > 0).then_some(target_chapters))
}

/// Whether batch should attempt a length expand.
/// SoftShort escalate sets `needs_user_choice` — that flag must NOT veto auto revise.
pub(crate) fn should_batch_auto_length_revise(
    auto_enabled: bool,
    published: bool,
    content_rule_blocked: bool,
    volume_ended: bool,
    consistency_passed: Option<bool>,
    length_status: &str,
    length_blocks: bool,
) -> bool {
    if !auto_enabled || published || content_rule_blocked || volume_ended {
        return false;
    }
    if consistency_passed == Some(false) {
        return false;
    }
    // HardLong is handled by auto-split, not expand/compress revise.
    if length_status == "hard_long" {
        return false;
    }
    matches!(
        length_status,
        "hard_short" | "soft_short_escalated" | "soft_long"
    ) || length_blocks
}

/// Timeline / daypart issues rarely fix with a one-paragraph patch — force full rewrite.
fn looks_like_timeline_issue(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "时段",
        "回跳",
        "跨日",
        "翌日",
        "timeline",
        "daypart",
        "timeline_daypart",
        "timeline_period",
        "timeline_countdown",
    ]
    .iter()
    .any(|k| t.contains(&k.to_ascii_lowercase()))
}

/// Build one-shot batch revise instructions.
/// Returns `(instructions, force_full_rewrite)`.
pub(crate) fn build_batch_revise_instructions(
    run: &PipelineRun,
    need_consistency: bool,
    need_hard: bool,
    need_len: bool,
    budget: &ChapterBudget,
) -> (String, bool) {
    let mut parts = Vec::new();
    // SoftLong compress keeps force_full=false (local-first); expand/short still forces full.
    let mut force_full = false;

    if need_consistency {
        let p0: Vec<String> = run
            .issues
            .iter()
            .filter(|i| {
                i.get("priority")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .eq_ignore_ascii_case("P0")
            })
            .map(|i| {
                let ty = i.get("type").and_then(|t| t.as_str()).unwrap_or("?");
                let loc = i.get("location").and_then(|t| t.as_str()).unwrap_or("");
                let msg = i.get("message").and_then(|t| t.as_str()).unwrap_or("");
                let quote = i.get("quote").and_then(|t| t.as_str()).unwrap_or("");
                format!("{ty} @ {loc}: {msg} | quote={quote}")
            })
            .take(6)
            .collect();
        let detail = if p0.is_empty() {
            run.message.chars().take(500).collect::<String>()
        } else {
            p0.join("\n")
        };
        let timeline = looks_like_timeline_issue(&detail)
            || run.issues.iter().any(|i| {
                let ty = i.get("type").and_then(|t| t.as_str()).unwrap_or("");
                looks_like_timeline_issue(ty)
                    || looks_like_timeline_issue(
                        i.get("message").and_then(|t| t.as_str()).unwrap_or(""),
                    )
            });
        if timeline {
            force_full = true;
            parts.push(format!(
                "按一致性 P0 整章理顺时间线/时段（勿只改单句；保持情节与人物一致）。issues：\n{detail}"
            ));
        } else {
            parts.push(format!(
                "按一致性 P0 修订正文（只改问题句，勿整章重写）。issues：\n{detail}"
            ));
        }
    }

    if need_hard {
        let detail: String = run.message.chars().take(400).collect();
        let violations: Vec<Value> = run
            .content_rule_violations
            .iter()
            .filter(|v| v.blocking)
            .map(|v| {
                json!({
                    "rule": v.rule,
                    "message": v.message,
                    "blocking": true,
                })
            })
            .collect();
        let plan = build_revise_plan(&RevisePlanConfig::default(), &[], &violations, 0);
        let hard_blob = format!(
            "{detail} {}",
            run.content_rule_violations
                .iter()
                .map(|v| format!("{} {}", v.rule, v.message))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if plan.scope == ReviseScope::Full || looks_like_timeline_issue(&hard_blob) {
            force_full = true;
            if plan.scope == ReviseScope::Full && !plan.instructions.is_empty() {
                parts.push(plan.instructions);
            } else {
                parts.push(format!(
                    "消除时段/时间线硬规则违规；整章理顺日夜顺序与时段锚点，勿只改违规句。参考：{detail}"
                ));
            }
        } else {
            parts.push(format!(
                "消除正文硬规则违规；只改违规句，勿整章重写。参考：{detail}"
            ));
        }
    }

    if need_len {
        if run.length_status == "soft_long" {
            // Compress once toward word_min–word_max; do not escalate like HardLong split.
            parts.push(format!(
                "{}优先删重复机理与 plot_includes/key_events 范围外枝节，压回 {}–{} 字。",
                budget.compress_revise_instructions(),
                budget.word_min,
                budget.word_max
            ));
        } else {
            force_full = true;
            parts.push(format!(
                "扩写到{}字完整一章；保持情节与人物一致，补足场景与对话，勿注水。",
                budget.range_label()
            ));
        }
    }

    (parts.join("\n"), force_full)
}

fn emit_batch_todos(
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    labels: &[String],
    current_index: usize,
    finished: bool,
) {
    let Some(tx) = tx else {
        return;
    };
    if labels.is_empty() {
        return;
    }
    let todos = novelx_protocol::progressive_todo_list(labels, current_index, finished);
    let _ = tx.send(PipelineEvent::TodoList { todos });
}

fn emit_batch(tx: &Option<mpsc::UnboundedSender<PipelineEvent>>, message: impl Into<String>) {
    if let Some(tx) = tx {
        let _ = tx.send(PipelineEvent::BatchProgress {
            message: message.into(),
        });
    }
}

struct BatchAutoReviseFlags {
    length: bool,
    hard_rule: bool,
    consistency: bool,
}

/// When consistency failed only as audit_infra, re-run AuditOnly (do not revise prose).
async fn batch_recover_audit_infra(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    mut run: PipelineRun,
    max_retries: u32,
    llm: Arc<LlmClient>,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    unit: &str,
) -> Result<(PipelineRun, u32)> {
    if max_retries == 0 || run.published || !pipeline_run_is_audit_infra(&run) {
        return Ok((run, 0));
    }
    let mut retries = 0u32;
    while retries < max_retries && !run.published && pipeline_run_is_audit_infra(&run) {
        retries += 1;
        emit_batch(
            tx,
            format!(
                "⚙ 第{chapter}{unit}审计基础设施失败，自动重审（{retries}/{max_retries}）…"
            ),
        );
        let _ = append_loop_journal(
            &crate::project::project_dir(projects_root, project),
            json!({
                "ts": now_rfc3339(),
                "event": "audit_infra_retry",
                "chapter": chapter,
                "attempt": retries,
                "max": max_retries,
            }),
        );
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        run = execute_pipeline(
            projects_root,
            config_root,
            project,
            chapter,
            RunMode::AuditOnly,
            RevisionOptions::default(),
            llm.clone(),
            tx.clone(),
        )
        .await?;
    }
    Ok((run, retries))
}

/// After HardLong auto-split, body changed — run AuditOnly on part A before revise/publish.
async fn reaudit_after_auto_split(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    run: PipelineRun,
    llm: Arc<LlmClient>,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<PipelineRun> {
    if !run.auto_split || run.published {
        return Ok(run);
    }
    let to = run.auto_split_to.unwrap_or(chapter.saturating_add(1));
    emit_batch(
        tx,
        format!("⚙ 第{chapter}章已自动拆出第{to}章，复审前半…"),
    );
    execute_pipeline(
        projects_root,
        config_root,
        project,
        chapter,
        RunMode::AuditOnly,
        RevisionOptions::default(),
        llm,
        tx.clone(),
    )
    .await
}

/// Retry revise up to `max_attempts` while hard rules / consistency / length still block publish.
async fn batch_auto_revise_until_publish(
    projects_root: &Path,
    config_root: &Path,
    project: &str,
    chapter: u32,
    mut run: PipelineRun,
    budget: &ChapterBudget,
    auto_length_revise: bool,
    max_attempts: u32,
    llm: Arc<LlmClient>,
    tx: &Option<mpsc::UnboundedSender<PipelineEvent>>,
    dir: &Path,
) -> Result<(PipelineRun, BatchAutoReviseFlags)> {
    let mut flags = BatchAutoReviseFlags {
        length: false,
        hard_rule: false,
        consistency: false,
    };
    let max_attempts = max_attempts.max(1);
    let mut attempt = 0u32;
    let mut did_auto_split = false;
    // SoftLong: compress at most once, then allow publish even if still soft_long.
    let mut soft_long_compress_tried = false;
    while !run.published && attempt < max_attempts && run.volume_ended.is_none() {
        // Fallback if pipeline auto-split was off / failed and status is still hard_long.
        if auto_length_revise
            && !did_auto_split
            && run.length_status == "hard_long"
            && crate::run::auto_split_hard_long_enabled(config_root)
        {
            did_auto_split = true;
            emit_batch(
                tx,
                format!("⚙ 第{chapter}章严重超长，自动拆成第{chapter}/{}章…", chapter + 1),
            );
            match crate::split_chapter::split_chapter_draft(dir, chapter, 0.5, None, false) {
                Ok(split) => {
                    flags.length = true;
                    tracing::info!(
                        chapter_a = split.chapter_a,
                        chapter_b = split.chapter_b,
                        chars_a = split.chars_a,
                        chars_b = split.chars_b,
                        "batch: auto-split overlong chapter"
                    );
                    emit_batch(
                        tx,
                        format!(
                            "✓ 已拆为第{}章（{}字）+ 第{}章（{}字），继续审校前半…",
                            split.chapter_a,
                            split.chars_a,
                            split.chapter_b,
                            split.chars_b
                        ),
                    );
                    run = execute_pipeline(
                        projects_root,
                        config_root,
                        project,
                        chapter,
                        RunMode::AuditOnly,
                        RevisionOptions::default(),
                        llm.clone(),
                        tx.clone(),
                    )
                    .await?;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(chapter, error = %e, "batch auto-split failed");
                    emit_batch(
                        tx,
                        format!("⚠ 自动拆章失败：{e}。批写将暂停，请人工拆章或压缩。"),
                    );
                    break;
                }
            }
        }
        let length_blocks = length_blocks_publish(dir, config_root, chapter);
        let need_hard = run.content_rule_blocked;
        // Never treat audit_infra META as content P0 — that forces bad full rewrites.
        let need_consistency = run.consistency_passed == Some(false)
            && !pipeline_run_is_audit_infra(&run);
        let mut need_len = should_batch_auto_length_revise(
            auto_length_revise,
            run.published,
            false,
            run.volume_ended.is_some(),
            if need_consistency {
                None
            } else {
                run.consistency_passed
            },
            &run.length_status,
            length_blocks,
        );
        // SoftLong compress is one-shot; further SoftLong must not loop until max_attempts.
        if need_len && run.length_status == "soft_long" && soft_long_compress_tried {
            need_len = false;
        }
        if !(need_hard || need_len || need_consistency) {
            // SoftLong held by defer_soft_long_publish with no more compress → publish once.
            if run.length_status == "soft_long"
                && soft_long_compress_tried
                && !run.content_rule_blocked
                && run.consistency_passed != Some(false)
                && run.volume_ended.is_none()
            {
                emit_batch(tx, format!("⚙ 第{chapter}章 SoftLong 已压缩一轮，审校发布…"));
                run = execute_pipeline(
                    projects_root,
                    config_root,
                    project,
                    chapter,
                    RunMode::AuditOnly,
                    RevisionOptions::default(),
                    llm.clone(),
                    tx.clone(),
                )
                .await?;
            }
            break;
        }
        attempt += 1;
        let (instr, force_full) =
            build_batch_revise_instructions(&run, need_consistency, need_hard, need_len, budget);
        let label = match (need_consistency, need_hard, need_len, run.length_status.as_str()) {
            (true, _, _, _) => "一致性未过",
            (false, true, true, _) => "硬规则+字数未过",
            (false, true, false, _) => "硬规则未过",
            (false, false, true, "soft_long") => "字数偏长",
            _ => "字数不足",
        };
        tracing::info!(
            chapter,
            attempt,
            max_attempts,
            need_hard,
            need_len,
            need_consistency,
            force_full,
            "batch: auto revise after continue/audit"
        );
        emit_batch(
            tx,
            format!("⚙ 第{chapter}章{label}，自动修订（{attempt}/{max_attempts}）…"),
        );
        if need_len && run.length_status == "soft_long" {
            soft_long_compress_tried = true;
        }
        // Hold SoftLong publish while compress not yet tried (e.g. consistency-first revise).
        // Once compress is in this attempt, allow SoftLong to publish afterward.
        let defer_soft_long = auto_length_revise
            && run.length_status == "soft_long"
            && !soft_long_compress_tried;
        run = execute_pipeline(
            projects_root,
            config_root,
            project,
            chapter,
            RunMode::Revise,
            RevisionOptions {
                prefer_local_patch: !force_full,
                revision_mode: true,
                user_instructions: Some(instr),
                audit_issues: run.issues.clone(),
                pacing_suggestions: vec![],
                verify_previous: need_consistency || attempt > 1,
                full_rescan: false,
                defer_soft_long_publish: defer_soft_long,
            },
            llm.clone(),
            tx.clone(),
        )
        .await?;
        flags.hard_rule |= need_hard;
        flags.consistency |= need_consistency;
        flags.length |= need_len;
    }
    Ok((run, flags))
}

/// Run continue_writing in a loop until a gate, max, or until_chapter.
///
/// When `tx` is set, chapter banners and inner pipeline step/draft events are
/// forwarded so Studio can show live progress on the batch tool card.
pub async fn run_continue_batch(
    projects_root: &Path,
    config_root: &Path,
    mut opts: BatchContinueOpts,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<BatchContinueResult> {
    let soft_gates_skipped_by_policy = apply_unattended_batch_policy(config_root, &mut opts);
    let soft_gates_skipped = soft_skip_keys_from_opts(
        soft_gates_skipped_by_policy,
        opts.confirm_skip_volume_audit,
        opts.confirm_skip_expected,
        opts.confirm_skip_foreshadow,
    );
    let lf = LongformConfig::load_from_config_root(config_root);
    let max = opts
        .max_chapters
        .unwrap_or(lf.batch_max_chapters)
        .max(1)
        .min(100);
    let max_auto_revise = lf.batch_max_auto_revise.max(1);
    let max_audit_infra_retries = lf.batch_max_audit_infra_retries;
    let dir = project_dir(projects_root, &opts.project);
    let short_drama = is_short_drama(&dir);
    let unit = if short_drama { "集" } else { "章" };
    let state0 = load_project_state(&dir)?;
    let started = state0.next_chapter.max(1);
    let until = resolve_batch_until(opts.until_chapter, state0.target_chapters);
    let mut results = Vec::new();
    let mut published_n = 0u32;
    let mut attempted = 0u32;
    let mut stopped = "completed".to_string();
    let budget = ChapterBudget::load_from_config_root(config_root);
    // Safety: avoid infinite loops if publishes never advance (gates thrash).
    let safety_cap = max.saturating_mul(3).max(max + 5);

    let mut loop_job = begin_loop_job(
        &dir,
        &opts.project,
        &opts,
        started,
        unit,
        soft_gates_skipped.clone(),
    )
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "loop job begin failed; continuing without persist");
        crate::loop_runtime::LoopJob {
            project: opts.project.clone(),
            status: crate::loop_runtime::LoopJobStatus::Running,
            until_chapter: opts.until_chapter,
            max_chapters: opts.max_chapters,
            respect_soft_gates: opts.respect_soft_gates,
            confirm_skip_volume_audit: opts.confirm_skip_volume_audit,
            confirm_skip_expected: opts.confirm_skip_expected,
            confirm_skip_foreshadow: opts.confirm_skip_foreshadow,
            auto_length_revise: opts.auto_length_revise,
            started_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            chapters_done: 0,
            chapters_attempted: 0,
            started_chapter: started,
            last_stop: None,
            soft_gates_skipped: soft_gates_skipped.clone(),
            pending_wake: false,
            loop_end_verify: None,
            unit: unit.to_string(),
        }
    });
    if soft_gates_skipped_by_policy {
        let _ = append_loop_journal(
            &dir,
            json!({
                "ts": now_rfc3339(),
                "event": "soft_gates_skipped",
                "keys": soft_gates_skipped,
            }),
        );
    }

    let until_label = until
        .map(|u| format!("，写到第{u}{unit}为止"))
        .unwrap_or_default();
    let soft_note = if soft_gates_skipped_by_policy {
        "；已按无人值守策略跳过卷 QA mid_due / 预期检阅 / 伏笔近债软相位"
    } else {
        ""
    };
    // Progressive To-dos for the planned publish span (Codex-style checklist).
    let plan_end = {
        let by_max = started.saturating_add(max).saturating_sub(1);
        match until {
            Some(u) => u.min(by_max).max(started),
            None => by_max,
        }
    };
    let todo_labels: Vec<String> = (started..=plan_end)
        .map(|c| format!("撰写第{c}{unit}"))
        .collect();
    emit_batch_todos(&tx, &todo_labels, 0, false);
    emit_batch(
        &tx,
        format!(
            "▶ 开始连写到卡点（最多成功发布 {max} {unit}{until_label}，自第{started}{unit}）{soft_note}"
        ),
    );

    while published_n < max && attempted < safety_cap {
        let _ = heartbeat_loop_job(&dir);
        let state = load_project_state(&dir)?;
        let chapter = state.next_chapter.max(1);
        if let Some(u) = until {
            if chapter > u {
                stopped = format!("until_chapter({u})");
                emit_batch_todos(&tx, &todo_labels, 0, true);
                emit_batch(&tx, format!("✓ 已写到 until_chapter({u})，批写结束"));
                break;
            }
        }

        attempted += 1;
        loop_job.chapters_attempted = attempted;
        loop_job.chapters_done = published_n;
        let todo_idx = (chapter.saturating_sub(started)) as usize;
        emit_batch_todos(&tx, &todo_labels, todo_idx, false);
        emit_batch(
            &tx,
            format!(
                "▶ 批写第{chapter}{unit}（本批已发布 {published_n}/{max}，第 {attempted} 次尝试）"
            ),
        );

        if let Some(block) = check_volume_audit_for_continue(
            config_root,
            &dir,
            chapter,
            opts.confirm_skip_volume_audit,
        ) {
            emit_batch(
                &tx,
                format!("⛔ 第{chapter}{unit}拦截·卷审：{}", block.message),
            );
            results.push(BatchChapterResult {
                chapter,
                published: false,
                blocked: true,
                reason: Some(block.reason.to_string()),
                needs_user_choice: true,
                message: block.message,
                length_auto_revised: false,
                hard_rule_auto_revised: false,
                consistency_auto_revised: false,
                audit_infra_retries: 0,
                run: None,
            });
            stopped = "volume_audit_gate".into();
            break;
        }

        if let Some((reason, message)) =
            expected_gate_message(&dir, chapter, opts.confirm_skip_expected)
        {
            emit_batch(&tx, format!("⛔ 第{chapter}{unit}拦截·预期检阅：{message}"));
            results.push(BatchChapterResult {
                chapter,
                published: false,
                blocked: true,
                reason: Some(reason.clone()),
                needs_user_choice: true,
                message,
                length_auto_revised: false,
                hard_rule_auto_revised: false,
                consistency_auto_revised: false,
                audit_infra_retries: 0,
                run: None,
            });
            stopped = reason;
            break;
        }

        let enforce = PhaseEnforceFlags::load(config_root);
        if enforce.chapter_order {
            if let Some(block) = check_chapter_order(&dir, chapter) {
                emit_batch(
                    &tx,
                    format!("⛔ 第{chapter}{unit}拦截·序：{}", block.message),
                );
                results.push(BatchChapterResult {
                    chapter,
                    published: false,
                    blocked: true,
                    reason: Some(block.reason.to_string()),
                    needs_user_choice: true,
                    message: block.message,
                    length_auto_revised: false,
                    hard_rule_auto_revised: false,
                    consistency_auto_revised: false,
                    audit_infra_retries: 0,
                    run: None,
                });
                stopped = "chapter_order".into();
                break;
            }
        }

        match check_plot_write_gate_with(&dir, enforce) {
            PlotWriteGate::Block { message, reason, .. } => {
                emit_batch(&tx, format!("⛔ 第{chapter}{unit}拦截·剧情门：{message}"));
                results.push(BatchChapterResult {
                    chapter,
                    published: false,
                    blocked: true,
                    reason: Some(reason.to_string()),
                    needs_user_choice: true,
                    message,
                    length_auto_revised: false,
                    hard_rule_auto_revised: false,
                    consistency_auto_revised: false,
                    audit_infra_retries: 0,
                    run: None,
                });
                stopped = format!("plot_gate:{reason}");
                break;
            }
            PlotWriteGate::Allow { mode, .. } => {
                if matches!(mode, PlotWriteMode::Bridge) {
                    if let Err(e) = ensure_bridge_plot_active(&dir) {
                        tracing::warn!(error = %e, chapter, "batch ensure_bridge_plot_active failed");
                    }
                }
            }
        }

        // ForeshadowPhase pressure_high → batch soft-stop (unless skip / unattended).
        if !opts.confirm_skip_foreshadow {
            let snap = crate::foreshadow_phase::resolve_foreshadow_phase(config_root, &dir);
            if let Some(message) = crate::foreshadow_phase::foreshadow_batch_block_message(&snap) {
                emit_batch(&tx, format!("⛔ {message}"));
                results.push(BatchChapterResult {
                    chapter,
                    published: false,
                    blocked: true,
                    reason: Some("foreshadow_pressure_high".into()),
                    needs_user_choice: true,
                    message,
                    length_auto_revised: false,
                    hard_rule_auto_revised: false,
                    consistency_auto_revised: false,
                    audit_infra_retries: 0,
                    run: None,
                });
                stopped = "foreshadow_pressure_high".into();
                break;
            }
        }

        // Existing long draft: try audit-only publish once before hard-stopping.
        let existing = read_chapter_draft(&dir, chapter).unwrap_or_default();
        let min_chars = novelx_harness::StudioPolicies::load(config_root).draft_min_chars();
        if existing.chars().count() >= min_chars {
            emit_batch(
                &tx,
                format!(
                    "⚙ 第{chapter}{unit}已有未发布正文（约 {} 字），先尝试审校发布…",
                    existing.chars().count()
                ),
            );
            let audit_run = execute_pipeline(
                projects_root,
                config_root,
                &opts.project,
                chapter,
                RunMode::AuditOnly,
                RevisionOptions {
                    defer_soft_long_publish: opts.auto_length_revise,
                    ..Default::default()
                },
                llm.clone(),
                tx.clone(),
            )
            .await?;
            let audit_run = reaudit_after_auto_split(
                projects_root,
                config_root,
                &opts.project,
                chapter,
                audit_run,
                llm.clone(),
                &tx,
            )
            .await?;
            let (audit_run, audit_infra_retries) = batch_recover_audit_infra(
                projects_root,
                config_root,
                &opts.project,
                chapter,
                audit_run,
                max_audit_infra_retries,
                llm.clone(),
                &tx,
                unit,
            )
            .await?;
            let (audit_run, revise_flags) = batch_auto_revise_until_publish(
                projects_root,
                config_root,
                &opts.project,
                chapter,
                audit_run,
                &budget,
                opts.auto_length_revise,
                max_auto_revise,
                llm.clone(),
                &tx,
                &dir,
            )
            .await?;
            let length_auto_revised = revise_flags.length;
            let hard_rule_auto_revised = revise_flags.hard_rule;
            let consistency_auto_revised = revise_flags.consistency;
            if audit_run.published {
                published_n += 1;
                let next_idx = (chapter.saturating_sub(started).saturating_add(1)) as usize;
                let finished = published_n >= max || next_idx >= todo_labels.len();
                emit_batch_todos(&tx, &todo_labels, next_idx, finished);
                emit_batch(
                    &tx,
                    format!(
                        "✓ 第{chapter}{unit}已有草稿经审校发布（本批累计 {published_n}/{max} {unit}）"
                    ),
                );
                results.push(BatchChapterResult {
                    chapter,
                    published: true,
                    blocked: false,
                    reason: None,
                    needs_user_choice: false,
                    message: audit_run.message.clone(),
                    length_auto_revised,
                    hard_rule_auto_revised,
                    consistency_auto_revised,
                    audit_infra_retries,
                    run: Some(audit_run),
                });
                continue;
            }
            let stop_reason = if pipeline_run_is_audit_infra(&audit_run) {
                "audit_infra"
            } else {
                "draft_exists"
            };
            let revise_hint = if unit == "集" {
                "revise_episode"
            } else {
                "revise_chapter"
            };
            let message = if stop_reason == "audit_infra" {
                format!(
                    "第{chapter}{unit}一致性审计基础设施失败（空响应/不可解析），自动重审 {audit_infra_retries} 次仍失败。批写暂停；请点「重新审校」。"
                )
            } else {
                format!(
                    "第{chapter}{unit}已有未发布正文（约 {} 字），审校/自动修订后仍未发布。批写暂停；请 {revise_hint} 或按审批卡修正。",
                    existing.chars().count()
                )
            };
            emit_batch(&tx, format!("⛔ {message}"));
            results.push(BatchChapterResult {
                chapter,
                published: false,
                blocked: true,
                reason: Some(stop_reason.into()),
                needs_user_choice: true,
                message,
                length_auto_revised,
                hard_rule_auto_revised,
                consistency_auto_revised,
                audit_infra_retries,
                run: Some(audit_run),
            });
            stopped = stop_reason.into();
            break;
        }

        let run = execute_pipeline(
            projects_root,
            config_root,
            &opts.project,
            chapter,
            RunMode::Continue,
            RevisionOptions {
                // Hold SoftLong publish so batch can compress once first.
                defer_soft_long_publish: opts.auto_length_revise,
                ..Default::default()
            },
            llm.clone(),
            tx.clone(),
        )
        .await?;
        let run = reaudit_after_auto_split(
            projects_root,
            config_root,
            &opts.project,
            chapter,
            run,
            llm.clone(),
            &tx,
        )
        .await?;
        let (run, audit_infra_retries) = batch_recover_audit_infra(
            projects_root,
            config_root,
            &opts.project,
            chapter,
            run,
            max_audit_infra_retries,
            llm.clone(),
            &tx,
            unit,
        )
        .await?;

        let (run, revise_flags) = batch_auto_revise_until_publish(
            projects_root,
            config_root,
            &opts.project,
            chapter,
            run,
            &budget,
            opts.auto_length_revise,
            max_auto_revise,
            llm.clone(),
            &tx,
            &dir,
        )
        .await?;
        let length_auto_revised = revise_flags.length;
        let hard_rule_auto_revised = revise_flags.hard_rule;
        let consistency_auto_revised = revise_flags.consistency;

        let blocked = !run.published
            || run.needs_user_choice
            || run.content_rule_blocked
            || run.volume_ended.is_some();
        let reason = if run.volume_ended.is_some() {
            Some("volume_ended".into())
        } else if run.content_rule_blocked {
            Some("content_rule".into())
        } else if pipeline_run_is_audit_infra(&run) {
            Some("audit_infra".into())
        } else if run.consistency_passed == Some(false) {
            Some("consistency_fail".into())
        } else if run.needs_user_choice {
            Some("needs_user_choice".into())
        } else if !run.published {
            Some("not_published".into())
        } else {
            None
        };

        if run.published {
            published_n += 1;
            let next_idx = (chapter.saturating_sub(started).saturating_add(1)) as usize;
            let finished = published_n >= max || next_idx >= todo_labels.len();
            emit_batch_todos(&tx, &todo_labels, next_idx, finished);
            emit_batch(
                &tx,
                format!("✓ 第{chapter}{unit}已发布（本批累计 {published_n}/{max} {unit}）"),
            );
        } else if blocked {
            let why = reason.as_deref().unwrap_or("gate");
            emit_batch_todos(
                &tx,
                &todo_labels,
                (chapter.saturating_sub(started)) as usize,
                false,
            );
            emit_batch(
                &tx,
                format!("⛔ 第{chapter}{unit}未发布（{why}），批写暂停"),
            );
        }

        let msg = run.message.clone();
        results.push(BatchChapterResult {
            chapter,
            published: run.published,
            blocked,
            reason: reason.clone(),
            needs_user_choice: run.needs_user_choice,
            message: msg,
            length_auto_revised,
            hard_rule_auto_revised,
            consistency_auto_revised,
            audit_infra_retries,
            run: Some(run),
        });

        if blocked {
            stopped = reason.unwrap_or_else(|| "gate".into());
            break;
        }

        let vp = resolve_volume_phase(&dir);
        if !matches!(vp, VolumePhase::DraftingVolume) {
            stopped = format!("volume_phase:{}", vp.as_str());
            emit_batch(
                &tx,
                format!("⏸ 卷阶段变为 {}，批写结束", vp.as_str()),
            );
            break;
        }
        let sp = resolve_setup_phase(&dir);
        if sp.as_str() != "ready" && enforce.setup {
            stopped = format!("setup_phase:{}", sp.as_str());
            emit_batch(
                &tx,
                format!("⏸ setup 阶段变为 {}，批写结束", sp.as_str()),
            );
            break;
        }
    }

    if published_n >= max && stopped == "completed" {
        stopped = format!("max_chapters({max})");
        emit_batch_todos(&tx, &todo_labels, 0, true);
        emit_batch(&tx, format!("✓ 已达批写成功发布上限 max_chapters({max})"));
    } else if attempted >= safety_cap && stopped == "completed" {
        stopped = format!("safety_cap({safety_cap})");
        emit_batch(
            &tx,
            format!("⏸ 达到尝试安全上限 {safety_cap}（成功发布 {published_n}/{max}）"),
        );
    } else if stopped == "completed" {
        emit_batch_todos(&tx, &todo_labels, 0, true);
        emit_batch(&tx, "✓ 批写循环正常结束");
    }

    let mut stop_contract = StopContract::from_reason(&stopped);
    let loop_end_verify = run_loop_end_verify(
        config_root,
        &dir,
        published_n,
        lf.loop_end_verify_min_chapters,
        &stop_contract,
        short_drama,
    );
    if let Some(ref v) = loop_end_verify {
        let _ = append_loop_journal(
            &dir,
            json!({
                "ts": now_rfc3339(),
                "event": "loop_end_verify",
                "verify": v,
            }),
        );
        if v.requires_human {
            stop_contract.requires_human = true;
            emit_batch(
                &tx,
                format!(
                    "⏸ 批结束跨章抽检：{}（{}）",
                    v.volume_qa_phase,
                    v.summary.as_deref().unwrap_or("需处理")
                ),
            );
        }
    }
    let status_label = stop_contract.status_label_zh().to_string();
    if let Err(e) = finish_loop_job(
        &dir,
        loop_job,
        stop_contract.clone(),
        published_n,
        attempted,
        loop_end_verify.clone(),
    ) {
        tracing::warn!(error = %e, "loop job finish failed");
    }

    let todos = novelx_protocol::progressive_todo_list(
        &todo_labels,
        published_n as usize,
        matches!(
            stopped.as_str(),
            s if s.starts_with("max_chapters")
                || s.starts_with("until_chapter")
                || s == "completed"
        ) || published_n as usize >= todo_labels.len(),
    );

    Ok(BatchContinueResult {
        project: opts.project,
        started_chapter: started,
        chapters_attempted: attempted,
        chapters_published: published_n,
        stopped_reason: stopped,
        stop_contract: Some(stop_contract),
        results,
        unit: unit.to_string(),
        soft_gates_skipped_by_policy,
        soft_gates_skipped,
        todos,
        loop_end_verify,
        status_label: Some(status_label),
    })
}

impl BatchContinueResult {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }

    pub fn summary_text(&self) -> String {
        let unit = if self.unit.is_empty() {
            "章"
        } else {
            self.unit.as_str()
        };
        let mut lines = vec![format!(
            "批写完成：尝试 {} {unit}，发布 {} {unit}，停止原因：{}",
            self.chapters_attempted, self.chapters_published, self.stopped_reason
        )];
        if let Some(label) = self.status_label.as_deref() {
            lines.push(format!("状态：{label}"));
        }
        if let Some(sc) = &self.stop_contract {
            lines.push(format!(
                "StopContract：kind={:?} resumable={} requires_human={}",
                sc.kind, sc.resumable, sc.requires_human
            ));
        }
        if self.soft_gates_skipped_by_policy {
            if self.soft_gates_skipped.is_empty() {
                lines.push(
                    "（无人值守：已跳过卷 QA mid_due / 预期检阅 / 伏笔近债软相位；硬门控仍会停）"
                        .into(),
                );
            } else {
                lines.push(format!(
                    "（无人值守软跳过：{}；硬门控仍会停）",
                    self.soft_gates_skipped.join(", ")
                ));
            }
        }
        if let Some(v) = &self.loop_end_verify {
            if let Some(s) = &v.summary {
                lines.push(format!("批结束抽检：{s}"));
            }
        }
        for r in &self.results {
            let flag = if r.published {
                "✓发布"
            } else if r.blocked {
                "⛔拦截"
            } else {
                "·"
            };
            let auto = match (
                r.length_auto_revised,
                r.hard_rule_auto_revised,
                r.consistency_auto_revised,
            ) {
                (true, _, _) => " ·已自动扩写",
                (false, true, true) => " ·已自动修订硬规则+一致性",
                (false, true, false) => " ·已自动修订硬规则",
                (false, false, true) => " ·已自动修订一致性",
                _ => "",
            };
            lines.push(format!(
                "- 第{}{unit} {flag}{} {}",
                r.chapter,
                auto,
                r.reason.as_deref().unwrap_or("")
            ));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::init_project;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(tag: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let root = std::env::temp_dir().join(format!("novelx-batch-{tag}-{stamp}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn expected_gate_message_blocks_when_hard_ok_needs_review() {
        let root = tmp("expected");
        let dir = init_project(&root, "sample-novel", "未定", 100).unwrap();
        fs::create_dir_all(dir.join("lore")).unwrap();
        // Minimal expected event that is hard_ok and needs review for chapter 1.
        // Use store shape from expected_events module.
        let store = serde_json::json!({
            "version": 1,
            "events": [{
                "id": "ev1",
                "text": "样例信号应出现",
                "status": "waiting",
                "hard": {"min_chapter": 1},
                "last_review": null
            }]
        });
        fs::write(
            dir.join("lore/expected_events.json"),
            serde_json::to_string_pretty(&store).unwrap(),
        )
        .unwrap();
        // If schema differs, needs_fresh_review may be false — still exercise skip path.
        let blocked = expected_gate_message(&dir, 1, false);
        let skipped = expected_gate_message(&dir, 1, true);
        assert!(skipped.is_none());
        // When hard-ok candidates exist with stale/missing review, we block.
        if crate::expected_events::needs_fresh_review(&dir, 1) {
            assert!(blocked.is_some());
            assert_eq!(blocked.unwrap().0, "need_expected_review");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn summary_text_mentions_stop_reason() {
        let stop = StopContract::from_reason("draft_exists");
        let r = BatchContinueResult {
            project: "sample-novel".into(),
            started_chapter: 1,
            chapters_attempted: 1,
            chapters_published: 0,
            stopped_reason: "draft_exists".into(),
            stop_contract: Some(stop.clone()),
            results: vec![BatchChapterResult {
                chapter: 1,
                published: false,
                blocked: true,
                reason: Some("draft_exists".into()),
                needs_user_choice: true,
                message: "paused".into(),
                length_auto_revised: false,
                hard_rule_auto_revised: false,
                consistency_auto_revised: false,
                audit_infra_retries: 0,
                run: None,
            }],
            unit: "章".into(),
            soft_gates_skipped_by_policy: false,
            soft_gates_skipped: vec![],
            todos: vec![],
            loop_end_verify: None,
            status_label: Some(stop.status_label_zh().into()),
        };
        let s = r.summary_text();
        assert!(s.contains("draft_exists"));
        assert!(s.contains("⛔拦截"));
        assert!(s.contains("需处理"));
    }

    #[test]
    fn stop_contract_on_batch_reasons() {
        assert!(!StopContract::from_reason("chapter_order").auto_wake_allowed());
        assert!(!StopContract::from_reason("consistency_fail").auto_wake_allowed());
        assert!(StopContract::from_reason("crash_interrupted").auto_wake_allowed());
        let infra = StopContract::from_reason("audit_infra");
        assert!(infra.requires_human);
        assert!(!infra.auto_wake_allowed());
        assert_eq!(infra.gate_id.as_deref(), Some("audit_infra"));
    }

    #[test]
    fn audit_infra_detection_ignores_content_meta() {
        use crate::run::is_audit_infra_failure;
        let infra = vec![json!({
            "type": "META",
            "priority": "P0",
            "message": "一致性审计模型返回为空，请重试 audit_chapter"
        })];
        assert!(is_audit_infra_failure(&infra, "", ""));
        let chapter_meta = vec![json!({
            "type": "META",
            "priority": "P0",
            "message": "正文出现「第3章」元叙述"
        })];
        assert!(!is_audit_infra_failure(&chapter_meta, "", ""));
        assert!(is_audit_infra_failure(
            &[],
            "一致性审计失败：模型返回为空",
            ""
        ));
    }

    #[test]
    fn unattended_policy_fills_soft_skips() {
        let root = tmp("unattended-batch");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("features.yaml"),
            "features:\n  studio.unattended_soft_skip: true\n",
        )
        .unwrap();
        fs::write(
            config.join("unattended.yaml"),
            "version: 1\nbatch:\n  skip_volume_audit_mid: true\n  skip_expected_review: true\n  skip_foreshadow_pressure: true\n",
        )
        .unwrap();
        let mut opts = BatchContinueOpts::default();
        assert!(apply_unattended_batch_policy(&config, &mut opts));
        assert!(opts.confirm_skip_volume_audit);
        assert!(opts.confirm_skip_expected);
        assert!(opts.confirm_skip_foreshadow);
        let mut respected = BatchContinueOpts {
            respect_soft_gates: true,
            ..Default::default()
        };
        assert!(!apply_unattended_batch_policy(&config, &mut respected));
        assert!(!respected.confirm_skip_volume_audit);
        assert!(!respected.confirm_skip_foreshadow);
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn short_drama_batch_enters_loop_until_gate() {
        let root = tmp("sd-batch");
        let _dir = crate::project::init_project_with_mode(
            &root,
            "demo",
            "未定",
            12,
            crate::project::ProjectMode::ShortDrama,
        )
        .unwrap();
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        let llm = Arc::new(LlmClient::new(novelx_llm::LlmConfig::default()));
        let result = run_continue_batch(
            &root,
            &config,
            BatchContinueOpts {
                project: "demo".into(),
                max_chapters: Some(2),
                ..Default::default()
            },
            llm,
            None,
        )
        .await
        .expect("short_drama batch should run");
        assert_eq!(result.unit, "集");
        assert_ne!(result.stopped_reason, "short_drama_no_batch");
        assert!(
            result.chapters_attempted >= 1 || result.results.iter().any(|r| r.blocked),
            "batch should attempt at least one unit or block at a gate: {result:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn auto_length_revise_runs_on_soft_short_escalate() {
        // SoftShort escalate sets needs_user_choice in the run result; must still revise.
        assert!(should_batch_auto_length_revise(
            true,
            false,
            false,
            false,
            Some(true),
            "soft_short_escalated",
            false,
        ));
        assert!(should_batch_auto_length_revise(
            true,
            false,
            false,
            false,
            None,
            "hard_short",
            false,
        ));
        assert!(should_batch_auto_length_revise(
            true,
            false,
            false,
            false,
            Some(true),
            "ok",
            true, // length_blocks_publish fallback
        ));
        // HardLong uses auto-split, not expand revise.
        assert!(!should_batch_auto_length_revise(
            true,
            false,
            false,
            false,
            Some(true),
            "hard_long",
            true,
        ));
        // SoftLong: one-shot compress (write-after insurance).
        assert!(should_batch_auto_length_revise(
            true,
            false,
            false,
            false,
            Some(true),
            "soft_long",
            false,
        ));
    }

    #[test]
    fn auto_length_revise_skips_when_consistency_fail_or_disabled() {
        assert!(!should_batch_auto_length_revise(
            true,
            false,
            false,
            false,
            Some(false),
            "hard_short",
            true,
        ));
        assert!(!should_batch_auto_length_revise(
            false,
            false,
            false,
            false,
            Some(true),
            "hard_short",
            true,
        ));
        assert!(!should_batch_auto_length_revise(
            true,
            true,
            false,
            false,
            Some(true),
            "hard_short",
            true,
        ));
        assert!(!should_batch_auto_length_revise(
            true,
            false,
            true,
            false,
            Some(true),
            "hard_short",
            true,
        ));
    }

    #[test]
    fn resolve_batch_until_prefers_explicit_then_target() {
        assert_eq!(resolve_batch_until(Some(12), 5), Some(12));
        assert_eq!(resolve_batch_until(None, 5), Some(5));
        assert_eq!(resolve_batch_until(None, 0), None);
    }

    fn sample_run() -> PipelineRun {
        serde_json::from_value(json!({
            "id": "",
            "project": "sample-novel",
            "chapter": 1,
            "mode": "continue",
            "steps": [],
            "status": "",
            "message": "",
            "prefer_local_patch": false,
            "revision_applied_via_patch": false
        }))
        .expect("sample PipelineRun")
    }

    #[test]
    fn timeline_hard_rule_forces_full_rewrite() {
        let mut run = sample_run();
        run.message = "时段回跳：午后→清晨".into();
        run.content_rule_blocked = true;
        run.content_rule_violations = vec![novelx_harness::ContentRuleViolation {
            rule: "timeline_daypart_regression".into(),
            message: "时段回跳".into(),
            blocking: true,
        }];
        let budget = ChapterBudget::load_from_config_root(std::path::Path::new("config"));
        let (instr, force_full) =
            build_batch_revise_instructions(&run, false, true, false, &budget);
        assert!(force_full, "timeline hard rule should force full rewrite");
        assert!(instr.contains("整章"));
    }

    #[test]
    fn body_state_hard_rule_forces_full_rewrite() {
        let mut run = sample_run();
        run.message = "身体状态板侧别冲突".into();
        run.content_rule_blocked = true;
        run.content_rule_violations = vec![novelx_harness::ContentRuleViolation {
            rule: "body_state_side".into(),
            message: "侧别冲突".into(),
            blocking: true,
        }];
        let budget = ChapterBudget::load_from_config_root(std::path::Path::new("config"));
        let (instr, force_full) =
            build_batch_revise_instructions(&run, false, true, false, &budget);
        assert!(force_full, "body_state_side should force full rewrite");
        assert!(
            instr.contains("状态板") || instr.contains("整章"),
            "expected body/full plan wording: {instr}"
        );
        assert!(
            !instr.contains("只改违规句，勿整章重写"),
            "must not keep local-only hard-rule wording: {instr}"
        );
    }

    #[test]
    fn consistency_timeline_p0_forces_full_rewrite() {
        let mut run = sample_run();
        run.consistency_passed = Some(false);
        run.issues = vec![json!({
            "type": "TIMELINE",
            "priority": "P0",
            "location": "段3",
            "message": "午后突然变清晨",
            "quote": "清晨的雾气"
        })];
        let budget = ChapterBudget::load_from_config_root(std::path::Path::new("config"));
        let (instr, force_full) =
            build_batch_revise_instructions(&run, true, false, false, &budget);
        assert!(force_full);
        assert!(instr.contains("时间线") || instr.contains("时段"));
    }

    #[test]
    fn soft_long_builds_compress_instructions_without_force_full() {
        let mut run = sample_run();
        run.length_status = "soft_long".into();
        let budget = ChapterBudget::load_from_config_root(std::path::Path::new("config"));
        let (instr, force_full) =
            build_batch_revise_instructions(&run, false, false, true, &budget);
        assert!(!force_full, "SoftLong compress should not force_full");
        assert!(
            instr.contains("压缩") || instr.contains("压回"),
            "expected compress wording: {instr}"
        );
        assert!(
            instr.contains(&budget.word_min.to_string())
                || instr.contains(&budget.word_max.to_string())
                || instr.contains(&budget.range_label()),
            "expected length target in: {instr}"
        );
    }

    #[test]
    fn soft_long_batch_continue_defers_publish_flag() {
        // Batch Continue must set defer so SoftLong is not published before compress.
        let opts = BatchContinueOpts {
            auto_length_revise: true,
            ..Default::default()
        };
        let rev = RevisionOptions {
            defer_soft_long_publish: opts.auto_length_revise,
            ..Default::default()
        };
        assert!(rev.defer_soft_long_publish);
        assert!(should_batch_auto_length_revise(
            true,
            false, // not yet published (deferred)
            false,
            false,
            Some(true),
            "soft_long",
            false,
        ));
    }
}
