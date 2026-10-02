//! Tests for runtime-host callback argument ownership and safe diagnostics.

use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;
use tinybus::{Connection, Result as BusResult};
use tinymemory_api::host::{
    ErrorReporter, MemoryEvent, MemoryEventSink, SpacyEntity, SpacyResponse,
};

/// Empty the shutdown-hook bank without running anything.
///
/// The bank is process-wide, so a test that asserts on what ran needs a known
/// starting point — otherwise a hook another test registered would count. Pair
/// it with `seam_lock::hold_global_seams_async`, which is what stops two such
/// tests interleaving.
///
/// Lives here rather than beside the bank because production sources carry no
/// test-only executable code; a child module still reaches its ancestor's
/// private statics.
fn drain_banked_hooks() {
    if let Ok(mut hooks) = super::BANKED_HOOKS.lock() {
        hooks.clear();
    }
}

struct HostSeamsRestore {
    event_sink: Option<std::sync::Arc<dyn MemoryEventSink>>,
    error_reporter: Option<std::sync::Arc<dyn ErrorReporter>>,
    nlp_host: Option<std::sync::Arc<dyn tinymemory_core::nlp_host::NlpHost>>,
    scheduler_gate: Option<std::sync::Arc<dyn tinymemory_core::scheduler_gate::SchedulerGate>>,
    shutdown_host: Option<std::sync::Arc<dyn tinymemory_core::shutdown::ShutdownHost>>,
}

impl HostSeamsRestore {
    fn capture() -> Self {
        Self {
            event_sink: tinymemory_core::events::event_sink(),
            error_reporter: tinymemory_core::observability::error_reporter(),
            nlp_host: tinymemory_core::nlp_host::nlp_host(),
            scheduler_gate: tinymemory_core::scheduler_gate::scheduler_gate(),
            shutdown_host: tinymemory_core::shutdown::shutdown_host(),
        }
    }
}

impl Drop for HostSeamsRestore {
    fn drop(&mut self) {
        match self.event_sink.take() {
            Some(sink) => tinymemory_core::events::set_event_sink(sink),
            None => tinymemory_core::events::clear_event_sink(),
        }
        match self.error_reporter.take() {
            Some(reporter) => tinymemory_core::observability::set_error_reporter(reporter),
            None => tinymemory_core::observability::clear_error_reporter(),
        }
        match self.nlp_host.take() {
            Some(host) => tinymemory_core::nlp_host::set_nlp_host(host),
            None => tinymemory_core::nlp_host::clear_nlp_host(),
        }
        match self.scheduler_gate.take() {
            Some(gate) => tinymemory_core::scheduler_gate::set_scheduler_gate(gate),
            None => tinymemory_core::scheduler_gate::clear_scheduler_gate(),
        }
        match self.shutdown_host.take() {
            Some(host) => tinymemory_core::shutdown::set_shutdown_host(host),
            None => tinymemory_core::shutdown::clear_shutdown_host(),
        }
    }
}

#[derive(Debug)]
enum Callback {
    Published(MemoryEvent),
    Error {
        expected: bool,
        rendered: String,
        domain: String,
        operation: String,
        tags: Vec<(String, String)>,
    },
}

struct FakeRuntimeHost {
    callbacks: tokio::sync::mpsc::UnboundedSender<Callback>,
}

#[tinybus::interface(name = "ai.tinyhumans.tinymemory.RuntimeHost")]
impl FakeRuntimeHost {
    async fn publish_event(&self, event: MemoryEvent) -> BusResult<()> {
        std::future::ready(()).await;
        let _ = self.callbacks.send(Callback::Published(event));
        Ok(())
    }

    #[allow(clippy::too_many_arguments, reason = "wire contract")]
    async fn report_error(
        &self,
        expected: bool,
        rendered: String,
        domain: String,
        operation: String,
        tags: Vec<(String, String)>,
    ) -> BusResult<()> {
        std::future::ready(()).await;
        let _ = self.callbacks.send(Callback::Error {
            expected,
            rendered,
            domain,
            operation,
            tags,
        });
        Ok(())
    }

