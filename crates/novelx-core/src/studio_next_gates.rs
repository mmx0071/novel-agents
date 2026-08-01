//! Situational next-step gates via Studio `offer_decisions(kind=studio_next)`.

use crate::studio_next::{
    contextual_fallback_options, studio_next_nudge, validate_studio_next_options,
    AwaitingStudioNext, PendingStudioNext, StudioNextContext, StudioNextOption,
};
use crate::ui_sync::{attach_ui_approval, strip_ui_approvals};
use crate::{PendingSettingBlocker, PlotWriteGateAction, NovelxCore};
use anyhow::Result;
use novelx_protocol::{new_id, EventMsg, ItemStatus, TurnItem, UserInputOption};
use serde_json::{json, Value};

impl NovelxCore {
    /// Drop situational pendings that no longer render open_gate cards.
    pub(crate) fn clear_deprecated_situational_pendings(t: &mut crate::ThreadState) {
        if t.pending_chapter_next
            .as_ref()
            .is_some_and(|c| c.suggest_next.is_some())
        {
            t.pending_chapter_next = None;
        }
    }

    /// Soft-request a model-offered card. Returns true = situational path handled
    /// (stop other contextual gates). Does **not** open a human gate yet — RegularTask
    /// drains the nudge. Higher-priority hard gates still win (returns false).
    pub(crate) async fn request_studio_next(
        &self,
        thread_id: &str,
        project: &str,
        reason: &str,
        context: StudioNextContext,
        outline_rewrite: bool,
    ) -> Result<bool> {
        if project.is_empty() {
            return Ok(false);
        }
        {
            let guard = self.threads.read().await;
            if let Some(t) = guard.get(thread_id) {
                if t.pending_mutation.is_some()
                    || t.pending_impact.is_some()
                    || t.pending_audit.is_some()
                    || t.pending_setup.is_some()
                    || t.pending_volume_sync.is_some()
                    || t.pending_volume_handoff.is_some()
                    || t.pending_chapter_order.is_some()
                    || t.pending_chapter_next.is_some()
                    || t.pending_studio_next.is_some()
                {
                    return Ok(false);
                }
                if t.awaiting_studio_next.is_some() {
                    return Ok(true);
                }
            }
        }
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.awaiting_studio_next = Some(AwaitingStudioNext {
                project: project.to_string(),
                reason: reason.to_string(),
                context,
            });
            if outline_rewrite {
                t.outline_rewrite_active = true;
            }
            // Clear deprecated situational pendings so RegularTask can drain.
            Self::clear_deprecated_situational_pendings(t);
        }
        self.enqueue_pending_user_text(thread_id, &studio_next_nudge(project, reason))
            .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    pub(crate) async fn apply_studio_next_offer(
        &self,
        thread_id: &str,
        turn_id: &str,
        args: &Value,
        data: &Value,
    ) -> Result<crate::OfferApplyResult> {
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("project").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if project.is_empty() {
            return Ok(crate::OfferApplyResult::Rejected("project 必填".into()));
        }
        let raw_options = data
            .get("options")
            .or_else(|| args.get("options"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let options = match validate_studio_next_options(&project, &raw_options) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(error = %e, "studio_next offer_decisions rejected");
                return Ok(crate::OfferApplyResult::Rejected(e));
            }
        };
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .or_else(|| data.get("prompt").and_then(|v| v.as_str()))
            .unwrap_or("请选择下一步：")
            .trim()
            .to_string();
        let prompt = if prompt.is_empty() {
            "请选择下一步：".into()
        } else {
            prompt
        };
        let context = {
            let guard = self.threads.read().await;
            guard
                .get(thread_id)
                .and_then(|t| t.awaiting_studio_next.as_ref())
                .map(|a| a.context.clone())
                .unwrap_or(StudioNextContext::Generic)
        };
        if self
            .open_studio_next_gate(thread_id, turn_id, project, prompt, options, context)
            .await?
        {
            Ok(crate::OfferApplyResult::OpenedGate)
        } else {
            Ok(crate::OfferApplyResult::Ignored)
        }
    }

