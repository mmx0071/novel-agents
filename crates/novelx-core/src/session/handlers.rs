//! Codex-style submission_loop for a single thread.
//!
//! Critical: never await a full RegularTask inside this loop — spawn it so
//! Interrupt / Steer / mailbox can be processed while a turn is running
//! (matches Codex `spawn_task` vs blocking `run_turn`).

use crate::session::{InputQueue, TurnGate, TurnInput};
use crate::tasks::regular::run_regular_task;
use crate::NovelxCore;
use anyhow::Result;
use novelx_protocol::{
    EventMsg, InterAgentCommunication, Op, Submission, UserInput,
};
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

pub async fn submission_loop(
    core: Arc<NovelxCore>,
    thread_id: String,
    mut rx: Receiver<Submission>,
    input_queue: Arc<InputQueue>,
    turn_gate: Arc<TurnGate>,
) {
    while let Some(sub) = rx.recv().await {
        let op = sub.op.clone().into_canonical();
        if let Err(err) = dispatch_one(
            &core,
            &thread_id,
            sub.id.clone(),
            op,
            &input_queue,
            &turn_gate,
        )
        .await
        {
            core.emit_to_thread(
                &thread_id,
                EventMsg::Error {
                    thread_id: Some(thread_id.clone()),
                    message: err.to_string(),
                },
            )
            .await;
        }
        if matches!(sub.op, Op::Shutdown { .. }) {
            break;
        }
    }
}

async fn dispatch_one(
    core: &Arc<NovelxCore>,
    thread_id: &str,
    sub_id: String,
    op: Op,
    input_queue: &Arc<InputQueue>,
    turn_gate: &Arc<TurnGate>,
) -> Result<()> {
    match op {
        Op::InterruptTurn { turn_id, .. } => {
            core.set_abort(thread_id, true).await;
            // Clear only the interrupted turn — never wipe a newer turn that already
            // claimed the gate after abort was set.
            if let Some(id) = turn_id.as_deref() {
                turn_gate.end_if(id).await;
            } else if let Some(active) = turn_gate.active_id().await {
                turn_gate.end_if(&active).await;
            }
            core.publish_session_phase(thread_id).await;
        }
        Op::UserInput { items, skills, .. } => {
            user_input_or_turn(
                core, thread_id, sub_id, items, skills, input_queue, turn_gate,
            )
            .await?;
        }
        Op::SteerTurn { text, turn_id, .. } => {
            if let Some(expected) = turn_id.as_deref() {
                if let Some(active) = turn_gate.active_id().await {
                    if active != expected {
                        anyhow::bail!("steer turn mismatch: expected {expected}, active {active}");
                    }
                }
            }
            if turn_gate.has_active().await {
                input_queue
                    .extend_pending_input(vec![TurnInput::UserInput {
                        content: vec![UserInput::text(text)],
                    }])
                    .await;
            } else {
                user_input_or_turn(
                    core,
                    thread_id,
                    sub_id,
                    vec![UserInput::text(text)],
                    vec![],
                    input_queue,
                    turn_gate,
                )
                .await?;
            }
        }
        Op::RespondUserInput {
            turn_id: gate_turn_id,
            option_id,
            free_text,
            ..
        } => {
            let opt = option_id.clone();
            let free = free_text.clone();
            let text = free_text.unwrap_or(option_id);
            // Correlate with gate_opened via Op.turn_id; response_turn_id is the new turn.
            core.journal_ops(
                None,
                Some(thread_id),
                Some(gate_turn_id.as_str()),
                None,
                Some(opt.as_str()),
                novelx_protocol::OpsJournalKind::GateResponded,
                format!("gate responded: {opt}"),
                serde_json::json!({
                    "option_id": opt,
                    "free_text": free,
                    "gate_turn_id": gate_turn_id,
                    "response_turn_id": sub_id,
                }),
            )
            .await;
            user_input_or_turn(
                core,
                thread_id,
                sub_id,
                vec![UserInput::text(text)],
                vec![],
                input_queue,
                turn_gate,
            )
            .await?;
        }
        Op::InterAgentCommunication { communication } => {
            handle_mailbox(core, thread_id, communication, input_queue, turn_gate).await?;
        }
        Op::Shutdown { .. } => {}
        Op::StartThread { .. } | Op::ResumeThread { .. } | Op::StartTurn { .. } => {
            // Handled at NovelxCore::handle_op / submit boundary.
        }
    }
    Ok(())
}

async fn user_input_or_turn(
    core: &Arc<NovelxCore>,
    thread_id: &str,
    turn_id: String,
    items: Vec<UserInput>,
    skills: Vec<String>,
    input_queue: &Arc<InputQueue>,
    turn_gate: &Arc<TurnGate>,
) -> Result<()> {
    // Atomic: avoid TOCTOU that spawned two RegularTasks on double-click.
    if !turn_gate.try_begin(turn_id.clone()).await {
        input_queue
            .extend_pending_input(vec![TurnInput::UserInput { content: items }])
            .await;
        return Ok(());
    }
    // Gate claimed — surface `working` before TurnStarted lands on the wire.
    core.publish_session_phase(thread_id).await;
    // Codex: spawn_task — do not block submission_loop.
    spawn_regular_task(
        Arc::clone(core),
        thread_id.to_string(),
        turn_id,
        items,
        skills,
        Arc::clone(input_queue),
        Arc::clone(turn_gate),
    );
    Ok(())
}

fn spawn_regular_task(
    core: Arc<NovelxCore>,
    thread_id: String,
    turn_id: String,
    items: Vec<UserInput>,
    skills: Vec<String>,
    input_queue: Arc<InputQueue>,
    turn_gate: Arc<TurnGate>,
) {
    tokio::spawn(async move {
        if let Err(err) = run_regular_task(
            Arc::clone(&core),
            thread_id.clone(),
            turn_id,
            items,
            skills,
            input_queue,
            turn_gate,
        )
        .await
        {
            core.emit_to_thread(
                &thread_id,
                EventMsg::Error {
                    thread_id: Some(thread_id.clone()),
                    message: err.to_string(),
                },
            )
            .await;
        }
    });
}

async fn handle_mailbox(
    core: &Arc<NovelxCore>,
    thread_id: &str,
    communication: InterAgentCommunication,
    input_queue: &Arc<InputQueue>,
    turn_gate: &Arc<TurnGate>,
) -> Result<()> {
    let trigger = communication.trigger_turn;
    let text = communication.content.clone();
    input_queue.enqueue_mailbox(communication).await;
    if trigger {
        let turn_id = novelx_protocol::new_id("turn");
        if !turn_gate.try_begin(turn_id.clone()).await {
            return Ok(());
        }
        core.publish_session_phase(thread_id).await;
        spawn_regular_task(
            Arc::clone(core),
            thread_id.to_string(),
            turn_id,
            vec![UserInput::text(text)],
            vec![],
            Arc::clone(input_queue),
            Arc::clone(turn_gate),
        );
    }
    Ok(())
}