    async fn extract_spacy(&self, text: String) -> BusResult<SpacyResponse> {
        std::future::ready(()).await;
        Ok(SpacyResponse {
            entities: vec![SpacyEntity {
                text,
                label: "ORG".to_string(),
                start: 0,
                end: 10,
            }],
            nouns: vec!["memory".to_string()],
        })
    }
}

async fn bus_with_runtime_host() -> (Connection, tokio::sync::mpsc::UnboundedReceiver<Callback>) {
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let _broker_task = broker.spawn(bus.clone());
    let (callbacks, receiver) = tokio::sync::mpsc::unbounded_channel();
    let host = Connection::connect(bus.connect().await.expect("host transport"))
        .await
        .expect("host connection");
    host.serve_at(
        super::RUNTIME_HOST_OBJECT_PATH
            .try_into()
            .expect("runtime host path"),
        FakeRuntimeHost { callbacks },
    )
    .await
    .expect("serve runtime host");
    host.request_name(super::RUNTIME_HOST_BUS_NAME)
        .await
        .expect("claim runtime host name");
    std::mem::forget(host);
    let module = Connection::connect(bus.connect().await.expect("module transport"))
        .await
        .expect("module connection");
    (module, receiver)
}

#[test]
fn callback_tags_are_owned_without_changing_order_or_values() {
    let key = String::from("source");
    let value = String::from("sync");
    let owned = super::owned_tags(&[(&key, &value), ("attempt", "2")]);
    drop(key);
    drop(value);
    assert_eq!(
        owned,
        vec![
            ("source".to_string(), "sync".to_string()),
            ("attempt".to_string(), "2".to_string())
        ]
    );
}

#[tokio::test]
async fn absent_runtime_host_returns_an_error_instead_of_hanging() {
    use tinymemory_core::nlp_host::NlpHost;

    let bus = MemoryBus::new();
    let broker = tinybus::broker::Broker::new();
    let _broker_task = broker.spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.expect("transport"))
        .await
        .expect("connection");
    let host = super::BusRuntimeHost::new(connection);
    let config = crate::config::ModuleConfig::default();
    let runtime = tinymemory_tinycortex::engine::EngineRuntimeConfig::from(&config);
    let error = host
        .extract_spacy(&runtime, "text")
        .await
        .expect_err("no runtime host is served");
    assert!(error.contains(super::RUNTIME_HOST_BUS_NAME), "{error}");
    assert!(!format!("{host:?}").contains("Connection"));
}

#[tokio::test]
async fn runtime_callbacks_and_spacy_cross_the_bus_with_their_full_payloads() {
    use tinymemory_core::nlp_host::NlpHost;

    let (connection, mut callbacks) = bus_with_runtime_host().await;
    let host = super::BusRuntimeHost::new(connection);
    host.publish(MemoryEvent::IngestionStarted {
        document_id: "document-7".to_string(),
        title: "Coverage".to_string(),
        namespace: "test".to_string(),
        queue_depth: 3,
    });
    host.report_error("failed", "sync", "publish", &[("source", "unit-test")]);
    host.report_error_or_expected("not found", "recall", "lookup", &[("namespace", "test")]);

    let config = crate::config::ModuleConfig::default();
    let runtime = tinymemory_tinycortex::engine::EngineRuntimeConfig::from(&config);
    let response = host
        .extract_spacy(&runtime, "TinyMemory")
        .await
        .expect("runtime host extracts entities");
    assert_eq!(response.entities.len(), 1);
    assert_eq!(response.entities[0].text, "TinyMemory");
    assert_eq!(response.entities[0].label, "ORG");
    assert_eq!(response.nouns, ["memory"]);

    let mut published = false;
    let mut ordinary_error = false;
    let mut expected_error = false;
    for _ in 0..3 {
        let callback = tokio::time::timeout(std::time::Duration::from_secs(1), callbacks.recv())
            .await
            .expect("callback arrives promptly")
            .expect("callback channel remains open");
        match callback {
            Callback::Published(MemoryEvent::IngestionStarted {
                document_id,
                title,
                namespace,
                queue_depth,
            }) => {
                assert_eq!(document_id, "document-7");
                assert_eq!(title, "Coverage");
                assert_eq!(namespace, "test");
                assert_eq!(queue_depth, 3);
                published = true;
            }
            Callback::Published(other) => panic!("unexpected event: {other:?}"),
            Callback::Error {
                expected,
                rendered,
                domain,
                operation,
                tags,
            } => {
                if expected {
                    assert_eq!(rendered, "not found");
                    assert_eq!(domain, "recall");
                    assert_eq!(operation, "lookup");
                    assert_eq!(tags, [("namespace".to_string(), "test".to_string())]);
                    expected_error = true;
                } else {
                    assert_eq!(rendered, "failed");
                    assert_eq!(domain, "sync");
                    assert_eq!(operation, "publish");
                    assert_eq!(tags, [("source".to_string(), "unit-test".to_string())]);
                    ordinary_error = true;
                }
            }
        }
    }
    assert!(published);
    assert!(ordinary_error);
    assert!(expected_error);
}

