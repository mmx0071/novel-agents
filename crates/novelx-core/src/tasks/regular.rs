//! RegularTask — Codex-like turn runner with pending-input drain.

use crate::session::{InputQueue, TurnGate, TurnInput};
use crate::NovelxCore;
use anyhow::Result;
use novelx_protocol::{EventMsg, UserInput};
use std::sync::Arc;

pub async fn run_regular_task(
    core: Arc<NovelxCore>,
    thread_id: String,
    turn_id: String,
    initial_items: Vec<UserInput>,
    skills: Vec<String>,
    input_queue: Arc<InputQueue>,
    turn_gate: Arc<TurnGate>,
) -> Result<()> {
    // Caller already try_begin'd; refresh id for steer mismatch checks.
    turn_gate.begin(turn_id.clone()).await;
    core.set_abort(&thread_id, false).await;

    let mut next_items = initial_items;
    let mut next_skills = skills;
    let result = async {
        loop {
            // Drain mailbox into user-visible text prefix.
            let mails = input_queue.drain_mailbox().await;
            if !mails.is_empty() {
                let mail_text = mails
                    .iter()
                    .map(|m| format!("[mail from {}] {}", m.author_thread_id, m.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                next_items.insert(0, UserInput::text(mail_text));
            }

            core.run_turn_session(
                &thread_id,
                &turn_id,
                &next_items,
                &next_skills,
            )
            .await?;

            // Human gate open → do not auto-drain duplicate option clicks into a new
            // run_turn (that was re-triggering volume deep-audit / chapter audit).
            if core.thread_awaiting_human(&thread_id).await {
                input_queue.clear_pending_input().await;
                break;
            }

            if !input_queue.has_pending_input().await {
                break;
            }
            let pending = input_queue.drain_pending_input().await;
            next_items = Vec::new();
            next_skills = Vec::new();
            for p in pending {
                match p {
                    TurnInput::UserInput { content } => next_items.extend(content),
                }
            }
            if next_items.is_empty() {
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    turn_gate.end().await;

    if let Err(err) = result {
        core.emit_to_thread(
            &thread_id,
            EventMsg::TurnAborted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                reason: err.to_string(),
            },
        )
        .await;
        return Err(err);
    }
    Ok(())
}
