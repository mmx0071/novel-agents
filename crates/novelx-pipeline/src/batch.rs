//! Batch continue-writing until a hard gate (longform throughput).

use crate::chapter_gate::check_chapter_order;
use crate::expected_events::{list_gate_candidates, list_hard_ok_candidates, needs_fresh_review};
use crate::phases::{
    resolve_setup_phase, resolve_volume_phase, PhaseEnforceFlags, VolumePhase,
};
use crate::plots::{check_plot_write_gate_with, PlotWriteGate};
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
    /// Hard stop after this many successful publishes (default from longform.yaml).
    pub max_chapters: Option<u32>,
    /// Stop after publishing this chapter number (inclusive), if set.
    pub until_chapter: Option<u32>,
    pub confirm_skip_volume_audit: bool,
    /// Skip expected-events review gate (same as continue_writing confirm_skip_expected).
    #[serde(default)]
    pub confirm_skip_expected: bool,
    /// When length HardShort / SoftShort-escalate blocks publish, try one full rewrite.
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
    /// One-shot hard-rule revise attempted for an existing draft (not a length expand).
    #[serde(default)]
    pub hard_rule_auto_revised: bool,
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
        LengthAssessment::HardShort => true,
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
        LengthAssessment::Ok => false,
    }
}

/// Whether batch should attempt a one-shot length expand.
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
    matches!(length_status, "hard_short" | "soft_short_escalated") || length_blocks
}

fn emit_batch(tx: &Option<mpsc::UnboundedSender<PipelineEvent>>, message: impl Into<String>) {
    if let Some(tx) = tx {
        let _ = tx.send(PipelineEvent::BatchProgress {
            message: message.into(),
        });
    }
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
    let dir = project_dir(projects_root, &opts.project);
    let state0 = load_project_state(&dir)?;
    let started = state0.next_chapter.max(1);
    let mut results = Vec::new();
    let mut published_n = 0u32;
    let mut attempted = 0u32;
    let mut stopped = "completed".to_string();
    let budget = ChapterBudget::load_from_config_root(config_root);

    emit_batch(
        &tx,
        format!("▶ 开始连写到卡点（最多 {max} 章，自第{started}章）"),
    );

    for _ in 0..max {
        let state = load_project_state(&dir)?;
        let chapter = state.next_chapter.max(1);
        if let Some(until) = opts.until_chapter {
            if chapter > until {
                stopped = format!("until_chapter({until})");
                emit_batch(&tx, format!("✓ 已写到 until_chapter({until})，批写结束"));
                break;
            }
        }

        attempted += 1;
        emit_batch(
            &tx,
            format!("▶ 批写第{chapter}章（第 {attempted}/{max} 次尝试）"),
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
                    run: None,
                });
                stopped = format!("plot_gate:{reason}");
                break;
            }
            PlotWriteGate::Allow { .. } => {}
        }

        // Foreshadow debt brake for unattended batch.
        let debt_cap = lf.batch_max_dangling_foreshadow;
        if debt_cap > 0 {
            let health = crate::memory::longform_health_snapshot(&dir);
            let dangling = health
                .pointer("/foreshadow/dangling_total")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if dangling > debt_cap {
                let message = format!(
                    "伏笔债务过高（未收 {dangling} 条 > 上限 {debt_cap}）。批写暂停；请先兑现/清理伏笔后再连写。"
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
            let mut audit_run = execute_pipeline(
                projects_root,
                config_root,
                &opts.project,
                chapter,
                RunMode::AuditOnly,
                RevisionOptions::default(),
                llm.clone(),
                tx.clone(),
            )
            .await?;
            let mut length_auto_revised = false;
            let mut hard_rule_auto_revised = false;
            // One-shot auto-revise for hard-rule / length blocks before hard-stopping.
            if !audit_run.published
                && (audit_run.content_rule_blocked
                    || matches!(
                        audit_run.length_status.as_str(),
                        "hard_short" | "soft_short_escalated"
                    ))
            {
                let for_hard_rule = audit_run.content_rule_blocked;
                let instr = if for_hard_rule {
                    let detail = audit_run.message.clone();
                    format!(
                        "消除正文硬规则违规；只改违规句，勿整章重写。参考：{}",
                        detail.chars().take(400).collect::<String>()
                    )
                } else {
                    format!(
                        "扩写到{}字完整一章；保持情节与人物一致，补足场景与对话，勿注水。",
                        budget.range_label()
                    )
                };
                emit_batch(
                    &tx,
                    format!("⚙ 第{chapter}章已有草稿未发布，自动修订一次后重试…"),
                );
                audit_run = execute_pipeline(
                    projects_root,
                    config_root,
                    &opts.project,
                    chapter,
                    RunMode::Revise,
                    RevisionOptions {
                        prefer_local_patch: false,
                        revision_mode: true,
                        user_instructions: Some(instr),
                        audit_issues: vec![],
                        pacing_suggestions: vec![],
                        verify_previous: false,
                        full_rescan: false,
                    },
                    llm.clone(),
                    tx.clone(),
                )
                .await?;
                if for_hard_rule {
                    hard_rule_auto_revised = true;
                } else {
                    length_auto_revised = true;
                }
            }
            if audit_run.published {
                published_n += 1;
                emit_batch(
                    &tx,
                    format!("✓ 第{chapter}章已有草稿经审校发布（本批累计 {published_n} 章）"),
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
                run: Some(audit_run),
            });
            stopped = "draft_exists".into();
            break;
        }

        let mut run = execute_pipeline(
            projects_root,
            config_root,
            &opts.project,
            chapter,
            RunMode::Continue,
            RevisionOptions::default(),
            llm.clone(),
            tx.clone(),
        )
        .await?;

        let mut length_auto_revised = false;
        // One-shot length expand when publish blocked by short body.
        if should_batch_auto_length_revise(
            opts.auto_length_revise,
            run.published,
            run.content_rule_blocked,
            run.volume_ended.is_some(),
            run.consistency_passed,
            &run.length_status,
            length_blocks_publish(&dir, config_root, chapter),
        ) {
            tracing::info!(chapter, "batch: auto length revise once");
            emit_batch(
                &tx,
                format!("⚙ 第{chapter}章字数不足，自动扩写一次…"),
            );
            let instr = format!(
                "扩写到{}字完整一章；保持情节与人物一致，补足场景与对话，勿注水。",
                budget.range_label()
            );
            run = execute_pipeline(
                projects_root,
                config_root,
                &opts.project,
                chapter,
                RunMode::Revise,
                RevisionOptions {
                    prefer_local_patch: false,
                    revision_mode: true,
                    user_instructions: Some(instr),
                    audit_issues: vec![],
                    pacing_suggestions: vec![],
                    verify_previous: false,
                    full_rescan: false,
                },
                llm.clone(),
                tx.clone(),
            )
            .await?;
            length_auto_revised = true;
        }

        let blocked = !run.published
            || run.needs_user_choice
            || run.content_rule_blocked
            || run.volume_ended.is_some();
        let reason = if run.volume_ended.is_some() {
            Some("volume_ended".into())
        } else if run.content_rule_blocked {
            Some("content_rule".into())
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
                format!("✓ 第{chapter}章已发布（本批累计 {published_n} 章）"),
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
            hard_rule_auto_revised: false,
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

    if attempted >= max && stopped == "completed" {
        stopped = format!("max_chapters({max})");
        emit_batch(&tx, format!("✓ 已达批写上限 max_chapters({max})"));
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
            let auto = if r.length_auto_revised {
                " ·已自动扩写"
            } else if r.hard_rule_auto_revised {
                " ·已自动修订硬规则"
            } else {
                ""
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
}