/// Kept as one test rather than two on purpose: both installs write
/// process-global seams, and a second test that captured, installed and
/// asserted in parallel with this one could have its assertion land after this
/// one's `HostSeamsRestore` had already put the globals back.
#[tokio::test]
async fn install_wires_every_seam_this_module_can_supply() {
    // Taken before the capture, so the state this restores on drop is the
    // state no other test can be moving underneath it. See `seam_lock`.
    let _seams = crate::seam_lock::hold_global_seams_async().await;
    let _restore = HostSeamsRestore::capture();
    let (connection, _callbacks) = bus_with_runtime_host().await;

    // The pair `setup` calls, in the order it calls them.
    super::install(connection);
    super::install_seams(None);

    assert!(tinymemory_core::events::event_sink().is_some());
    assert!(tinymemory_core::observability::error_reporter().is_some());
    assert!(tinymemory_core::nlp_host::nlp_host().is_some());
    // The two that used to be left out entirely, and so degraded in silence
    // instead of failing with a named cause the way `config_loader` does.
    assert!(tinymemory_core::scheduler_gate::scheduler_gate().is_some());
    assert!(tinymemory_core::shutdown::shutdown_host().is_some());
}

#[test]
fn the_unserved_scheduler_gate_answers_exactly_what_an_unwired_seam_answered() {
    use tinymemory_core::scheduler_gate::SchedulerGate;

    // Loud, not different. A stub that answered anything else would change
    // scheduling as a side effect of loading the module — and with no channel
    // to the host's live gate, any other answer would be a guess that goes
    // stale the moment the user toggles background AI.
    assert_eq!(
        super::UnservedSchedulerGate.current_policy(),
        tinymemory_core::scheduler_gate::Policy::Normal
    );
}

/// A registered hook is banked and runs when the module is shut down.
///
/// This is the whole point of the seam: the engine's one hook releases the
/// in-flight job locks, and before it was banked it was dropped, so a clean
/// restart always waited out the lease instead of re-claiming the work.
#[tokio::test]
async fn a_registered_hook_is_banked_and_runs_on_shutdown() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tinymemory_core::shutdown::ShutdownHost;

    // Declared before any statement: the module crate denies items appearing
    // after statements, and a process-wide counter is what lets the assertions
    // below distinguish "ran once" from "ran twice".
    static RUNS: AtomicUsize = AtomicUsize::new(0);

    let _seams = crate::seam_lock::hold_global_seams_async().await;
    drain_banked_hooks();
    RUNS.store(0, Ordering::SeqCst);

    super::ModuleShutdownHost.register(Box::new(|| {
        Box::pin(async {
            RUNS.fetch_add(1, Ordering::SeqCst);
        })
    }));
    assert_eq!(
        RUNS.load(Ordering::SeqCst),
        0,
        "registering must not run it"
    );

    super::run_shutdown_hooks().await;
    assert_eq!(RUNS.load(Ordering::SeqCst), 1, "the banked hook ran");

    // Draining is what makes the `Shutdown` member idempotent, which its
    // contract requires: a second call must not release the same locks twice.
    super::run_shutdown_hooks().await;
    assert_eq!(
        RUNS.load(Ordering::SeqCst),
        1,
        "a second shutdown runs nothing"
    );
}

