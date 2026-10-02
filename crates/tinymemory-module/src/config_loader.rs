//! The engine's config loader, answered from what the module was handed.
//!
//! # Why this one is *not* a bus proxy
//!
//! Every other seam in this crate goes to the host, and the reason is always
//! the same: the host holds live state the module cannot be handed once — a
//! credential, an inference route, the user's Composio connections. This seam
//! is the one where that reasoning runs the other way.
//!
//! The module is handed [`crate::config::ModuleConfig`] at load. It is the
//! host's own configuration, already resolved by the host's loader with the
//! host's env overrides and migrations applied, already narrowed to what the
//! engine reads, and already the thing every engine call in this process runs
//! against — `provider::provider` and the queue worker pool both take their
//! [`EngineRuntimeConfig`] from it. Asking the host to re-read a config the
//! module was handed would introduce a *second* answer to a question that
//! already has one, and the interesting case is not when the two agree.
//!
//! They can disagree in both directions. `ConfigLoader::load` is documented to
//! "follow the ambient environment", so a host with more than one workspace can
//! answer for a different one than the module is bound to — and the module's
//! store, queue and summary tree are all rooted at
//! `ModuleConfig::workspace_dir`. A loader that answered from somewhere else
//! would hand a sync loop in this process a config pointing at another user's
//! workspace, which is precisely the cross-workspace leak the engine's own
//! `get_source_in` exists to avoid.
//!
//! # What this costs, stated rather than hidden
//!
//! `tinymemory_core::config_loader`'s whole purpose is *freshness*: background
//! loops re-load so a mid-session settings change takes effect on the next tick
//! rather than the next restart, and `ProviderContext::execute` re-reads on
//! every call so a `composio.mode` toggle is honoured immediately. Answering
//! from the load-time snapshot gives up exactly that. A user who changes a
//! setting after this module loaded gets the old value from anything in this
//! process until the host reloads the module.
//!
//! That is a real limit and it is logged once per process the first time
//! anything consults this loader — `warn_degraded_once`, which keeps the log
//! line the scheduler-gate stub emits but leaves the error reporter alone. It
//! is a documented design limit rather than something gone wrong, and the host
//! is the one that handed this module the frozen snapshot, so reporting it as a
//! defect only pages someone about a decision already made. Closing it
//! properly means a host-pushed config signal (this module declares
//! `signals = []`), not a bus *pull*: a pull would re-introduce the two-answers
//! problem above while still being stale between ticks.
//!
//! # The gap this loader used to have, and how it was closed
//!
//! `EngineRuntimeConfig::memory_sync_interval_secs` answered the constant
//! `Some(0)`, and the contract reads `Some(0)` as **manual only**. A periodic
//! sync loop started inside this process therefore considered every source
//! manual and skipped it — silently, which is the failure class this migration
//! keeps producing.
//!
//! The fix was not for this loader to invent a better number. Guessing at a user
//! setting the module was never told is the same thing `crate::host` refuses to
//! do when it declines to synthesise a scheduler-gate policy from
//! `ModuleConfig::scheduler_gate`, and it has the same answer: guessing is worse
//! than not answering. So the *host* now sends the cadence, as
//! `ModuleConfig::memory_sync_interval_secs`, and this loader hands it back
//! along with everything else. Nothing here needed changing, which is the point
//! — the snapshot answers whatever the host put in it.
//!
//! What is left is the staleness above, and it now bites one more setting: a
//! user who changes their sync cadence, or switches Composio between backend and
//! direct mode, after this module loaded keeps the old value in this process
//! until the host reloads the module.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use tinymemory_core::config_loader::ConfigLoader;
// The trait, not only the alias: `config_path` is reached as a METHOD on both
// sides of the comparison below, and `EngineRuntimeConfig` also has a field of
// that name. Without the trait in scope the method call resolves to nothing and
// rustc points at the field, which would compare a path against a path-shaped
// field on a different type.
use tinymemory_api::host::MemoryHostConfig;
use tinymemory_core::Config;
use tinymemory_tinycortex::engine::EngineRuntimeConfig;

use crate::config::ModuleConfig;
use crate::host::warn_degraded_once;

/// Latched so the degradation is named once per process rather than once per
/// call — `ProviderContext::execute` reloads on *every* Composio action, and an
/// unlatched report would page per tool call.
static LOADER_REPORTED: AtomicBool = AtomicBool::new(false);

