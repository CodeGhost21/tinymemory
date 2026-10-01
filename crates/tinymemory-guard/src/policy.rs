//! [`GuardPolicy`] — the seam between the generic decorator and the host.
//!
//! The decorator ([`GuardedProvider`](crate::GuardedProvider) and the family
//! decorators under [`families`](crate::families)) owns the *shape* of
//! enforcement: which contract method runs which of the seven steps, in what
//! order, and how a refusal is spelled. It owns none of the *facts* the steps
//! consult. Those are the host's, and arrive through this trait:
//!
//! | Step | Required method(s) |
//! | ---- | ------------------ |
//! | 1 tier | [`enforce_read`](GuardPolicy::enforce_read), [`enforce_write`](GuardPolicy::enforce_write) |
//! | 2 scope | [`ambient_scope`](GuardPolicy::ambient_scope) (with [`narrow_scope`](GuardPolicy::narrow_scope) built on it) |
//! | 3 taint | built on `ambient_scope` ([`stamp_taint`](GuardPolicy::stamp_taint)) |
//! | 4 redaction | [`redact_outbound`](GuardPolicy::redact_outbound), [`redact_outbound_json`](GuardPolicy::redact_outbound_json) |
//! | 5 egress | [`check_egress`](GuardPolicy::check_egress) |
//! | 6 budgets | [`recall_budget`](GuardPolicy::recall_budget), [`capture_budget`](GuardPolicy::capture_budget) |
//! | 7 audit | [`on_denied`](GuardPolicy::on_denied), [`on_allowed`](GuardPolicy::on_allowed) |
//!
//! ## Nothing here may be cached by the decorator
//!
//! A host's policy can change under a live guard (an autonomy tier that is
//! hot-swapped, a source scope that is a task-local). The decorator therefore
//! calls the trait on **every** operation and holds no policy value of its own;
//! an implementation that answers from live state stays correct across the
//! swap, which a value captured at construction would not.

use std::borrow::Cow;

use tinymemory_api::capabilities::Capability;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::types::SourceScope;
use tinymemory_api::types::MemoryTaint;

/// Prefix on every guard-authored error message, so a refusal that surfaces to
/// a caller is attributable to the guard rather than to the driver underneath.
pub const GUARD_DENIED_PREFIX: &str = "memory guard: ";