    pub(crate) async fn open_studio_next_gate(
        &self,
        thread_id: &str,
        turn_id: &str,
        project: String,
        prompt: String,
        options: Vec<StudioNextOption>,
        context: StudioNextContext,
    ) -> Result<bool> {
        if options.is_empty() {
            return Ok(false);
        }
        let ui_opts: Vec<UserInputOption> =
            options.iter().map(StudioNextOption::to_ui_option).collect();
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.awaiting_studio_next = None;
            t.pending_studio_next = Some(PendingStudioNext {
                project,
                prompt: prompt.clone(),
                options,
                context,
            });
            t.ui_turns = attach_ui_approval(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &prompt,
                &ui_opts,
            );
        }
        self.clear_queued_inputs(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::RequestUserInput {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                prompt,
                options: ui_opts,
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        Ok(true)
    }

    /// When Studio finishes a nudge turn without offering a card — soft two-key fallback.
    pub(crate) async fn maybe_offer_studio_next_fallback(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<bool> {
        let awaiting = {
            let guard = self.threads.read().await;
            let t = guard.get(thread_id);
            match t {
                Some(t)
                    if t.awaiting_studio_next.is_some()
                        && t.pending_studio_next.is_none()
                        && t.pending_mutation.is_none()
                        && t.pending_impact.is_none()
                        && t.pending_audit.is_none()
                        && t.pending_setup.is_none()
                        && t.pending_volume_sync.is_none()
                        && t.pending_volume_handoff.is_none()
                        && t.pending_chapter_order.is_none()
                        && t.pending_chapter_next.is_none() =>
                {
                    t.awaiting_studio_next.clone()
                }
                _ => None,
            }
        };
        let Some(awaiting) = awaiting else {
            return Ok(false);
        };
        let (prompt_prefix, mut options) =
            contextual_fallback_options(&awaiting.project, &awaiting.context);
        // Generic/mutation: prefer gates.yaml labels when present.
        if matches!(
            awaiting.context,
            StudioNextContext::Mutation { .. } | StudioNextContext::Generic
        ) {
            let fb = self.gates.studio_next_fallback_options();
            if !fb.is_empty() {
                options = fb
                    .into_iter()
                    .map(|o| {
                        let resolve = match o.id.as_str() {
                            "snf_continue" => Some("continue_studio".into()),
                            _ => Some("dismiss_gate".into()),
                        };
                        StudioNextOption {
                            id: o.id,
                            label: o.label,
                            tool: None,
                            args: None,
                            resolve,
                        }
                    })
                    .collect();
            }
        }
        if options.is_empty() {
            if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                t.awaiting_studio_next = None;
            }
            return Ok(false);
        }
        let prompt = if awaiting.reason.is_empty() {
            prompt_prefix
        } else {
            format!("{} {}", awaiting.reason, prompt_prefix)
        };
        self.open_studio_next_gate(
            thread_id,
            turn_id,
            awaiting.project,
            prompt,
            options,
            awaiting.context,
        )
        .await
    }

    pub(crate) async fn parse_studio_next_op(
        &self,
        thread_id: &str,
        text: &str,
    ) -> Option<StudioNextPick> {
        let pending = {
            let guard = self.threads.read().await;
            guard.get(thread_id)?.pending_studio_next.clone()
        };
        let pending = pending?;
        let t = text.trim();
        let opt = pending
            .options
            .iter()
            .find(|o| o.id == t || o.label == t)
            .or_else(|| {
                let idx: usize = t.parse().ok()?;
                pending.options.get(idx.checked_sub(1)?)
            })
            .cloned()?;
        Some(StudioNextPick { pending, opt })
    }

    pub(crate) async fn run_studio_next_pick(
        &self,
        thread_id: &str,
        turn_id: &str,
        pick: StudioNextPick,
    ) -> Result<()> {
        let StudioNextPick { pending, opt } = pick;
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.pending_studio_next = None;
            t.awaiting_studio_next = None;
            t.ui_turns = strip_ui_approvals(std::mem::take(&mut t.ui_turns));
        }
        let _ = self.persist_thread(thread_id).await;

        if let Some(tool) = opt.tool.as_deref() {
            let args = opt.args.clone().unwrap_or_else(|| json!({}));
            if matches!(tool, "design_master_outline" | "design_arc_outline") {
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.outline_rewrite_active = true;
                }
            }
            // Reuse mutation-followup tool path shape: emit + run_one_tool + post gates.
            self.run_studio_next_tool(thread_id, turn_id, tool, &args, &pending.context)
                .await?;
            return Ok(());
        }

