//! A policy decorator over any TinyMemory [`MemoryProvider`].
//!
//! [`GuardedProvider`] implements [`MemoryProvider`] over an
//! `Arc<dyn MemoryProvider>`. That makes it *transparent* — a caller writes
//! the same code against the guard as against the driver — and it makes the
//! guard *unskippable by construction* for anyone holding it, because there is
//! no second, unguarded shape to reach for.
//!
//! The load-bearing detail is the `as_*` accessors. Every optional capability
//! family (23 of the contract's 26; only `MemoryCore`, `MemoryRecall` and
//! `MemoryPortability` are mandatory and implemented on the guard directly in
//! `mandatory.rs`) is reachable **only** through them, so an override that
//! forwarded `self.inner.as_tree()` would hand out a raw driver handle and
//! defeat the entire design with one method call. Each family therefore gets
//! its own decorator, owned as a field on the guard (an accessor returns a
//! borrow, so it cannot build one on demand) and present exactly when the inner
//! driver provides that family. See [`families`].
//!
//! ## The seven enforcement steps
//!
//! | # | Step | Where |
//! | - | ---- | ----- |
//! | 1 | tier | [`GuardPolicy::enforce_read`] / [`GuardPolicy::enforce_write`] |
//! | 1b | path rules | **no-op** — no contract method carries a path |
//! | 2 | source scope as a query predicate | [`GuardPolicy::ambient_scope`], applied in `GuardedTree::query_source` |
//! | 3 | taint stamping | [`GuardPolicy::stamp_taint`] |
//! | 4 | redaction | [`GuardPolicy::redact_outbound`] |
//! | 5 | egress + trust | [`GuardPolicy::check_egress`] |
//! | 6 | char budgets | [`budget`], driven by [`GuardPolicy::recall_budget`] / [`GuardPolicy::capture_budget`] |
//! | 7 | audit + tracing | [`audit`], [`GuardPolicy::on_denied`] / [`GuardPolicy::on_allowed`] |
//!
//! Three of those depart from the obvious reading, and each departure is argued
//! at its own call site:
//!
//! - **Step 2 is not applied to `recall`.** A driver may *refuse* a scoped
//!   recall (`SCOPE_UNAPPLIED`), so filling the parameter from the ambient scope
//!   would turn every recall inside a source scope into a hard error. The scope
//!   is filled on `MemoryTree::query_source` (and the other query paths that push
//!   it into the driver's predicate before `LIMIT`).
//! - **Step 3 raises, it never overrides.** A plain override would rewrite a
//!   caller's `ExternalSync` down to `Internal` outside a scope, which is the
//!   laundering step the contract says the guard exists to prevent.
//! - **Step 1's path half is a no-op**, because nothing in the contract carries
//!   a filesystem path to validate.
//!
//! ## What belongs to the host
//!
//! Everything [`GuardPolicy`] asks for: what a tier is, where the ambient scope
//! comes from, how a secret is scrubbed, the egress and trust rule, the budget
//! numbers, and who hears about a refusal. This crate depends on the contract
//! crate and nothing else of substance.
//!
//! [`MemoryProvider`]: tinymemory_api::provider::MemoryProvider

#![forbid(unsafe_code)]

pub mod audit;
pub mod budget;
pub mod families;
mod mandatory;
pub mod policy;
pub mod provider;

pub use policy::{GuardPolicy, GUARD_DENIED_PREFIX};
pub use provider::GuardedProvider;
