//! Secret and PII scrubbing for anything a memory host persists or hands on.
//!
//! Conservative by design — it prefers false positives over leaking
//! credentials into long-lived stores. One copy of this policy is shared by the
//! memory engines and the OpenHuman host; it used to exist three times. The
//! design, what is blocked versus redacted, and the known limits are in this
//! module's `README.md`.
//!
//! [`scrub_item`] applies the policy to every text a
//! [`tinymemory_api::StoreItem`] carries, and is what a host runs on each item
//! before `MemoryEngine::store`.
//!
//! The exhaustive multilingual national-ID PII module ([`pii`]) runs as part
//! of [`sanitize_text`]. The write-rejection boundary ([`has_likely_pii`])
//! stays stricter than content scrubbing: formatted national IDs are rejected,
//! while phone/email-like text is scrubbed from content without rejecting
//! every write that mentions them.
//!
//! Before the shape regexes, [`sanitize_text`] redacts the value after a
//! credential *marker* — a one-time-secret URL's `/secret/<key>` and a `Bearer`
//! value too short for the regexes — keeping the marker and the prose around
//! it. [`redact_credential_markers`] runs just those rules, for a host that
//! scrubs plain text without the PII pass.
//!
//! # The one policy knob
//!
//! The previous copies differed in exactly one behaviour: how a *bare*
//! (separator-less) Luhn-valid 13-19 digit run is treated as a credit card.
//! The OpenHuman host redacted every such run; TinyCortex additionally demanded
//! corroboration (a real network IIN at an issued length, or a card keyword
//! nearby) so 13-digit epoch-millisecond timestamps in stored JSON envelopes
//! stopped being corrupted (opencompany#1201). [`BareCardGate`] names both and
//! the plain functions default to the stricter [`BareCardGate::LuhnOnly`], so no
//! caller that does not opt in redacts less than before. Callers that want the
//! corroborated behaviour use the `*_with` variants and [`Policy::corroborated`].

/// Scrubbing a whole [`tinymemory_api::StoreItem`] before it is stored.
mod item;
/// One-time-secret URLs and `Bearer` values, including short ones.
mod markers;
/// Compiling the built-in regular expressions.
mod pattern;
/// Exhaustive checksum-gated multilingual national-ID PII module. Content
/// scrubbing runs from [`sanitize_text`]; the boundary check is re-exported as
/// [`has_likely_pii`].
pub mod pii;
/// The policy knob and the report types every pass returns.
mod policy;
/// Text and JSON scrubbing: private keys, credential shapes, sensitive keys.
mod sanitize;

pub use item::{scrub_item, scrub_item_with};
pub use markers::redact_credential_markers;
pub use pii::{has_likely_email, has_likely_pii};
pub use policy::{BareCardGate, Policy, SanitizationReport, Sanitized};
pub use sanitize::{
    has_likely_secret, sanitize_json, sanitize_json_with, sanitize_text, sanitize_text_with,
};
