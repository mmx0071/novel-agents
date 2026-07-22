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

    /// Unconditional mark (tests / already-serialized paths). Prefer `try_begin`.
    pub async fn begin(&self, turn_id: TurnId) {
        *self.active.lock().await = Some(ActiveTurn { turn_id });
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
}
