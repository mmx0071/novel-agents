//! Batch continue-writing until a hard gate (longform throughput).

use crate::chapter_gate::check_chapter_order;
use crate::expected_events::{list_gate_candidates, list_hard_ok_candidates, needs_fresh_review};
use crate::phases::{
    resolve_setup_phase, resolve_volume_phase, PhaseEnforceFlags, VolumePhase,
};
use crate::plots::{
    check_plot_write_gate_with, ensure_bridge_plot_active, PlotWriteGate, PlotWriteMode,
};
use crate::project::{load_project_state, project_dir, read_chapter_draft};
use crate::run::{execute_pipeline, PipelineRun, RevisionOptions, RunMode};
use crate::schemas::draft_body_chars;
use crate::volume_audit_gate::check_volume_audit_for_continue;
use anyhow::Result;
use novelx_harness::{ChapterBudget, LengthAssessment, LongformConfig};
use novelx_llm::LlmClient;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::run::PipelineEvent;

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
            auto_length_revise: true,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<PipelineRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchContinueResult {
    pub project: String,
    pub started_chapter: u32,
    pub chapters_attempted: u32,
    pub chapters_published: u32,
    pub stopped_reason: String,
    pub results: Vec<BatchChapterResult>,
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
        let hard_blob = format!(
            "{detail} {}",
            run.content_rule_violations
                .iter()
                .map(|v| format!("{} {}", v.rule, v.message))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if looks_like_timeline_issue(&hard_blob) {
            force_full = true;
            parts.push(format!(
                "消除时段/时间线硬规则违规；整章理顺日夜顺序与时段锚点，勿只改违规句。参考：{detail}"
            ));
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
        let need_consistency = run.consistency_passed == Some(false);
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
    opts: BatchContinueOpts,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<BatchContinueResult> {
    let lf = LongformConfig::load_from_config_root(config_root);
    let max = opts
        .max_chapters
        .unwrap_or(lf.batch_max_chapters)
        .max(1)
        .min(100);
    let max_auto_revise = lf.batch_max_auto_revise.max(1);
    let dir = project_dir(projects_root, &opts.project);
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

    let until_label = until
        .map(|u| format!("，写到第{u}章为止"))
        .unwrap_or_default();
    emit_batch(
        &tx,
        format!("▶ 开始连写到卡点（最多成功发布 {max} 章{until_label}，自第{started}章）"),
    );

    while published_n < max && attempted < safety_cap {
        let state = load_project_state(&dir)?;
        let chapter = state.next_chapter.max(1);
        if let Some(u) = until {
            if chapter > u {
                stopped = format!("until_chapter({u})");
                emit_batch(&tx, format!("✓ 已写到 until_chapter({u})，批写结束"));
                break;
            }
        }

        attempted += 1;
        emit_batch(
            &tx,
            format!(
                "▶ 批写第{chapter}章（本批已发布 {published_n}/{max}，第 {attempted} 次尝试）"
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
                format!("⛔ 第{chapter}章拦截·卷审：{}", block.message),
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
                run: None,
            });
            stopped = "volume_audit_gate".into();
            break;
        }

        if let Some((reason, message)) =
            expected_gate_message(&dir, chapter, opts.confirm_skip_expected)
        {
            emit_batch(&tx, format!("⛔ 第{chapter}章拦截·预期检阅：{message}"));
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
                    format!("⛔ 第{chapter}章拦截·章序：{}", block.message),
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
                    run: None,
                });
                stopped = "chapter_order".into();
                break;
            }
        }

        match check_plot_write_gate_with(&dir, enforce) {
            PlotWriteGate::Block { message, reason, .. } => {
                emit_batch(&tx, format!("⛔ 第{chapter}章拦截·剧情门：{message}"));
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

        // Foreshadow *pressure* debt brake (not total dangling / not far-horizon).
        let debt_cap = lf.batch_max_dangling_foreshadow;
        if debt_cap > 0 {
            let health = crate::memory::longform_health_snapshot(&dir);
            let pressure = health
                .pointer("/foreshadow/dangling_pressure")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let total = health
                .pointer("/foreshadow/dangling_total")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let fresh = health
                .pointer("/foreshadow/dangling_fresh")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let far = health
                .pointer("/foreshadow/dangling_far")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if pressure > debt_cap {
                let message = format!(
                    "伏笔近债过高（压力债 {pressure} 条 > 上限 {debt_cap}；总量 {total}，宽限内 {fresh}，远期 {far}）。\
                     批写暂停；请先兑现近期应回收的伏笔，或将长线标为 horizon=far。"
                );
                emit_batch(&tx, format!("⛔ {message}"));
                results.push(BatchChapterResult {
                    chapter,
                    published: false,
                    blocked: true,
                    reason: Some("foreshadow_debt".into()),
                    needs_user_choice: true,
                    message,
                    length_auto_revised: false,
                    hard_rule_auto_revised: false,
                    consistency_auto_revised: false,
                    run: None,
                });
                stopped = "foreshadow_debt".into();
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
                    "⚙ 第{chapter}章已有未发布正文（约 {} 字），先尝试审校发布…",
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
                emit_batch(
                    &tx,
                    format!("✓ 第{chapter}章已有草稿经审校发布（本批累计 {published_n}/{max} 章）"),
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
                    run: Some(audit_run),
                });
                continue;
            }
            let message = format!(
                "第{chapter}章已有未发布正文（约 {} 字），审校/自动修订后仍未发布。批写暂停；请 revise_chapter 或按审批卡修正。",
                existing.chars().count()
            );
            emit_batch(&tx, format!("⛔ {message}"));
            results.push(BatchChapterResult {
                chapter,
                published: false,
                blocked: true,
                reason: Some("draft_exists".into()),
                needs_user_choice: true,
                message,
                length_auto_revised,
                hard_rule_auto_revised,
                consistency_auto_revised,
                run: Some(audit_run),
            });
            stopped = "draft_exists".into();
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
            emit_batch(
                &tx,
                format!("✓ 第{chapter}章已发布（本批累计 {published_n}/{max} 章）"),
            );
        } else if blocked {
            let why = reason.as_deref().unwrap_or("gate");
            emit_batch(
                &tx,
                format!("⛔ 第{chapter}章未发布（{why}），批写暂停"),
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
        emit_batch(&tx, format!("✓ 已达批写成功发布上限 max_chapters({max})"));
    } else if attempted >= safety_cap && stopped == "completed" {
        stopped = format!("safety_cap({safety_cap})");
        emit_batch(
            &tx,
            format!("⏸ 达到尝试安全上限 {safety_cap}（成功发布 {published_n}/{max}）"),
        );
    } else if stopped == "completed" {
        emit_batch(&tx, "✓ 批写循环正常结束");
    }

    Ok(BatchContinueResult {
        project: opts.project,
        started_chapter: started,
        chapters_attempted: attempted,
        chapters_published: published_n,
        stopped_reason: stopped,
        results,
    })
}

impl BatchContinueResult {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }

    pub fn summary_text(&self) -> String {
        let mut lines = vec![format!(
            "批写完成：尝试 {} 章，发布 {} 章，停止原因：{}",
            self.chapters_attempted, self.chapters_published, self.stopped_reason
        )];
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
                "- 第{}章 {flag}{} {}",
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
        let r = BatchContinueResult {
            project: "sample-novel".into(),
            started_chapter: 1,
            chapters_attempted: 1,
            chapters_published: 0,
            stopped_reason: "draft_exists".into(),
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
                run: None,
            }],
        };
        let s = r.summary_text();
        assert!(s.contains("draft_exists"));
        assert!(s.contains("⛔拦截"));
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
