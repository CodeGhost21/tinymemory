//! Host-owned runtime services used by the compiled memory engine.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tinybus::Connection;
use tinymemory_api::host::{ErrorReporter, MemoryEvent, MemoryEventSink, SpacyResponse};

/// Host callback routing constants.
pub const RUNTIME_HOST_BUS_NAME: &str = "ai.tinyhumans.tinymemory.RuntimeHost";
/// Object path for the host callbacks.
pub const RUNTIME_HOST_OBJECT_PATH: &str = "/ai/tinyhumans/tinymemory/RuntimeHost";
/// Interface exported by the host.
pub const RUNTIME_HOST_INTERFACE: &str = "ai.tinyhumans.tinymemory.RuntimeHost";

#[derive(Clone)]
pub(crate) struct BusRuntimeHost {
    connection: Connection,
}

impl std::fmt::Debug for BusRuntimeHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BusRuntimeHost")
            .finish_non_exhaustive()
    }
}

impl BusRuntimeHost {
    pub(crate) fn new(connection: Connection) -> Self {
        Self { connection }
    }

    fn proxy(&self) -> Result<tinybus::Proxy, tinybus::Error> {
        self.connection.proxy(
            RUNTIME_HOST_BUS_NAME,
            RUNTIME_HOST_OBJECT_PATH,
            RUNTIME_HOST_INTERFACE,
        )
    }

    fn notify<T>(&self, method: &'static str, arguments: T)
    where
        T: serde::Serialize + Send + 'static,
    {
        let host = self.clone();
        tokio::spawn(async move {
            let result = match host.proxy() {
                Ok(proxy) => proxy.call::<()>(method, arguments).await,
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                log::debug!("[tinymemory:module] host callback {method} failed: {error}");
            }
        });
    }
}

impl MemoryEventSink for BusRuntimeHost {
    fn publish(&self, event: MemoryEvent) {
        self.notify("PublishEvent", (event,));
    }
}

impl ErrorReporter for BusRuntimeHost {
    fn report_error(&self, rendered: &str, domain: &str, operation: &str, tags: &[(&str, &str)]) {
        self.notify(
            "ReportError",
            (
                false,
                rendered.to_string(),
                domain.to_string(),
                operation.to_string(),
                owned_tags(tags),
            ),
        );
    }