/// What answering locally costs, in the terms a reader of the log needs.
const CONFIG_LOADER_FROZEN: &str = "config loader answered from the module's load-time snapshot: \
                                    this module re-reads no config file, so a settings change \
                                    made after it loaded (Composio mode, sync cadence, a memory \
                                    source switched off) does not reach the engine in this \
                                    process until the host reloads the module";

/// Refusal message for a snapshot this module was not loaded for.
///
/// Names no path. A workspace path identifies a user, and a module error
/// crosses the bus into logs that are not this module's to decide about — the
/// same rule [`ModuleConfig::validate`] follows.
const FOREIGN_SNAPSHOT: &str = "config loader was asked to re-read a snapshot from a different \
                                workspace than the one this module was loaded for; this module \
                                serves exactly one workspace and will not answer for another";

/// The engine's [`ConfigLoader`], served from [`ModuleConfig`].
#[derive(Debug)]
pub struct ModuleConfigLoader {
    /// Behind an `Arc` because `reload_snapshot` hands back a shared handle and
    /// `load` hands back an owned one; keeping one canonical value means the
    /// two can never answer differently.
    snapshot: Arc<EngineRuntimeConfig>,
}

impl ModuleConfigLoader {
    /// Build the loader from the config this module was handed.
    ///
    /// # The credential is dropped here too
    ///
    /// `EngineRuntimeConfig::from` clones `ModuleConfig::memory` wholesale, and
    /// that struct carries `agentmemory_secret` — a bearer token for a remote
    /// memory backend. `setup` already strips it before this is built, so this
    /// clears nothing in practice today. It is here because this type's whole
    /// job is to *hand the config back out*, repeatedly, to any engine code
    /// that asks: a future caller that built a loader before the strip, or from
    /// a config that never went through `setup`, would turn one missed ordering
    /// into a token handed to every consumer. Defence in depth costs one line
    /// and removes a whole class of ordering bug.
    #[must_use]
    pub fn new(config: &ModuleConfig) -> Self {
        let mut snapshot = EngineRuntimeConfig::from(config);
        snapshot.memory.agentmemory_secret = None;
        Self {
            snapshot: Arc::new(snapshot),
        }
    }
}

#[async_trait]
impl ConfigLoader for ModuleConfigLoader {
    /// The config this module was loaded with.
    ///
    /// A `Box`, not an `Arc`, because the contract's callers include config
    /// *migrations* that need `&mut`. Those writes land on the copy and go
    /// nowhere: `EngineRuntimeConfig::save` is a no-op, since this module has
    /// no config file to write and inventing one would put a second writer on
    /// the host's. The composio source-caps migration is the one caller that
    /// notices — it re-runs each time rather than recording that it ran.
    ///
    /// # Errors
    ///
    /// Never. The answer is a clone of a value this module already holds; there
    /// is no read to fail. The `Result` is the contract's, shaped for a host
    /// that reads a file.
    async fn load(&self) -> Result<Box<Config>, String> {
        warn_degraded_once(&LOADER_REPORTED, CONFIG_LOADER_FROZEN);
        let owned: Box<Config> = Box::new((*self.snapshot).clone());
        Ok(owned)
    }

    /// Re-read the config `snapshot` came from — which, here, is this one.
    ///
    /// The contract distinguishes this from `load` because it
    /// follows the ambient environment and can land on a different workspace
    /// than the caller is working in. In this module both answer from the same
    /// value, so the distinction collapses — except for the check below, which
    /// is the one thing the distinction still buys.
    ///
    /// # Errors
    ///
    /// `FOREIGN_SNAPSHOT` when `snapshot` was loaded from a different
    /// workspace. Answering with this module's config would be worse than
    /// failing: the caller asked to re-read *its* config and would silently get
    /// another workspace's, which is how a sync run writes one user's data into
    /// another user's store. The paths are compared rather than the values
    /// because `config_path` is what the contract itself calls the anchor.
    async fn reload_snapshot(&self, snapshot: &Config) -> Result<Arc<Config>, String> {
        warn_degraded_once(&LOADER_REPORTED, CONFIG_LOADER_FROZEN);
        if snapshot.config_path() != self.snapshot.config_path() {
            return Err(FOREIGN_SNAPSHOT.to_string());
        }
        // Annotated rather than `Arc::clone`d: the field is an
        // `Arc<EngineRuntimeConfig>` and the contract wants an
        // `Arc<dyn MemoryHostConfig>`, so the binding's type is what drives the
        // unsizing coercion. `Arc::clone` would infer the concrete type and fail.
        let shared: Arc<Config> = self.snapshot.clone();
        Ok(shared)
    }
}

#[cfg(test)]
#[path = "config_loader_tests.rs"]
mod test;
