use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::oneshot;
use tracing::info;

pub struct ActiveAgentTimer {
    pub cancel_tx: oneshot::Sender<String>,
    pub duration_secs: u64,
    pub started_at: std::time::Instant,
}

pub struct AgentTimerManager {
    timers: Mutex<HashMap<String, ActiveAgentTimer>>,
}

impl Default for AgentTimerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentTimerManager {
    pub fn new() -> Self {
        Self {
            timers: Mutex::new(HashMap::new()),
        }
    }

    /// Registers a new sleep timer for the given agent.
    /// Strictly cancels and aborts any existing timer for this agent first,
    /// guaranteeing that an agent can have strictly only 1 active timer at any time.
    pub fn register_timer(
        &self,
        agent_id: &str,
        cancel_tx: oneshot::Sender<String>,
        duration_secs: u64,
    ) {
        let mut timers = self.timers.lock().unwrap();
        if let Some(existing) = timers.remove(agent_id) {
            info!(
                "Cancelling existing sleep timer for agent {} (duration: {}s) to enforce single-timer invariant",
                agent_id, existing.duration_secs
            );
            let _ = existing
                .cancel_tx
                .send("cancelled: superseded by new timer".to_string());
        }

        timers.insert(
            agent_id.to_string(),
            ActiveAgentTimer {
                cancel_tx,
                duration_secs,
                started_at: std::time::Instant::now(),
            },
        );
    }

    /// Cancels the active timer for an agent if one exists.
    /// Returns true if an active timer was cancelled.
    pub fn cancel_timer(&self, agent_id: &str, reason: &str) -> bool {
        let mut timers = self.timers.lock().unwrap();
        if let Some(timer) = timers.remove(agent_id) {
            info!(
                "Cancelled active timer for agent {} due to: {}",
                agent_id, reason
            );
            let _ = timer.cancel_tx.send(reason.to_string());
            true
        } else {
            false
        }
    }

    /// Clears the timer entry when the timer completes normally without cancellation.
    pub fn clear_timer(&self, agent_id: &str) {
        let mut timers = self.timers.lock().unwrap();
        timers.remove(agent_id);
    }

    /// Checks if an agent currently has an active timer.
    pub fn has_active_timer(&self, agent_id: &str) -> bool {
        let timers = self.timers.lock().unwrap();
        timers.contains_key(agent_id)
    }
}

/// Returns the process-wide global AgentTimerManager singleton.
pub fn global_timer_manager() -> &'static AgentTimerManager {
    static INSTANCE: OnceLock<AgentTimerManager> = OnceLock::new();
    INSTANCE.get_or_init(AgentTimerManager::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_single_timer_superseded() {
        let mgr = AgentTimerManager::new();
        let agent_id = "agent-single-test";

        let (tx1, rx1) = oneshot::channel::<String>();
        mgr.register_timer(agent_id, tx1, 60);
        assert!(mgr.has_active_timer(agent_id));

        // Registering a second timer must cancel the first timer
        let (tx2, mut rx2) = oneshot::channel::<String>();
        mgr.register_timer(agent_id, tx2, 30);

        // First timer should have received the cancellation notice
        let cancel_reason = rx1.await.expect("rx1 received cancellation");
        assert!(cancel_reason.contains("superseded"));

        // Second timer is still active
        assert!(mgr.has_active_timer(agent_id));

        // Cancel second timer explicitly
        let cancelled = mgr.cancel_timer(agent_id, "interrupted by test");
        assert!(cancelled);
        assert!(!mgr.has_active_timer(agent_id));

        let res2 = rx2.try_recv().expect("rx2 received cancellation");
        assert_eq!(res2, "interrupted by test");
    }

    #[tokio::test]
    async fn test_timer_cleared_on_completion() {
        let mgr = AgentTimerManager::new();
        let agent_id = "agent-complete-test";

        let (tx, _rx) = oneshot::channel::<String>();
        mgr.register_timer(agent_id, tx, 10);
        assert!(mgr.has_active_timer(agent_id));

        mgr.clear_timer(agent_id);
        assert!(!mgr.has_active_timer(agent_id));
    }
}