/// One bad hook costs its own deadline, not the shutdown.
///
/// Both a panic and a wedge degrade to where a dropped hook already left the
/// work — lease expiry at the next startup — so neither may take the rest of
/// the shutdown sequence down with it.
#[tokio::test]
async fn a_panicking_hook_does_not_stop_the_others() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tinymemory_core::shutdown::ShutdownHost;

    static SURVIVORS: AtomicUsize = AtomicUsize::new(0);

    let _seams = crate::seam_lock::hold_global_seams_async().await;
    drain_banked_hooks();
    SURVIVORS.store(0, Ordering::SeqCst);

    super::ModuleShutdownHost.register(Box::new(|| Box::pin(async { panic!("hook exploded") })));
    super::ModuleShutdownHost.register(Box::new(|| {
        Box::pin(async {
            SURVIVORS.fetch_add(1, Ordering::SeqCst);
        })
    }));

    super::run_shutdown_hooks().await;
    assert_eq!(
        SURVIVORS.load(Ordering::SeqCst),
        1,
        "the hook after the panicking one still ran"
    );
}

/// A hook that never finishes is cancelled at its deadline, and the next one
/// still runs.
///
/// The deadline is the guard against a wedged hook outliving the bus call that
/// triggered the shutdown, so it is worth asserting rather than assuming. On a
/// paused clock the timeout fires the moment the runtime goes idle, so this
/// costs no real time and cannot flake on a loaded machine.
///
/// It also pins the cancellation: the hook must not still be running when this
/// returns, because the caller releases the store next.
#[tokio::test(start_paused = true)]
async fn a_hook_that_never_finishes_is_cancelled_at_its_deadline() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tinymemory_core::shutdown::ShutdownHost;

    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static FINISHED: AtomicUsize = AtomicUsize::new(0);
    static AFTER: AtomicUsize = AtomicUsize::new(0);

    let _seams = crate::seam_lock::hold_global_seams_async().await;
    drain_banked_hooks();
    STARTED.store(0, Ordering::SeqCst);
    FINISHED.store(0, Ordering::SeqCst);
    AFTER.store(0, Ordering::SeqCst);

    super::ModuleShutdownHost.register(Box::new(|| {
        Box::pin(async {
            STARTED.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            FINISHED.fetch_add(1, Ordering::SeqCst);
        })
    }));
    super::ModuleShutdownHost.register(Box::new(|| {
        Box::pin(async {
            AFTER.fetch_add(1, Ordering::SeqCst);
        })
    }));

    super::run_shutdown_hooks().await;

    assert_eq!(STARTED.load(Ordering::SeqCst), 1, "the stalled hook ran");
    assert_eq!(
        FINISHED.load(Ordering::SeqCst),
        0,
        "it was cancelled rather than awaited to completion"
    );
    assert_eq!(
        AFTER.load(Ordering::SeqCst),
        1,
        "one hook exceeding its deadline does not skip the rest"
    );
}

#[tokio::test]
async fn shutting_down_with_nothing_banked_is_a_no_op() {
    let _seams = crate::seam_lock::hold_global_seams_async().await;
    drain_banked_hooks();
    // Reached whenever a host shuts a driver down before the queue ever
    // started, which is ordinary rather than an error.
    super::run_shutdown_hooks().await;
}

#[tokio::test]
async fn fire_and_forget_notification_tolerates_an_absent_host() {
    let bus = MemoryBus::new();
    let broker = Broker::new();
    let _broker_task = broker.spawn(bus.clone());
    let connection = Connection::connect(bus.connect().await.expect("transport"))
        .await
        .expect("connection");
    let host = super::BusRuntimeHost::new(connection);

    host.publish(MemoryEvent::IngestionStarted {
        document_id: "missing-host".to_string(),
        title: String::new(),
        namespace: "test".to_string(),
        queue_depth: 0,
    });
    tokio::task::yield_now().await;
}

