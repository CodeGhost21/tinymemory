//! Step 7: the guard's tracing span and its trace lines.
//!
//! ## What may be logged, and what may never be
//!
//! Memory content is the most sensitive data in the product. Nothing in this
//! module — span field or log line — carries a memory body, a recall query, a
//! namespace *key*, or a document title. What it carries is **shapes**: the
//! driver id, the contract method, the namespace, char counts, and hit counts.
//! The refusal side (log + event) belongs to the host through
//! [`GuardPolicy::on_denied`]; this crate publishes nothing itself.
//!
//! Success is traced through [`GuardPolicy::on_allowed`] but is not something a
//! host should put on a bus: one event per memory read would flood every
//! subscriber on the hot path for no operator benefit.

use tinymemory_api::capabilities::Capability;

use crate::policy::GuardPolicy;

/// Grep prefix for every guard log line.
pub const LOG_PREFIX: &str = "[memory:guard]";

/// The tracing span every guarded call's admission decision runs inside.
///
/// Carries the three correlation fields the spec asks for — `driver_id`, the
/// capability family, and the namespace — plus the contract method, so two
/// calls into the same family are distinguishable in a trace.
pub fn guard_span(
    driver_id: &str,
    capability: Capability,
    method: &str,
    namespace: &str,
) -> tracing::Span {
    tracing::debug_span!(
        "memory_guard",
        driver_id = driver_id,
        capability = capability.as_str(),
        method = method,
        namespace = namespace,
    )
}

/// Namespace placeholder for the contract methods that address no namespace
/// (maintenance, portability, goals). Better than an empty field, which reads
/// as "the namespace was lost".
pub const NO_NAMESPACE: &str = "-";

/// Trace a call the guard let through. Shapes only.
pub fn trace_allowed<P: GuardPolicy + ?Sized>(
    policy: &P,
    method: &str,
    namespace: &str,
    chars: usize,
) {
    policy.on_allowed(method, namespace, chars);
}

/// Log the effect of a budget, when it actually bit. Silent when it did not, so
/// the log is a record of truncation rather than a per-call heartbeat.
pub fn trace_budget<P: GuardPolicy + ?Sized>(
    policy: &P,
    method: &str,
    dropped: usize,
    trimmed_chars: usize,
) {
    if dropped == 0 && trimmed_chars == 0 {
        return;
    }
    log::debug!(
        "{LOG_PREFIX} budget applied driver={} method={method} dropped={dropped} \
         trimmed_chars={trimmed_chars}",
        policy.driver_id(),
    );
}