        let resolve = opt.resolve.as_deref().unwrap_or("dismiss_gate");
        match resolve {
            "continue_studio" => {
                if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                    t.outline_rewrite_active = true;
                }
                let project = pending.project.clone();
                let (nudge, summary) = if let StudioNextContext::PlotWrite {
                    kind,
                    title,
                    ..
                } = &pending.context
                {
                    if kind == "missing_exit" {
                        let title = if title.is_empty() {
                            "当前进行中剧情卡".to_string()
                        } else {
                            title.clone()
                        };
                        (
                            format!(
                                "项目「{project}」写章被拦：剧情卡「{title}」缺少可检验的「收束条件」。\
                                 请立即补全该卡 frontmatter exit_condition 与正文「## 收束条件」\
                                 （可核验的叙事落点，勿写成整卷终止）；可用 design_plot(force=true) \
                                 重写该卡并保留其余要点。禁止 continue_writing。完成后一句话确认。"
                            ),
                            "好的，正在补全收束条件…",
                        )
                    } else {
                        (
                            format!(
                                "请继续推进项目「{project}」未完成步骤。立即调用工具；\
                                 除非用户明确要写章，禁止 continue_writing。完成后一句话确认。"
                            ),
                            "好的，继续推进…",
                        )
                    }
                } else {
                    (
                        format!(
                            "请继续推进项目「{project}」未完成步骤。立即调用工具；\
                             除非用户明确要写章，禁止 continue_writing。完成后一句话确认。"
                        ),
                        "好的，继续推进…",
                    )
                };
                self.emit_studio_next_summary(thread_id, turn_id, summary, false)
                    .await?;
                self.enqueue_pending_user_text(thread_id, &nudge).await;
            }
            "activate_plot_write" => {
                if let StudioNextContext::PlotWrite { title, .. } = &pending.context {
                    self.run_plot_write_gate_action(
                        thread_id,
                        turn_id,
                        PlotWriteGateAction::ActivateAndWrite {
                            project: pending.project.clone(),
                            title: title.clone(),
                        },
                    )
                    .await?;
                    return Ok(());
                }
                self.emit_studio_next_summary(
                    thread_id,
                    turn_id,
                    "无法激活剧情卡：缺少上下文。",
                    false,
                )
                .await?;
            }
            "skip_volume" => {
                if let StudioNextContext::VolumeAudit { .. } = &pending.context {
                    self.emit_studio_next_summary(thread_id, turn_id, "已结束卷复盘。", false)
                        .await?;
                } else {
                    self.emit_studio_next_summary(thread_id, turn_id, "已跳过。", false)
                        .await?;
                }
            }
            _ => {
                // dismiss_gate — resume setting-blocker followups when applicable
                if let StudioNextContext::SettingBlocker {
                    chapter,
                    detail: _,
                    resume_chapter_next,
                    published,
                    plot_accept_open,
                    content_blocked,
                    revise_instructions,
                    offer_volume_handoff_after,
                } = &pending.context
                {
                    let sb = PendingSettingBlocker {
                        project: pending.project.clone(),
                        chapter: *chapter,
                        detail: String::new(),
                        resume_chapter_next: *resume_chapter_next,
                        published: *published,
                        plot_accept_open: *plot_accept_open,
                        content_blocked: *content_blocked,
                        revise_instructions: revise_instructions.clone(),
                        offer_volume_handoff_after: *offer_volume_handoff_after,
                    };
                    self.emit_studio_next_summary(
                        thread_id,
                        turn_id,
                        "已接受并继续。",
                        false,
                    )
                    .await?;
                    let _ = self
                        .finish_setting_blocker_followups(thread_id, turn_id, &sb)
                        .await?;
                } else {
                    if let Some(t) = self.threads.write().await.get_mut(thread_id) {
                        t.outline_rewrite_active = false;
                    }
                    self.emit_studio_next_summary(thread_id, turn_id, "好的，稍后再说。", false)
                        .await?;
                }
            }
        }
        let _ = self.persist_thread(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::TurnComplete {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            },
        )
        .await;
        Ok(())
    }

    async fn emit_studio_next_summary(
        &self,
        thread_id: &str,
        turn_id: &str,
        summary: &str,
        awaiting_gate: bool,
    ) -> Result<()> {
        use crate::ui_sync::append_completion_ui_turn;
        use novelx_llm::ChatMessage;
        let agent_item_id = new_id("item");
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemStarted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id.clone(),
                    text: String::new(),
                    status: ItemStatus::InProgress,
                },
            },
        )
        .await;
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: summary.to_string(),
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            });
            t.ui_turns = append_completion_ui_turn(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                summary,
                awaiting_gate,
            );
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id,
                    text: summary.to_string(),
                    status: ItemStatus::Completed,
                },
            },
        )
        .await;
        Ok(())
    }

    async fn run_studio_next_tool(
        &self,
        thread_id: &str,
        turn_id: &str,
        tool_name: &str,
        args: &Value,
        context: &StudioNextContext,
    ) -> Result<()> {
        use crate::ui_sync::append_completion_ui_turn;
        use novelx_llm::ChatMessage;
        let intro = match tool_name {
            "design_master_outline" => "已收到选择，正在重建总纲…",
            "design_arc_outline" => "已收到选择，正在生成卷纲…",
            "upsert_setting" | "supplement_setting" => "已收到选择，正在补齐世界观…",
            "list_entities" => "已收到选择，正在列出设定卡…",
            "audit_setting" => "已收到选择，正在跑设定审计…",
            "design_plot" => "已收到选择，正在设计剧情卡…",
            "continue_writing" => "已收到选择，正在写章…",
            "continue_writing_batch" => "已收到选择，正在连写（先处理未发布草稿）…",
            "audit_chapter" | "audit_chapters" => "已收到选择，正在审校…",
            "revise_chapter" => "已收到选择，正在修订…",
            "review_expected_events" => "已收到选择，正在检阅预期…",
            _ => "已收到选择，正在推进…",
        };
        let agent_item_id = new_id("item");
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemStarted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id.clone(),
                    text: String::new(),
                    status: ItemStatus::InProgress,
                },
            },
        )
        .await;
        self.emit_to_thread(
            thread_id,
            EventMsg::AgentMessageContentDelta {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item_id: agent_item_id.clone(),
                delta: intro.into(),
            },
        )
        .await;
        // StudioNext card already is the user's confirmation — skip a second mutation card.
        let args = novelx_tools::with_confirm_skip(args.clone());
        let (output, data) = self
            .run_one_tool(thread_id, turn_id, tool_name, &args.to_string())
            .await?;
        let needs_confirm = data.get("needs_confirm").and_then(|v| v.as_bool()) == Some(true);
        let mut summary = format!("{intro}\n\n{output}");
        let gate_open = if needs_confirm {
            self.maybe_offer_mutation_confirm(thread_id, turn_id, tool_name, &args, &data)
                .await?
        } else {
            let asked_impact = self
                .maybe_offer_impact_cascade(thread_id, turn_id, tool_name, &args, &data)
                .await?;
            let asked_setup = if asked_impact {
                false
            } else {
                self.maybe_offer_setup_confirm(thread_id, turn_id, tool_name, &args, &data)
                    .await?
            };
            let asked_followup = if asked_impact || asked_setup {
                false
            } else {
                self.maybe_offer_mutation_followup(thread_id, turn_id, tool_name, &args, &data)
                    .await?
            };
            if asked_impact {
                summary.push_str("\n\n已扫描依赖面，请选择是否自动同步修正。");
            } else if asked_followup {
                summary.push_str("\n\n正在请 Studio 给出下一步审批卡…");
            }
            asked_impact || asked_setup || asked_followup
        };
        // After setting-blocker repair tools, resume chapter_next when context says so.
        let mut resumed = false;
        if !gate_open {
            if let StudioNextContext::SettingBlocker {
                chapter,
                resume_chapter_next,
                published,
                plot_accept_open,
                content_blocked,
                revise_instructions,
                offer_volume_handoff_after,
                ..
            } = context
            {
                let sb = PendingSettingBlocker {
                    project: args
                        .get("project")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    chapter: *chapter,
                    detail: String::new(),
                    resume_chapter_next: *resume_chapter_next,
                    published: *published,
                    plot_accept_open: *plot_accept_open,
                    content_blocked: *content_blocked,
                    revise_instructions: revise_instructions.clone(),
                    offer_volume_handoff_after: *offer_volume_handoff_after,
                };
                resumed = self
                    .finish_setting_blocker_followups(thread_id, turn_id, &sb)
                    .await?;
            }
        }
        if let Some(t) = self.threads.write().await.get_mut(thread_id) {
            t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: summary.clone(),
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            });
            t.ui_turns = append_completion_ui_turn(
                std::mem::take(&mut t.ui_turns),
                turn_id,
                &summary,
                gate_open || resumed,
            );
        }
        self.emit_to_thread(
            thread_id,
            EventMsg::ItemCompleted {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
                item: TurnItem::AgentMessage {
                    id: agent_item_id,
                    text: summary,
                    status: ItemStatus::Completed,
                },
            },
        )
        .await;
        let _ = self.persist_thread(thread_id).await;
        self.emit_to_thread(
            thread_id,
            EventMsg::TurnComplete {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            },
        )
        .await;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StudioNextPick {
    pub pending: PendingStudioNext,
    pub opt: StudioNextOption,
}