// ── bus scheduler gate (scheduler-gate round) ────────────────────────────────

/// A gate with no poller: `store_policy` driven by hand. Lives here rather
/// than as an inline `#[cfg(test)]` constructor because the coverage lanes
/// filter test files by name, and inline test-only code pollutes the
/// measured production lines (the powerset lane enforces exactly that).
fn gate_for_test() -> std::sync::Arc<super::BusSchedulerGate> {
    std::sync::Arc::new(super::BusSchedulerGate {
        policy: std::sync::RwLock::new(tinymemory_core::scheduler_gate::Policy::Normal),
        notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    })
}

#[test]
fn wire_to_policy_maps_every_tier_and_reason() {
    use tinymemory_core::scheduler_gate::{PauseReason, Policy};
    assert_eq!(
        super::wire_to_policy("aggressive", None),
        Policy::Aggressive
    );
    assert_eq!(super::wire_to_policy("throttled", None), Policy::Throttled);
    // "normal" and every unknown tier share one deliberate arm: the pre-gate
    // behaviour, never a surprise pause.
    assert_eq!(super::wire_to_policy("normal", None), Policy::Normal);
    assert_eq!(
        super::wire_to_policy("something-newer", None),
        Policy::Normal
    );
    for (wire, reason) in [
        ("user_disabled", PauseReason::UserDisabled),
        ("on_battery", PauseReason::OnBattery),
        ("cpu_pressure", PauseReason::CpuPressure),
        ("signed_out", PauseReason::SignedOut),
        ("unheard-of", PauseReason::Unknown),
    ] {
        assert_eq!(
            super::wire_to_policy("paused", Some(wire)),
            Policy::Paused { reason },
            "reason wire {wire}"
        );
    }
    // A pause with no reason string is still a pause.
    assert_eq!(
        super::wire_to_policy("paused", None),
        Policy::Paused {
            reason: PauseReason::Unknown
        }
    );
}

#[tokio::test]
async fn store_policy_wakes_sleepers_only_on_resume() {
    use tinymemory_core::scheduler_gate::{PauseReason, Policy, SchedulerGate};
    let gate = gate_for_test();
    assert_eq!(gate.current_policy(), Policy::Normal);

    gate.store_policy(Policy::Paused {
        reason: PauseReason::UserDisabled,
    });
    assert!(matches!(gate.current_policy(), Policy::Paused { .. }));

    // A sleeper parked on the resume handle wakes when the pause lifts.
    let notify = gate.resume_notify();
    let waiter = tokio::spawn(async move { notify.notified().await });
    tokio::task::yield_now().await;
    gate.store_policy(Policy::Normal);
    tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("resume must wake the sleeper")
        .expect("waiter task");
    assert_eq!(gate.current_policy(), Policy::Normal);

    // Same-policy stores are quiet no-ops.
    gate.store_policy(Policy::Normal);
    assert_eq!(gate.current_policy(), Policy::Normal);
}

#[test]
fn manual_override_outranks_a_paused_gate_and_is_bounded() {
    use tinymemory_core::scheduler_gate as core_gate;
    use tinymemory_core::scheduler_gate::{PauseReason, Policy};
    let _seams = crate::seam_lock::hold_global_seams();
    core_gate::clear_manual_override();
    let gate = gate_for_test();
    gate.store_policy(Policy::Paused {
        reason: PauseReason::UserDisabled,
    });
    core_gate::set_scheduler_gate(gate);
    assert!(matches!(core_gate::current_policy(), Policy::Paused { .. }));

    // The member's whole contract: user-initiated work wins while the window
    // is open, and only while it is open.
    core_gate::set_manual_override(60);
    assert_eq!(core_gate::current_policy(), Policy::Normal);
    core_gate::clear_manual_override();
    assert!(matches!(core_gate::current_policy(), Policy::Paused { .. }));

    core_gate::clear_scheduler_gate();
}
