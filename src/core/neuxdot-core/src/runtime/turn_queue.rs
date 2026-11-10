//! One turn per agent. Everything else waits its turn.
//!
//! An agent used to be able to run several turns at once: the HTTP route, the
//! MCP push bridge and the subscription hub each spawned `run_turn`
//! independently. With `sleep` that became a livelock — turn A registers a
//! 30-minute timer, turn B starts and registers its own, which cancels A, so A
//! wakes, calls `sleep` again, cancels B, and the two ping-pong forever. The
//! event log showed exactly that:
//!
//! ```text
//! SLEEP  30m  ->  WAKE cancelled: superseded by new timer
//! SLEEP  10m  ->  WAKE cancelled: superseded by new timer
//! ```
//!
//! This queue makes the rule explicit: **an agent has at most one running
//! turn.** A message that arrives mid-turn is held and merged with any other
//! arrivals into a single batch, which runs as the next turn once the current
//! one finishes. A sleeping agent is woken so the batch is not stuck behind a
//! 30-minute timer, but the running turn is never duplicated.
//!
//! Ownership is a token, not a boolean, so a runner that unwinds still releases
//! the agent (see [`TurnGuard`]) without a stale runner clobbering its
//! successor.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct AgentState {
    /// Token of the turn currently allowed to run this agent.
    running: Option<u64>,
    /// Messages that arrived while a turn was running.
    pending: Vec<String>,
    /// Set when a new message arrived mid-turn: the running turn should stop at
    /// its next iteration boundary so the batch can be handled promptly.
    preempted: bool,
}

#[derive(Default)]
pub struct AgentTurnQueue {
    state: Mutex<HashMap<String, AgentState>>,
    next_token: AtomicU64,
}

/// Proof that this caller owns the agent. Releasing on drop means a panic in
/// the turn loop cannot leave the agent permanently locked.
pub struct TurnGuard<'a> {
    queue: &'a AgentTurnQueue,
    agent_id: String,
    token: u64,
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        self.queue.release(&self.agent_id, self.token);
    }
}

impl AgentTurnQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer a message to the agent.
    ///
    /// Returns `Some(token)` when the caller became the runner and must drive
    /// the queue, or `None` when the message was queued behind an active turn.
    pub fn submit(&self, agent_id: &str, input: String) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(agent_id.to_string()).or_default();
        entry.pending.push(input);

        if entry.running.is_some() {
            // Ask the active turn to yield at the next boundary. It will drain
            // this message (batched with any others) when it returns.
            entry.preempted = true;
            None
        } else {
            let token = self.next_token.fetch_add(1, Ordering::SeqCst) + 1;
            entry.running = Some(token);
            Some(token)
        }
    }

    /// Take everything queued for the owner. `None` means nothing is pending.
    pub fn take_batch(&self, agent_id: &str, token: u64) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(agent_id.to_string()).or_default();
        if entry.running != Some(token) {
            return None;
        }
        if entry.pending.is_empty() {
            return None;
        }
        entry.preempted = false;
        let batch: Vec<String> = entry.pending.drain(..).collect();
        Some(format_batch(&batch))
    }

    /// True once, if a message arrived while the current turn was running.
    pub fn take_preempt(&self, agent_id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        match state.get_mut(agent_id) {
            Some(entry) if entry.preempted => {
                entry.preempted = false;
                true
            }
            _ => false,
        }
    }

    fn release(&self, agent_id: &str, token: u64) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.get_mut(agent_id) {
            if entry.running == Some(token) {
                entry.running = None;
            }
        }
    }

    pub fn is_running(&self, agent_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .get(agent_id)
            .map(|e| e.running.is_some())
            .unwrap_or(false)
    }

    pub fn pending_len(&self, agent_id: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .get(agent_id)
            .map(|e| e.pending.len())
            .unwrap_or(0)
    }

    /// Convenience for the executor: become the runner and hold the agent for
    /// the lifetime of the returned guard.
    pub fn acquire<'a>(&'a self, agent_id: &str, input: String) -> Option<(u64, TurnGuard<'a>)> {
        let token = self.submit(agent_id, input)?;
        Some((
            token,
            TurnGuard {
                queue: self,
                agent_id: agent_id.to_string(),
                token,
            },
        ))
    }
}

