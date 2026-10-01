//! [`SchedulerGate`] — the host's background-work throttle, as the core sees it.
//!
//! The memory subsystem runs three kinds of unattended work: the ingest-queue
//! workers, the periodic Composio sync, and the workspace watcher. All three
//! must back off when the host says background AI work is not welcome right now
//! — the user turned it off, the machine is on battery, or nobody is signed in.
//!
//! That decision is host policy and it is global: the same gate throttles cron,
//! the subconscious and the agent harness. The core only asks.
//!
//! # Why the trait is here and not in `tinymemory-api`
//!
//! [`SchedulerGate::resume_notify`] hands back a `tokio::sync::Notify`, and the
//! contract crate must not depend on an async runtime. This crate already does.
//!
//! # The permit is opaque on purpose
//!
//! [`SchedulerGate::wait_for_capacity`] returns a `Box<dyn Send>` rather than
//! the host's concrete `LlmPermit`. Callers bind it and hold it for the
//! duration of the LLM-bound work; releasing it is `Drop`, which works exactly
//! the same through the box. Naming the concrete type would drag the host's
//! semaphore into the contract for no gain.
//!
//! # Unwired means "no throttling", not "blocked"
//!
//! With no gate installed — unit tests, the standalone engine build — the
//! policy reads [`Policy::Normal`] and `wait_for_capacity` returns immediately.
//! Failing closed here would deadlock every worker in every test that has not
//! wired a host up, and the gate is an optimisation, not a correctness barrier.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::Notify;

pub use tinymemory_api::host::{PauseReason, Policy};

/// The host's view of whether background AI work should run right now.
#[async_trait]
pub trait SchedulerGate: Send + Sync + std::fmt::Debug {
    /// The current scheduling tier.
    fn current_policy(&self) -> Policy;

    /// A handle that is notified whenever the gate leaves a paused state, so a
    /// sleeping loop can wake immediately instead of waiting out its tick.
    fn resume_notify(&self) -> Arc<Notify>;

    /// Wait until an LLM-bound slot is free, returning a permit to hold for the
    /// duration of the call. `None` when the caller should proceed ungated.
    async fn wait_for_capacity(&self) -> Option<Box<dyn Send>>;
}

static GATE: RwLock<Option<Arc<dyn SchedulerGate>>> = RwLock::new(None);

/// Install the host's scheduler gate. Called once during startup wiring.
pub fn set_scheduler_gate(gate: Arc<dyn SchedulerGate>) {
    *GATE.write() = Some(gate);
}

/// Remove any installed gate, returning to ungated behaviour. For tests.
pub fn clear_scheduler_gate() {
    *GATE.write() = None;
}

/// The installed gate, or `None` when nothing has been wired up.
#[must_use]
pub fn scheduler_gate() -> Option<Arc<dyn SchedulerGate>> {
    GATE.read().clone()
}

/// The current scheduling tier, or [`Policy::Normal`] when ungated.
///
/// A live manual override (see [`set_manual_override`]) takes precedence over
/// the gate: user-initiated maintenance is the one thing a pause must not
/// stop, because the pause exists to protect the user from *background* cost
/// they did not ask for — work they explicitly requested is the opposite
/// case.
#[must_use]
pub fn current_policy() -> Policy {
    if manual_override_active() {
        return Policy::Normal;
    }
    scheduler_gate().map_or(Policy::Normal, |gate| gate.current_policy())
}

static MANUAL_OVERRIDE_UNTIL: RwLock<Option<std::time::Instant>> = RwLock::new(None);

fn manual_override_active() -> bool {
    MANUAL_OVERRIDE_UNTIL
        .read()
        .is_some_and(|until| std::time::Instant::now() < until)
}

/// Clear any live manual override. For tests: the window is a process
/// global, and a test that opens one must not leak it into its neighbours.
pub fn clear_manual_override() {
    *MANUAL_OVERRIDE_UNTIL.write() = None;
}

/// Open a manual-override window: for `seconds`, [`current_policy`] answers
/// [`Policy::Normal`] regardless of the installed gate, and paused sleepers
/// are woken so the window is not spent waiting out a tick.
///
/// For user-initiated maintenance under `mode = off` (openhuman#5935): the
/// off switch stops background work, and this is how a user's explicit
/// "process now" still runs. The window is bounded — there is no "override
/// forever", because that would just be the gate turned off with extra steps.
pub fn set_manual_override(seconds: u64) {
    // The clamp is this function's contract, not its callers': the window is
    // bounded to an hour (an unbounded override is the gate turned off with
    // extra steps), and the bound also makes the expiry arithmetic
    // infallible — `Instant + 1h` cannot overflow, where an unclamped u64
    // could panic inside library code.
    let seconds = seconds.min(3600);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    *MANUAL_OVERRIDE_UNTIL.write() = Some(until);
    resume_notify().notify_waiters();
}

/// The resume handle. When ungated this is a `Notify` nobody ever fires, so a
/// `select!` on it simply never takes that arm.
#[must_use]
pub fn resume_notify() -> Arc<Notify> {
    match scheduler_gate() {
        Some(gate) => gate.resume_notify(),
        None => {
            static IDLE: std::sync::OnceLock<Arc<Notify>> = std::sync::OnceLock::new();
            Arc::clone(IDLE.get_or_init(|| Arc::new(Notify::new())))
        }
    }
}

/// Wait for an LLM-bound slot. Returns immediately when ungated.
pub async fn wait_for_capacity() -> Option<Box<dyn Send>> {
    match scheduler_gate() {
        Some(gate) => gate.wait_for_capacity().await,
        None => None,
    }
}

#[cfg(test)]
#[path = "scheduler_gate_override_tests.rs"]
mod override_tests;
