pub mod handlers;
pub mod input_queue;

pub use input_queue::{InputQueue, TurnInput};

use novelx_protocol::TurnId;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ActiveTurn {
    pub turn_id: TurnId,
}

#[derive(Default)]
pub struct TurnGate {
    pub active: Mutex<Option<ActiveTurn>>,
}

impl TurnGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn active_id(&self) -> Option<TurnId> {
        self.active.lock().await.as_ref().map(|t| t.turn_id.clone())
    }

    /// Mark active only if empty or already this turn — never steal another turn's slot.
    pub async fn begin(&self, turn_id: TurnId) {
        let mut guard = self.active.lock().await;
        match guard.as_ref() {
            Some(active) if active.turn_id != turn_id => {}
            _ => *guard = Some(ActiveTurn { turn_id }),
        }
    }

    /// Atomically start a turn. Returns false if another turn is already active
    /// (caller should queue the input instead of spawning a concurrent task).
    pub async fn try_begin(&self, turn_id: TurnId) -> bool {
        let mut guard = self.active.lock().await;
        if guard.is_some() {
            return false;
        }
        *guard = Some(ActiveTurn { turn_id });
        true
    }

    pub async fn end(&self) {
        *self.active.lock().await = None;
    }

    /// Clear the gate only when it still belongs to `turn_id` (safe after interrupt + new turn).
    pub async fn end_if(&self, turn_id: &str) {
        let mut guard = self.active.lock().await;
        if guard.as_ref().map(|t| t.turn_id.as_str()) == Some(turn_id) {
            *guard = None;
        }
    }

    pub async fn has_active(&self) -> bool {
        self.active.lock().await.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn try_begin_rejects_concurrent() {
        let g = TurnGate::new();
        assert!(g.try_begin("t1".into()).await);
        assert!(!g.try_begin("t2".into()).await);
        assert_eq!(g.active_id().await.as_deref(), Some("t1"));
        g.end().await;
        assert!(g.try_begin("t3".into()).await);
    }

    #[tokio::test]
    async fn end_if_does_not_clear_other_turn() {
        let g = TurnGate::new();
        assert!(g.try_begin("old".into()).await);
        g.end_if("old").await;
        assert!(g.try_begin("new".into()).await);
        g.end_if("old").await;
        assert_eq!(g.active_id().await.as_deref(), Some("new"));
        g.begin("old".into()).await; // must not steal
        assert_eq!(g.active_id().await.as_deref(), Some("new"));
        g.end_if("new").await;
        assert!(!g.has_active().await);
    }
}