    fn report_error_or_expected(
        &self,
        rendered: &str,
        domain: &str,
        operation: &str,
        tags: &[(&str, &str)],
    ) {
        self.notify(
            "ReportError",
            (
                true,
                rendered.to_string(),
                domain.to_string(),
                operation.to_string(),
                owned_tags(tags),
            ),
        );
    }
}

fn owned_tags(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[async_trait]
impl tinymemory_core::nlp_host::NlpHost for BusRuntimeHost {
    async fn extract_spacy(
        &self,
        _config: &tinymemory_core::Config,
        text: &str,
    ) -> Result<SpacyResponse, String> {
        let proxy = self.proxy().map_err(|error| error.to_string())?;
        proxy
            .call("ExtractSpacy", (text.to_string(),))
            .await
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn install(connection: Connection) {
    let host = Arc::new(BusRuntimeHost::new(connection));
    tinymemory_core::events::set_event_sink(Arc::clone(&host) as Arc<dyn MemoryEventSink>);
    tinymemory_core::observability::set_error_reporter(Arc::clone(&host) as Arc<dyn ErrorReporter>);
    tinymemory_core::nlp_host::set_nlp_host(host);
}

// ── Seams no bus interface serves ───────────────────────────────────────────
//
// This module serves seven of the host's nine seams. Six cross the bus: the
// embedder and the chat model have host interfaces of their own, `composio_host`
// has one too (see `crate::composio`), and the event sink, the error reporter
// and spaCy share `BusRuntimeHost` above. The seventh, `config_loader`, is
// answered locally from the `ModuleConfig` this module was handed, for the
// reason its own module docs give: proxying it would ask the host to re-read a
// config the module already has, and the interesting case is the one where the
// two answers disagree.
//
// The two below are what is left, and both were quiet on every path. With
// nothing installed, `scheduler_gate::current_policy()` reads `Normal` and
// `wait_for_capacity()` returns instantly — background LLM work runs flat out no
// matter what the user asked for — and `shutdown::register` drops the ingest
// queue's lock-release hook behind a `log::debug!`. That is precisely the
// outcome the host's own `install_memory_host_seams` comment says the seams
// exist to prevent: a sync run that looks empty rather than broken. Silence is
// the bug; these stubs are the fix.
//
// # Why stubs and not bus proxies
//
// Not a surface-budget choice — the trait shapes rule a proxy out.
// `SchedulerGate::current_policy` is sync and a bus call is not; `resume_notify`
// hands back a `tokio::sync::Notify`, a runtime primitive with no wire form; and
// `ShutdownHost::register` takes a Rust closure, which cannot be serialised at
// all. Mirroring the policy locally would need a signal this module does not
// declare (`signals = []`), a host half this crate does not own, and a cache
// that is wrong between ticks.
//
// # Why not synthesise a policy from the module's own config
//
// `ModuleConfig` carries `SchedulerGateConfig`, so `mode = off` looks
// answerable from here. It is not. The gate is *live* state — the user's toggle,
// AC power, CPU pressure, whether anyone is signed in — and the module holds the
// config it was loaded with. A module that paused itself on a stale `off` would
// stay paused after the user switched background AI back on, with no channel to
// learn otherwise and no way out short of a restart. Guessing is worse than not
// answering.
//
// # So: identical behaviour, no longer silent
//
// Each stub returns exactly what the unwired path returned, so installing them
// changes no scheduling and wedges nothing, and each reports once per process
// the first time anything actually consults it. The queue worker pool now runs
// in here (`crate::start_queue_pool`) and consults both, so the scheduler-gate
// report fires on every boot that drains a job — which is the honest signal that
// the throttle is not in effect.
//
// # The scheduler-gate stub now also silences two user-visible pauses
//
// `crate::start_sync_loops` moved the two periodic sync loops in here as well,
// and they consult this gate for something the queue pool does not: not a
// throttle but a *stop*. Step 0 of every tick in both loops is
// `sync::composio::periodic::periodic_pause_reason`, which exists to honour two
// states — `PauseReason::UserDisabled`, the user switching Memory Tree off in
// Settings, and `PauseReason::SignedOut`, no live session. It reads them off
// `current_policy()`, which is the stub, which always answers `Policy::Normal`.
// So in module mode both loops tick straight through both pauses: a user who
// switched memory off still gets background fetches, and a signed-out user still
// gets a Composio connection walk every 20 minutes.
//
// The per-source `enabled` toggle is unaffected — that is read from the source
// registry inside the tick, not from the gate — so switching off one source
// still works. It is the two *global* pauses that do not.
//
// The same stub's `resume_notify` hands back a `Notify` nobody fires, so the
// other half of that design is gone too: re-enabling sync no longer wakes the
// loops early, and the user waits out the remaining 20-minute tick instead of
// syncing within seconds. That half is benign; the paragraph above is not, and
// closing it needs the same `SchedulerGate` bus interface named below.
//
// The module-local registry this paragraph used to argue against now exists —
// see `ModuleShutdownHost` — and the argument is worth recording because it was
// right about the part that has not changed. Banking hooks and draining them on
// the module's own `Shutdown` method is only useful if something calls it, and
// at the time of writing nothing in the host does on the way out.
//
// What tipped it is that banking is not worse than dropping in the case the
// paragraph worried about, and is better in every other: a dropped hook never
// runs, a banked one runs the moment the host wires the call. The objection was
// really to going quiet, not to banking, so the gap is still announced at
// `install_seams` — now naming the precise condition ("only when the host calls
// Shutdown") instead of a flat "unserved" that a host-side fix would leave
// stale. See tinyhumansai/tinymemory#133 for the host half.

/// Latched so the gap is reported once per process, not once per job claim —
/// `wait_for_capacity` is consulted before every claim, and an unlatched report
/// would page on every poll. Same guard `queue::worker` puts on its own
/// storage-failure reports.
static GATE_REPORTED: AtomicBool = AtomicBool::new(false);

/// Log a degradation once per process, without reporting it as a defect.
///
/// The sibling of [`report_unserved_once`] for a gap that is a documented
/// design limit rather than something gone wrong. Both keep the log line; only
/// this one leaves the error reporter alone, so a limit the host already knows
/// about — it is the host that hands this module the frozen snapshot — stops
/// arriving as a classified error on the host's side.
pub(crate) fn warn_degraded_once(latch: &AtomicBool, message: &'static str) {
    if latch.swap(true, Ordering::SeqCst) {
        return;
    }
    log::warn!("[tinymemory:module] {message}");
}

/// What the missing scheduler gate costs, in the terms a reader of the log needs.
const GATE_UNSERVED: &str = "scheduler gate unserved in module mode: background memory work in \
                             this process runs ungated, ignoring the host's background-AI \
                             throttle (user toggle, AC power, CPU pressure, signed-out) — and \
                             the periodic sync loops here therefore also ignore the \
                             \"Memory Tree off\" and \"signed out\" pauses that would stop them";

/// Log and report a seam degradation once per process.
///
/// Shared with [`crate::composio`] and [`crate::config_loader`], which have
/// their own latches and their own messages but need exactly this behaviour:
/// one `log::error!` unconditionally, one classified report when a runtime
/// exists to send it on, and nothing at all on every later call. Each caller
/// owns its latch so one seam going quiet never silences another.
pub(crate) fn report_unserved_once(
    latch: &AtomicBool,
    message: &'static str,
    operation: &'static str,
) {
    if latch.swap(true, Ordering::SeqCst) {
        return;
    }
    log::error!("[tinymemory:module] {message}");
    // The error reporter reaches the host by spawning onto the module runtime,
    // and several call sites are sync methods a caller could reach from a plain
    // thread — the scheduler-gate stub below and both Composio probes. The log
    // line above is unconditional; only the telemetry needs a runtime to exist,
    // so the gap is never silent even when the report cannot be sent.
    if tokio::runtime::Handle::try_current().is_ok() {
        tinymemory_core::observability::report_error_or_expected(
            message,
            "memory",
            operation,
            &[("mode", "module")],
        );
    }
}

/// The host's background-AI throttle, which this module cannot observe.
///
/// Answers exactly what an uninstalled gate answered — see the section comment
/// above for why it must not answer anything else — and says so out loud the
/// first time it is asked.
#[derive(Debug)]
/// Scheduler gate answered by the host over the bus.
///
/// The host serves `SchedulerPolicy` on its `RuntimeHost` object (the same
/// object the event sink and error reporter already call), answering the
/// policy its own `cron::scheduler_gate` computes — mode, battery, CPU
/// pressure, signed-out. This gate polls it and caches the answer, because
/// [`SchedulerGate::current_policy`] is a synchronous step-0 read on every
/// queue claim and every periodic tick, and a bus round-trip per claim would
/// put the broker on the hot path.
///
/// What deliberately does NOT cross the bus: `wait_for_capacity`. The
/// LLM-slot semaphore is a host-process resource; a permit forged here would
/// be a lie about a semaphore this process cannot see. Policy pauses are the
/// consent-bearing half, and they cross. A host that serves no
/// `SchedulerPolicy` member (older host) degrades to exactly the previous
/// stub behaviour: `Policy::Normal`, reported once.
pub(crate) struct BusSchedulerGate {
    policy: std::sync::RwLock<tinymemory_core::scheduler_gate::Policy>,
    notify: Arc<tokio::sync::Notify>,
}

impl BusSchedulerGate {
    /// Store a freshly polled policy: log on change, and wake paused sleepers
    /// on a pause → not-paused transition so a resume is immediate rather than
    /// one tick late. Factored off the bus call so the transition rules are
    /// testable without a broker.
    fn store_policy(&self, next: tinymemory_core::scheduler_gate::Policy) {
        let (was_paused, changed) = {
            let mut slot = self
                .policy
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let was = matches!(
                *slot,
                tinymemory_core::scheduler_gate::Policy::Paused { .. }
            );
            let changed = *slot != next;
            *slot = next;
            (was, changed)
        };
        if changed {
            log::info!(
                "[tinymemory:module] scheduler policy from host: {next:?} — background claims \
                 honour it from the next tick"
            );
        }
        let now_paused = matches!(next, tinymemory_core::scheduler_gate::Policy::Paused { .. });
        if was_paused && !now_paused {
            self.notify.notify_waiters();
        }
    }

    /// Poll cadence while the host answers. Claims read the cache, so this
    /// bounds how stale a pause can be, not how often anything blocks.
    const POLL_SECS: u64 = 15;
    /// Poll cadence after a failed call — an older host answers
    /// `MemberNotFound` forever, and once a minute keeps the retirement of
    /// that host observable without spamming its log.
    const POLL_SECS_UNSERVED: u64 = 60;

    pub(crate) fn start(connection: tinybus::Connection) -> Arc<Self> {
        let gate = Arc::new(Self {
            policy: std::sync::RwLock::new(tinymemory_core::scheduler_gate::Policy::Normal),
            notify: Arc::new(tokio::sync::Notify::new()),
        });
        let poller = Arc::clone(&gate);
        tokio::spawn(async move {
            loop {
                let served = poller.refresh(&connection).await;
                let secs = if served {
                    Self::POLL_SECS
                } else {
                    Self::POLL_SECS_UNSERVED
                };
                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            }
        });
        gate
    }

    /// One poll: ask the host, map the wire strings, store, and wake sleepers
    /// on a pause → not-paused transition. Returns whether the member
    /// answered.
    async fn refresh(&self, connection: &tinybus::Connection) -> bool {
        let reply = match connection.proxy(
            RUNTIME_HOST_BUS_NAME,
            RUNTIME_HOST_OBJECT_PATH,
            RUNTIME_HOST_INTERFACE,
        ) {
            Ok(proxy) => {
                proxy
                    .call::<(String, Option<String>)>("SchedulerPolicy", ())
                    .await
            }
            Err(error) => Err(error),
        };
        match reply {
            Ok((tier, reason)) => {
                self.store_policy(wire_to_policy(&tier, reason.as_deref()));
                true
            }
            Err(error) => {
                report_unserved_once(&GATE_REPORTED, GATE_UNSERVED, "scheduler_gate");
                log::debug!(
                    "[tinymemory:module] SchedulerPolicy poll failed; keeping the last policy: {error}"
                );
                false
            }
        }
    }
}

#[async_trait]
impl tinymemory_core::scheduler_gate::SchedulerGate for BusSchedulerGate {
    fn current_policy(&self) -> tinymemory_core::scheduler_gate::Policy {
        *self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn resume_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.notify)
    }

    async fn wait_for_capacity(&self) -> Option<Box<dyn Send>> {
        // Host-process semaphore; see the struct docs. Policy pauses are
        // enforced by every claim's step-0 `current_policy` read instead.
        None
    }
}

/// Map the wire tier + pause-reason strings back onto the contract types.
///
/// Unknown strings collapse to the safe end of their type: an unknown tier is
/// `Normal` (the pre-gate behaviour, never a surprise pause), an unknown
/// pause reason is `PauseReason::Unknown` (still a pause — the host said
/// stop, and the unknown part is only the label).
fn wire_to_policy(tier: &str, reason: Option<&str>) -> tinymemory_core::scheduler_gate::Policy {
    use tinymemory_core::scheduler_gate::{PauseReason, Policy};
    match tier {
        "aggressive" => Policy::Aggressive,
        "throttled" => Policy::Throttled,
        "paused" => Policy::Paused {
            reason: match reason {
                Some("user_disabled") => PauseReason::UserDisabled,
                Some("on_battery") => PauseReason::OnBattery,
                Some("cpu_pressure") => PauseReason::CpuPressure,
                Some("signed_out") => PauseReason::SignedOut,
                _ => PauseReason::Unknown,
            },
        },
        // "normal" lands here with every unknown tier, deliberately in one
        // arm: an unknown tier degrades to the pre-gate behaviour.
        _ => Policy::Normal,
    }
}

#[derive(Debug)]
pub(crate) struct UnservedSchedulerGate;

#[async_trait]
impl tinymemory_core::scheduler_gate::SchedulerGate for UnservedSchedulerGate {
    fn current_policy(&self) -> tinymemory_core::scheduler_gate::Policy {
        report_unserved_once(&GATE_REPORTED, GATE_UNSERVED, "scheduler_gate");
        tinymemory_core::scheduler_gate::Policy::Normal
    }

    fn resume_notify(&self) -> Arc<tokio::sync::Notify> {
        report_unserved_once(&GATE_REPORTED, GATE_UNSERVED, "scheduler_gate");
        Arc::clone(IDLE_NOTIFY.get_or_init(|| Arc::new(tokio::sync::Notify::new())))
    }

    async fn wait_for_capacity(&self) -> Option<Box<dyn Send>> {
        report_unserved_once(&GATE_REPORTED, GATE_UNSERVED, "scheduler_gate");
        None
    }
}

/// A `Notify` nobody ever fires.
///
/// A `select!` on it simply never takes that arm, so the queue loops fall back
/// on their own tick cadence — which is what they did with no gate installed at
/// all. One per process rather than one per call, because every caller has to
/// receive the same handle for a wait on it to mean anything.
static IDLE_NOTIFY: std::sync::OnceLock<Arc<tokio::sync::Notify>> = std::sync::OnceLock::new();

/// Hooks the engine registered, waiting for this module's `Shutdown` member.
///
/// A plain `Mutex` because [`ShutdownHost::register`] is synchronous and the
/// engine registers from wherever the queue starts; the lock is held only long
/// enough to push or to take the list.
static BANKED_HOOKS: std::sync::Mutex<Vec<tinymemory_core::shutdown::ShutdownHook>> =
    std::sync::Mutex::new(Vec::new());

/// Longest any one hook may hold up the module's shutdown.
///
/// The bus call that triggers a shutdown carries the caller's deadline, so a
/// hook that wedges must not be able to outlast it — the caller would time out
/// and learn nothing. Releasing job locks is a handful of local SQLite writes;
/// anything past this is stuck, not slow, and the lease-expiry path at the next
/// startup is what it degrades to.
const HOOK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Banks the engine's shutdown hooks so [`run_shutdown_hooks`] can await them.
///
/// # Why this is banked here rather than dropped
///
/// The engine registers exactly one hook today: `queue::worker` releasing the
/// in-flight job locks so a clean restart re-claims that work immediately
/// instead of waiting out the lease. Dropping it guaranteed the slow path on
/// every launch.
///
/// This module does have a moment to await them — the `Shutdown` member it
/// already serves. What it does **not** have is a guarantee that anyone calls
/// it: a host that exits without shutting the driver down still leaves the
/// locks to expire, which is why [`install_seams`] keeps saying so out loud.
/// Banking is strictly better than dropping either way — the hook runs when the
/// host asks, instead of never — but it is only half of the fix, and the other
/// half lives in the host.
#[derive(Debug)]
pub(crate) struct ModuleShutdownHost;

impl tinymemory_core::shutdown::ShutdownHost for ModuleShutdownHost {
    fn register(&self, hook: tinymemory_core::shutdown::ShutdownHook) {
        match BANKED_HOOKS.lock() {
            Ok(mut hooks) => hooks.push(hook),
            // A poisoned lock means a previous holder panicked mid-push. The
            // hook is dropped rather than recovered: shutdown is best-effort
            // and the lease still expires, so this must not panic a caller.
            Err(_) => log::warn!(
                "[tinymemory:module] shutdown hook dropped: the hook registry is poisoned; \
                 in-flight job locks will be reclaimed by lease expiry instead"
            ),
        }
    }
}

/// Run and clear every banked shutdown hook.
///
/// Draining is what makes this idempotent, which the `Shutdown` member's
/// contract requires: a second call finds nothing banked and releases nothing
/// twice.
///
/// Each hook runs in its own task under [`HOOK_DEADLINE`], so one that panics
/// or wedges costs its own deadline and not the whole shutdown. Both outcomes
/// degrade to the same place a dropped hook did — lease expiry at the next
/// startup — so neither is worth failing the call over.
pub(crate) async fn run_shutdown_hooks() {
    // The guard is confined to this block deliberately. A `std::sync`
    // `MutexGuard` is not `Send`, so merely being in scope across the awaits
    // below would make this whole future non-`Send` — and the bus member that
    // calls it has to be. Taking the list here also means a hook that registers
    // another one cannot deadlock against a lock this function still holds.
    let hooks = {
        let Ok(mut banked) = BANKED_HOOKS.lock() else {
            log::warn!("[tinymemory:module] shutdown hooks skipped: the hook registry is poisoned");
            return;
        };
        std::mem::take(&mut *banked)
    };
    if hooks.is_empty() {
        return;
    }
    let total = hooks.len();
    log::info!("[tinymemory:module] running {total} banked shutdown hook(s)");
    for hook in hooks {
        // Spawned so a panic inside a hook surfaces as a `JoinError` here
        // rather than unwinding through the bus call.
        let mut task = tokio::spawn(async move { hook().await });
        // `&mut` so the handle survives the timeout. Passing it by value would
        // hand ownership to `timeout`, which drops it on expiry — and dropping
        // a `JoinHandle` **detaches** the task rather than cancelling it. The
        // hook would then still be running, still holding the store, while
        // `provider.shutdown()` released the backend underneath it.
        match tokio::time::timeout(HOOK_DEADLINE, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!(
                "[tinymemory:module] a shutdown hook panicked: {error}; its work is left to \
                 lease expiry at the next startup"
            ),
            Err(_) => {
                task.abort();
                // `abort` only requests cancellation, so await the handle to
                // know the task has actually stopped touching the store before
                // this returns and the caller tears the backend down. The
                // result is a `Cancelled` join error and carries nothing worth
                // reporting past the warning below.
                let _ = task.await;
                log::warn!(
                    "[tinymemory:module] a shutdown hook exceeded {HOOK_DEADLINE:?} and was \
                     cancelled; its work is left to lease expiry at the next startup"
                );
            }
        }
    }
}

/// Install the two seams this module can only stub, and name the gap at setup.
///
/// Kept separate from [`install`] on purpose: that function wires the seams the
/// host genuinely serves over the bus, and folding these in would blur the
/// difference between "wired" and "wired to nothing".
/// Install the host seams, bus-backing the scheduler gate when a connection
/// is available.
///
/// With a connection, the gate is [`BusSchedulerGate`] — the host's policy,
/// polled and cached — and only `shutdown` remains a stub. Without one (unit
/// tests, or a caller that has not connected yet), both fall back to the
/// unserved stubs, which keep the previous unwired behaviour and say so once.
pub(crate) fn install_seams(connection: Option<tinybus::Connection>) {
    match connection {
        Some(connection) => {
            tinymemory_core::scheduler_gate::set_scheduler_gate(BusSchedulerGate::start(
                connection,
            ));
        }
        None => {
            tinymemory_core::scheduler_gate::set_scheduler_gate(Arc::new(UnservedSchedulerGate));
        }
    }
    tinymemory_core::shutdown::set_shutdown_host(Arc::new(ModuleShutdownHost));
    // One line, once per process — `setup` runs exactly once. A warning rather
    // than a debug line because a reader of the log should not have to diff
    // seam lists to find out which host behaviours are not in effect here.
    //
    // Shutdown is half-served and the wording says which half: the hooks are
    // banked and this module runs them, but only if the host calls `Shutdown`.
    // A host that exits without doing so still leaves the locks to expire, and
    // that is not something this process can detect or fix from in here.
    log::warn!(
        "[tinymemory:module] shutdown hooks are banked and run on this module's Shutdown \
         member, so graceful queue-lock release is honoured only when the host shuts the \
         driver down before exiting; a host that exits without calling Shutdown still leaves \
         in-flight locks to lease expiry. The scheduler gate is bus-backed when the host \
         serves SchedulerPolicy, and degrades to the unwired Policy::Normal stub behaviour \
         when it does not"
    );
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod test;
