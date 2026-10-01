//! `tinymemory-gate` — gate background AI work on host conditions.
//!
//! Background AI tasks (memory-tree digests, embeddings, summarisation) used to
//! run flat-out and made the host visibly lag, especially on battery. The
//! decision (what [`Policy`] a set of [`Signals`] and a [`SchedulerGateConfig`]
//! yield) is a pure function in `tinymemory-api`. This crate is the rest:
//!
//! * [`signals`] samples the machine: power state, CPU usage, deployment mode,
//!   with environment overrides whose names the host supplies ([`SignalEnv`]).
//! * [`gate`] keeps the cached policy ([`GateCore`]), refreshes it from a
//!   background sampler ([`spawn_sampler`]), and lets callers cooperatively
//!   wait for capacity ([`wait_for_capacity`]) under a single-slot LLM
//!   semaphore.
//!
//! What stays with the host is the process-wide wiring: the singleton, the
//! signed-out override, the resume notification and whatever test isolation it
//! wants around them.

pub mod gate;
pub mod signals;

pub use gate::{
    acquire_llm_permit, new_llm_slots, spawn_sampler, try_acquire_llm_permit, wait_for_capacity,
    GateCore, LlmPermit, SharedCore, LLM_SLOTS, SAMPLE_INTERVAL,
};
pub use signals::{sample, SignalEnv};
pub use tinymemory_api::host::{
    decide, PauseReason, Policy, SchedulerGateConfig, SchedulerGateMode, Signals,
};