/// Merge one or more arrivals into the prompt the model actually sees.
///
/// A single message passes through untouched so the common case has no wrapper
/// noise. Two or more are numbered and explicitly framed as a batch, so the
/// model knows they arrived together and can answer them as one turn rather than
/// repeating itself per message.
pub fn format_batch(batch: &[String]) -> String {
    match batch {
        [] => String::new(),
        [single] => single.clone(),
        many => {
            let mut out = format!(
                "{} messages arrived while you were working. Address them together in one reply:\n",
                many.len()
            );
            for (i, msg) in many.iter().enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, msg));
            }
            out.trim_end().to_string()
        }
    }
}

pub fn global_turn_queue() -> &'static AgentTurnQueue {
    static INSTANCE: OnceLock<AgentTurnQueue> = OnceLock::new();
    INSTANCE.get_or_init(AgentTurnQueue::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_runner_per_agent() {
        let q = AgentTurnQueue::new();
        let token = q.submit("a", "first".into()).expect("first becomes runner");
        assert!(q.submit("a", "second".into()).is_none(), "second is queued");
        assert!(q.is_running("a"));
        assert_eq!(q.pending_len("a"), 2);
        q.release("a", token);
        assert!(!q.is_running("a"));
    }

    #[test]
    fn different_agents_do_not_block_each_other() {
        let q = AgentTurnQueue::new();
        assert!(q.submit("a", "x".into()).is_some());
        assert!(q.submit("b", "y".into()).is_some());
    }

    #[test]
    fn batch_drains_then_the_guard_releases() {
        let q = AgentTurnQueue::new();
        let token = q.submit("a", "first".into()).unwrap();
        q.submit("a", "second".into());

        let batch = q.take_batch("a", token).expect("batch");
        assert!(batch.starts_with("2 messages arrived"));
        assert!(batch.contains("1. first"));
        assert!(batch.contains("2. second"));
        assert!(q.take_batch("a", token).is_none(), "queue drained");
        assert!(q.is_running("a"), "still owned until the guard drops");
        q.release("a", token);
        assert!(!q.is_running("a"));
    }

    #[test]
    fn a_stale_token_cannot_drain_or_release() {
        let q = AgentTurnQueue::new();
        let t1 = q.submit("a", "first".into()).unwrap();
        q.release("a", t1);
        let t2 = q.submit("a", "second".into()).unwrap();
        assert_ne!(t1, t2);
        // The first runner must not be able to touch the second's queue.
        assert!(q.take_batch("a", t1).is_none());
        q.release("a", t1);
        assert!(q.is_running("a"), "stale release must not free the new owner");
        assert!(q.take_batch("a", t2).is_some());
    }

    #[test]
    fn a_single_message_is_not_wrapped() {
        let q = AgentTurnQueue::new();
        let t = q.submit("a", "hello".into()).unwrap();
        assert_eq!(q.take_batch("a", t).as_deref(), Some("hello"));
    }

    #[test]
    fn preempt_is_consumed_once() {
        let q = AgentTurnQueue::new();
        q.submit("a", "first".into());
        assert!(!q.take_preempt("a"));
        q.submit("a", "second".into());
        assert!(q.take_preempt("a"));
        assert!(!q.take_preempt("a"));
    }

    #[test]
    fn guard_releases_on_drop_even_when_unwinding() {
        let q = AgentTurnQueue::new();
        {
            let (_t, _guard) = q.acquire("a", "x".into()).expect("runner");
            assert!(q.is_running("a"));
        }
        assert!(!q.is_running("a"), "drop must release the agent");
    }
}