/// The facts and side effects a [`GuardedProvider`](crate::GuardedProvider)
/// consults. See the [module docs](self).
pub trait GuardPolicy: Send + Sync + 'static {
    /// The bound driver's stable id — appears in every span and audit event.
    fn driver_id(&self) -> &str;

    // ── Step 1: tier ─────────────────────────────────────────────────────────

    /// Tier check for a **read** operation.
    ///
    /// # Errors
    ///
    /// Whatever the host's tier refuses, built with [`Self::denied`].
    fn enforce_read(&self, operation: &str) -> Result<(), MemoryError>;

    /// Tier check for a **write** operation.
    ///
    /// # Errors
    ///
    /// Whatever the host's tier refuses, built with [`Self::denied`].
    fn enforce_write(&self, operation: &str) -> Result<(), MemoryError>;

    // ── Step 5: egress + trust ───────────────────────────────────────────────

    /// Per-call egress gate for an external driver.
    ///
    /// `carries_content` says whether the call hands raw memory bodies across
    /// the boundary (rather than metadata only).
    ///
    /// # Errors
    ///
    /// The host's refusal, built with [`Self::denied`].
    fn check_egress(&self, method: &str, carries_content: bool) -> Result<(), MemoryError>;

    // ── Step 2: source scope ─────────────────────────────────────────────────

    /// The ambient per-turn source allowlist, in contract form.
    ///
    /// `None` is unrestricted. `Some` — including `Some` over an empty set —
    /// restricts: [`SourceScope`]'s own docs make an empty allow list deny all
    /// source-attributed content.
    fn ambient_scope(&self) -> Option<SourceScope>;

    // ── Step 4: redaction ────────────────────────────────────────────────────

    /// Content on its way to the driver, redacted when the driver is external.
    ///
    /// The borrowed arm is what makes a no-op a byte-identical pass-through
    /// rather than a re-allocation that merely happens to compare equal.
    fn redact_outbound<'a>(&self, content: &'a str) -> Cow<'a, str>;

    /// [`Self::redact_outbound`] for structured payloads (KV values, document
    /// metadata).
    fn redact_outbound_json(&self, value: serde_json::Value) -> serde_json::Value;

    // ── Step 6: char budgets ─────────────────────────────────────────────────

    /// The recall char budget, or `None` when it is disabled.
    fn recall_budget(&self) -> Option<usize>;

    /// The capture char budget, or `None` when it is disabled.
    fn capture_budget(&self) -> Option<usize>;

    // ── Step 7: audit sink ───────────────────────────────────────────────────

    /// A refusal happened: log it and publish the audit event.
    ///
    /// Called from [`Self::denied`], so every deny path audits by construction
    /// rather than by each call site remembering to. Must never carry a memory
    /// body, a recall query or a key — `reason` is already free of them.
    fn on_denied(&self, method: &str, reason: &str);

    /// A call the guard let through. Shapes only: the method, the namespace and
    /// a char count, never content.
    fn on_allowed(&self, method: &str, namespace: &str, chars: usize);

    // ── Provided ─────────────────────────────────────────────────────────────

    /// Build the guard's canonical refusal error, running [`Self::on_denied`]
    /// as a side effect. Every deny path goes through here so a refusal can
    /// never be raised without the operator seeing it.
    fn denied(&self, method: &str, reason: impl Into<String>) -> MemoryError
    where
        Self: Sized,
    {
        let reason = reason.into();
        self.on_denied(method, &reason);
        MemoryError::Invalid(format!("{GUARD_DENIED_PREFIX}{reason}"))
    }

    /// Enter the guard's tracing span, run the tier (step 1) and egress
    /// (step 5) checks inside it, and leave — all before the caller awaits
    /// anything.
    ///
    /// The span is entered and exited **within this synchronous call** on
    /// purpose. [`tracing::span::EnteredSpan`] is `!Send`, and every method on
    /// the driver contract is an `#[async_trait]` method whose future must be
    /// `Send`; holding an entered span across the `.await` of the forwarded
    /// driver call makes the whole future `!Send` and fails to compile.
    ///
    /// # Errors
    ///
    /// The first refusal, tier or egress.
    fn admit_read(
        &self,
        capability: Capability,
        method: &str,
        namespace: &str,
        carries_content: bool,
    ) -> Result<(), MemoryError> {
        let span = crate::audit::guard_span(self.driver_id(), capability, method, namespace);
        let _enter = span.enter();
        self.enforce_read(method)?;
        self.check_egress(method, carries_content)
    }

    /// [`Self::admit_read`] for a write, taking the write-tier check.
    ///
    /// # Errors
    ///
    /// The first refusal, tier or egress.
    fn admit_write(
        &self,
        capability: Capability,
        method: &str,
        namespace: &str,
        carries_content: bool,
    ) -> Result<(), MemoryError> {
        let span = crate::audit::guard_span(self.driver_id(), capability, method, namespace);
        let _enter = span.enter();
        self.enforce_write(method)?;
        self.check_egress(method, carries_content)
    }

    /// The scope a query actually runs under, given what the caller asked for.
    ///
    /// The ambient allowlist is an **upper bound**, never a default that an
    /// argument replaces. An earlier version returned `requested.or(ambient)`,
    /// which let a source-restricted turn widen itself back out: passing an
    /// explicit scope naming a collection the ambient allowlist did not contain
    /// made that explicit scope the sole query predicate, and the restriction
    /// the turn was running under vanished.
    ///
    /// So the two are intersected. Membership is decided by the ambient scope's
    /// own [`SourceScope::allows_source_id`] rule, so the guard and the
    /// driver's SQL agree on what "in scope" means rather than the guard
    /// inventing a second rule.
    ///
    /// An empty intersection is returned as an empty `Some`, not `None`: an
    /// empty allow list denies all source-attributed content, which is the
    /// fail-closed reading [`SourceScope`] documents. Returning `None` there
    /// would turn "you asked for nothing you are allowed to see" into
    /// "unrestricted", the exact leak this method exists to close.
    fn narrow_scope(&self, requested: Option<&SourceScope>) -> Option<SourceScope> {
        match (requested, self.ambient_scope()) {
            (None, ambient) => ambient,
            (Some(requested), None) => Some(requested.clone()),
            (Some(requested), Some(ambient)) => {
                let allow: Vec<String> = requested
                    .allow
                    .iter()
                    .filter(|id| ambient.allows_source_id(id))
                    .cloned()
                    .collect();
                if allow.len() != requested.allow.len() {
                    log::debug!(
                        "[memory:guard] explicit source scope narrowed by the ambient \
                         allowlist requested={} admitted={}",
                        requested.allow.len(),
                        allow.len()
                    );
                }
                Some(SourceScope { allow })
            }
        }
    }

    /// The provenance the guard stamps on a write.
    ///
    /// The contract is explicit that the driver never assigns provenance, and
    /// that the single failure mode the guard exists to prevent is *laundering*
    /// externally-sourced content into internal-trust content.
    ///
    /// So this is a monotone raise, not a plain override: the result is
    /// [`MemoryTaint::ExternalSync`] when the caller asked for it **or** when
    /// the turn is running under a source scope (a source-restricted turn is
    /// by definition handling source-attributed content), and
    /// [`MemoryTaint::Internal`] only when neither holds. A pure override would
    /// happily rewrite a caller's `ExternalSync` down to `Internal` outside a
    /// scope, which is precisely the laundering step.
    fn stamp_taint(&self, requested: MemoryTaint) -> MemoryTaint {
        if requested == MemoryTaint::ExternalSync || self.ambient_scope().is_some() {
            MemoryTaint::ExternalSync
        } else {
            MemoryTaint::Internal
        }
    }
}
