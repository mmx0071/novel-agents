//! Pending steer input + inter-agent mailbox (Codex InputQueue subset).

use novelx_protocol::{InterAgentCommunication, UserInput};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub enum TurnInput {
    UserInput { content: Vec<UserInput> },
}

#[derive(Default)]
pub struct InputQueue {
    pending: Mutex<Vec<TurnInput>>,
    mailbox: Mutex<Vec<InterAgentCommunication>>,
}

impl InputQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn extend_pending_input(&self, items: Vec<TurnInput>) {
        self.pending.lock().await.extend(items);
    }

    pub async fn has_pending_input(&self) -> bool {
        !self.pending.lock().await.is_empty()
    }

    pub async fn drain_pending_input(&self) -> Vec<TurnInput> {
        std::mem::take(&mut *self.pending.lock().await)
    }

    /// Drop queued user clicks (e.g. duplicate gate options) when pausing for human input.
    pub async fn clear_pending_input(&self) {
        self.pending.lock().await.clear();
    }

    pub async fn enqueue_mailbox(&self, mail: InterAgentCommunication) {
        self.mailbox.lock().await.push(mail);
    }

    pub async fn drain_mailbox(&self) -> Vec<InterAgentCommunication> {
        std::mem::take(&mut *self.mailbox.lock().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novelx_protocol::{InterAgentCommunication, InterAgentKind, UserInput};
    use std::sync::Arc;

    #[tokio::test]
    async fn pending_steer_drains() {
        let q = InputQueue::new();
        q.extend_pending_input(vec![TurnInput::UserInput {
            content: vec![UserInput::text("steer me")],
        }])
        .await;
        assert!(q.has_pending_input().await);
        let drained = q.drain_pending_input().await;
        assert_eq!(drained.len(), 1);
        assert!(!q.has_pending_input().await);
    }

    #[tokio::test]
    async fn mailbox_enqueue_drain() {
        let q = InputQueue::new();
        q.enqueue_mailbox(InterAgentCommunication {
            author_thread_id: "a".into(),
            recipient_thread_id: "b".into(),
            content: "hi".into(),
            trigger_turn: true,
            kind: InterAgentKind::Message,
        })
        .await;
        let mails = q.drain_mailbox().await;
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].content, "hi");
        assert!(q.drain_mailbox().await.is_empty());
    }

    #[tokio::test]
    async fn active_steer_queues_without_consuming_loop() {
        // Mimic mid-turn: pending can be extended while a consumer later drains.
        let q = Arc::new(InputQueue::new());
        let q2 = q.clone();
        let producer = tokio::spawn(async move {
            q2.extend_pending_input(vec![TurnInput::UserInput {
                content: vec![UserInput::text("mid-turn")],
            }])
            .await;
        });
        producer.await.unwrap();
        assert!(q.has_pending_input().await);
        let drained = q.drain_pending_input().await;
        match &drained[0] {
            TurnInput::UserInput { content } => {
                assert_eq!(UserInput::primary_text(content), "mid-turn");
            }
        }
    }
}
