//! Tests for the service's error mapping.
//!
//! This is the module's half of the wire contract: every `MemoryError` the
//! engine can raise has to leave as a named bus error the host's client can map
//! back. `tinymemory_api::wire_tests` pins the table itself; what is tested here
//! is that the service actually goes through it.
//!
//! Covered here rather than in the loader E2E deliberately. An E2E can only
//! provoke the errors the engine happens to raise for a given input, which makes
//! it a test of engine internals this port does not own — an earlier revision
//! tried `ExportPage` with a zero limit, and since a driver accepting a zero
//! limit is equally legitimate, the test asserted nothing when it passed. Here
//! every variant is reachable by construction.

use tinybus::Error as BusError;
use tinymemory_api::error::MemoryError;
use tinymemory_api::tree::{NodeLevel, TreeNode};
use tinymemory_api::wire;

use super::into_bus_error;

fn test_provider() -> std::sync::Arc<dyn tinymemory_api::provider::MemoryProvider> {
    std::sync::Arc::new(tinymemory_tinycortex::provider(std::sync::Arc::new(
        tinycortex::memory::store::InMemoryMemoryStore::new(),
    )))
}

async fn test_connection() -> tinybus::Connection {
    use tinybus::transport::memory::MemoryBus;

    let bus = MemoryBus::new();
    let broker = tinybus::broker::Broker::new();
    let _broker_task = broker.spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await.expect("test transport"))
        .await
        .expect("test connection");
    connection
        .request_name(super::BUS_NAME)
        .await
        .expect("claim test service name");
    connection
}

fn test_config(workspace: &std::path::Path) -> crate::config::ModuleConfig {
    crate::config::ModuleConfig {
        workspace_dir: workspace.to_path_buf(),
        ..crate::config::ModuleConfig::default()
    }
}

/// Holds the embedding-host test mutex while a temporary host is installed.
///
/// Restoring in `Drop` keeps the process global correct even when an assertion
/// panics. The mutex guard is deliberately retained for the whole scope: the
/// factory reads the host during each `OpenStore`, not just during setup.
struct EmbeddingHostRestore {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::sync::Arc<dyn tinymemory_core::embedding_host::EmbeddingHost>>,
}

impl EmbeddingHostRestore {
    fn install(connection: tinybus::Connection, config: &crate::config::ModuleConfig) -> Self {
        let lock = tinymemory_core::embedding_host::embedding_test_guard();
        let previous = tinymemory_core::embedding_host::embedding_host();
        tinymemory_core::embedding_host::set_embedding_host(std::sync::Arc::new(
            crate::embedding::BusEmbeddingHost::new(connection, config),
        ));
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for EmbeddingHostRestore {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => tinymemory_core::embedding_host::set_embedding_host(previous),
            None => tinymemory_core::embedding_host::clear_embedding_host(),
        }
    }
}

fn test_opener(
    connection: tinybus::Connection,
    config: crate::config::ModuleConfig,
) -> std::sync::Arc<super::StoreOpener> {
    std::sync::Arc::new(super::StoreOpener::new(connection, config))
}

/// The name and message a mapped error carries on the wire.
fn mapped(error: &MemoryError) -> (String, String) {
    match into_bus_error(error) {
        BusError::MethodFailed { name, message } => (name, message),
        other => panic!("expected MethodFailed, got {other:?}"),
    }
}

#[test]
fn every_variant_leaves_under_its_contract_name() {
    // Exhaustive by construction: `wire::wire_name` is a total match over
    // `MemoryError`, so a new variant fails to compile there before it can
    // silently leave this list.
    let cases = [
        (MemoryError::NotFound("k".into()), wire::NOT_FOUND),
        (MemoryError::Invalid("bad".into()), wire::INVALID),
        (
            MemoryError::BudgetExceeded("too big".into()),
            wire::BUDGET_EXCEEDED,
        ),
        (
            MemoryError::PathEscape("../outside".into()),
            wire::PATH_ESCAPE,
        ),
        (MemoryError::unsupported_raw("tree"), wire::UNSUPPORTED),
        (
            MemoryError::Other(anyhow::anyhow!("engine fell over")),
            wire::OTHER,
        ),
    ];

    for (error, expected) in &cases {
        let (name, _) = mapped(error);
        assert_eq!(&name, expected, "{error:?} left under the wrong name");
    }
}

#[test]
fn a_path_escape_never_leaves_as_an_invalid() {
    // The security-relevant collapse. `Invalid` tells a caller its input was
    // malformed and invites a retry; a sandbox escape is not that, and the host
    // re-raises whatever it receives to its own callers.
    let (name, _) = mapped(&MemoryError::PathEscape("../../etc".into()));
    assert_eq!(name, wire::PATH_ESCAPE);
    assert_ne!(name, wire::INVALID);
}

#[test]
fn a_miss_never_leaves_as_an_invalid() {
    // `get`'s contract makes a miss `Ok(None)`, so a `NotFound` that arrived as
    // `Invalid` would turn an ordinary absence into a caller-visible failure.
    let (name, _) = mapped(&MemoryError::NotFound("absent".into()));
    assert_eq!(name, wire::NOT_FOUND);
    assert_ne!(name, wire::INVALID);
}

#[test]
fn the_names_the_service_emits_are_the_ones_the_host_decodes() {
    // The drift that matters is silent, so this closes the loop rather than
    // trusting the two tables to agree: map out through the service, back
    // through the client's decoder, and require the variant to survive.
    let originals = [
        MemoryError::NotFound("k".into()),
        MemoryError::Invalid("bad".into()),
        MemoryError::BudgetExceeded("too big".into()),
        MemoryError::PathEscape("../outside".into()),
        MemoryError::unsupported_raw("tree"),
    ];

    for original in &originals {
        let (name, message) = mapped(original);
        let decoded = wire::from_wire(&name, &message);
        assert_eq!(
            std::mem::discriminant(&decoded),
            std::mem::discriminant(original),
            "{original:?} did not survive the round trip, arrived as {decoded:?}"
        );
    }
}

#[test]
fn a_message_carries_no_user_content_beyond_what_the_engine_put_there() {
    // Not a redaction test — the engine owns its message. This pins that the
    // service adds nothing of its own, so the only thing that can leak is what
    // the engine already chose to say.
    let error = MemoryError::NotFound("some-key".into());
    let (_, message) = mapped(&error);
    assert_eq!(message, wire::wire_message(&error));
}

/// An entry whose content is `bytes` long.
fn entry_of(bytes: usize) -> tinymemory_api::types::MemoryEntry {
    tinymemory_api::types::MemoryEntry {
        id: "id".into(),
        key: "key".into(),
        content: "x".repeat(bytes),
        namespace: Some("ns".into()),
        category: tinymemory_api::types::MemoryCategory::Core,
        timestamp: "2026-01-01T00:00:00Z".into(),
        session_id: None,
        score: None,
        taint: tinymemory_api::types::MemoryTaint::Internal,
    }
}

#[test]
fn an_ordinary_list_response_is_not_refused() {
    // The ceiling must not be so tight that normal use trips it. A hundred
    // entries of a kilobyte each is an unremarkable namespace.
    let entries: Vec<_> = (0..100).map(|_| entry_of(1024)).collect();
    assert!(super::ensure_response_fits(&entries, "List").is_ok());
}

#[test]
fn an_empty_list_response_is_not_refused() {
    assert!(
        super::ensure_response_fits(&Vec::<tinymemory_api::types::MemoryEntry>::new(), "List")
            .is_ok()
    );
}

#[test]
fn a_response_over_the_ceiling_is_refused_as_a_budget_error() {
    // `List` takes no limit and no cursor, so entries accumulate across
    // individually valid `Store` calls until the response cannot cross a
    // 16 MiB frame. Without this check the caller gets a transport failure it
    // cannot act on; with it, a named error that says how to narrow the query.
    let entries: Vec<_> = (0..2)
        .map(|_| entry_of(super::MAX_RESPONSE_BYTES))
        .collect();

    let error = super::ensure_response_fits(&entries, "List")
        .expect_err("a response over the ceiling must be refused");
    match error {
        BusError::MethodFailed { name, message } => {
            assert_eq!(
                name,
                wire::BUDGET_EXCEEDED,
                "must use a name the host already decodes"
            );
            assert!(message.contains("List"), "{message}");
            assert!(
                message.contains("narrow"),
                "the message must tell the caller what to do: {message}"
            );
        }
        other => panic!("expected MethodFailed, got {other:?}"),
    }
}

#[test]
fn the_refusal_decodes_host_side_as_a_budget_error() {
    // The whole point of reusing an existing name: a new one would decode to
    // `Other` on any host older than the module, turning an actionable "narrow
    // your query" into an opaque backend failure.
    let entries: Vec<_> = (0..2)
        .map(|_| entry_of(super::MAX_RESPONSE_BYTES))
        .collect();

    let BusError::MethodFailed { name, message } =
        super::ensure_response_fits(&entries, "List").expect_err("refused")
    else {
        panic!("expected MethodFailed");
    };

    let decoded = wire::from_wire(&name, &message);
    assert!(
        matches!(decoded, MemoryError::BudgetExceeded(_)),
        "{decoded:?}"
    );
}

#[test]
fn the_refusal_message_carries_no_entry_content() {
    // Entry content is user memory. The message names sizes and the method, and
    // nothing that was stored.
    let secret = "correct-horse-battery-staple";
    let mut entries: Vec<_> = (0..2)
        .map(|_| entry_of(super::MAX_RESPONSE_BYTES))
        .collect();
    entries[0].content.push_str(secret);

    let BusError::MethodFailed { message, .. } =
        super::ensure_response_fits(&entries, "List").expect_err("refused")
    else {
        panic!("expected MethodFailed");
    };
    assert!(!message.contains(secret), "{message}");
}

#[test]
fn the_per_entry_overhead_is_counted_so_many_tiny_entries_still_trip_it() {
    // A million empty entries carry no content at all but still cannot cross a
    // frame — the JSON structure around each one is the payload. Counting only
    // `content.len()` would let this through.
    let encoded_entry = serde_json::to_vec(&entry_of(0))
        .expect("serializable")
        .len();
    let count = super::MAX_RESPONSE_BYTES / encoded_entry + 1;
    let entries: Vec<_> = (0..count).map(|_| entry_of(0)).collect();
    assert!(
        super::ensure_response_fits(&entries, "List").is_err(),
        "entries with no content must still be counted"
    );
}

#[test]
fn store_object_paths_accept_only_one_safe_identifier_component() {
    let valid = [
        ("profile-1", "profile_2d1".to_string()),
        ("profile_one", "profile_5fone".to_string()),
        ("A9", "A9".to_string()),
        (&"x".repeat(128), "x".repeat(128)),
    ];
    for (subdir, component) in valid {
        assert_eq!(
            super::object_path_for_subdir(subdir),
            Some(format!("{}/stores/{component}", super::OBJECT_PATH))
        );
    }

    assert_ne!(
        super::object_path_for_subdir("a-b"),
        super::object_path_for_subdir("a_2db"),
        "escaped identifiers must not collide"
    );

    for invalid in [
        "",
        ".",
        "..",
        "../escape",
        "nested/store",
        "nested\\store",
        "profile.name",
        "profile name",
        "pröfile",
        &"x".repeat(129),
    ] {
        assert!(
            super::object_path_for_subdir(invalid).is_none(),
            "unsafe subdirectory was admitted: {invalid:?}"
        );
    }
}

#[tokio::test]
async fn a_leaf_store_cannot_recursively_open_another_store() {
    let service = super::MemoryService::new(test_provider());
    let error = service
        .open_store("child".to_string())
        .await
        .expect_err("leaf stores must not recursively open stores");
    let tinybus::Error::MethodFailed { name, message } = error else {
        panic!("expected MethodFailed");
    };
    assert_eq!(name, tinymemory_api::wire::INVALID);
    assert!(message.contains("root"));
}

#[tokio::test]
async fn repeated_and_concurrent_opens_reuse_the_registered_object_path() {
    use std::sync::Arc;

    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let config = test_config(workspace.path());
    let _embedding_host = EmbeddingHostRestore::install(connection.clone(), &config);
    let opener = test_opener(connection.clone(), config);
    let expected = format!("{}/stores/profile_2d1", super::OBJECT_PATH);
    let service = Arc::new(super::MemoryService::root(
        test_provider(),
        Arc::clone(&opener),
    ));

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let service = Arc::clone(&service);
        tasks.push(tokio::spawn(async move {
            service.open_store("profile-1".to_string()).await
        }));
    }
    for task in tasks {
        assert_eq!(task.await.expect("join").expect("reused store"), expected);
    }
    assert_eq!(opener.instrumentation.allocation_attempts(), 1);
    assert_eq!(opener.instrumentation.registration_attempts(), 1);
    assert_eq!(opener.served.lock().await.len(), 1);

    let driver_id: String = connection
        .proxy(super::BUS_NAME, &expected, super::BUS_NAME)
        .expect("store proxy")
        .call("DriverId", ())
        .await
        .expect("the newly registered object must answer");
    assert_eq!(driver_id, "tinycortex");
}

#[tokio::test]
async fn a_failed_registration_is_retried_and_only_success_counts_toward_the_cap() {
    use std::sync::Arc;

    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let config = test_config(workspace.path());
    let _embedding_host = EmbeddingHostRestore::install(connection.clone(), &config);
    let opener = test_opener(connection, config);
    opener.instrumentation.fail_registrations(1);
    let service = super::MemoryService::root(test_provider(), Arc::clone(&opener));
    service
        .open_store("retry".to_string())
        .await
        .expect_err("the first registration is injected to fail");
    assert!(opener.served.lock().await.is_empty());

    let path = service
        .open_store("retry".to_string())
        .await
        .expect("the same subtree must be retried");
    assert_eq!(path, format!("{}/stores/retry", super::OBJECT_PATH));
    assert_eq!(opener.instrumentation.allocation_attempts(), 2);
    assert_eq!(opener.instrumentation.registration_attempts(), 2);
    assert_eq!(opener.served.lock().await.len(), 1);
}

#[tokio::test]
async fn the_open_store_cap_is_reached_through_successful_opens() {
    use std::sync::Arc;

    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let config = test_config(workspace.path());
    let _embedding_host = EmbeddingHostRestore::install(connection.clone(), &config);
    let opener = test_opener(connection, config);
    let service = super::MemoryService::root(test_provider(), Arc::clone(&opener));

    for index in 0..super::MAX_OPEN_STORES {
        service
            .open_store(format!("profile-{index}"))
            .await
            .unwrap_or_else(|error| panic!("successful open {index} failed: {error}"));
    }
    let error = service
        .open_store("one-more".to_string())
        .await
        .expect_err("the store cap must be enforced");
    let tinybus::Error::MethodFailed { name, message } = error else {
        panic!("expected MethodFailed");
    };
    assert_eq!(name, tinymemory_api::wire::INVALID);
    assert!(message.contains(&super::MAX_OPEN_STORES.to_string()));
    assert_eq!(opener.served.lock().await.len(), super::MAX_OPEN_STORES);
    assert_eq!(
        opener.instrumentation.allocation_attempts(),
        super::MAX_OPEN_STORES,
        "the refused open must not allocate"
    );
    assert_eq!(
        opener.instrumentation.registration_attempts(),
        super::MAX_OPEN_STORES,
        "the refused open must not register"
    );
}

/// The queue worker pool is claimed once per process, and a store under a
/// second workspace is refused loudly rather than left with no pool.
///
/// Asserted through `claim_queue_pool` rather than `start_queue_pool` on
/// purpose. Starting the pool for real spawns four job workers and a daily
/// scheduler against a temporary directory the test deletes while they are
/// still polling it; they then mark the store degraded process-wide, which
/// every later test that reads health would inherit. The claim is the whole of
/// the decision — what follows it is one call into `tinymemory-core`, whose own
/// `Once` guards it a second time.
///
/// The three outcomes are asserted in one test because the cell behind them is
/// a process-global `OnceLock`: split across three tests they would race, and
/// only the first to run would see `Start`.
#[test]
fn the_queue_pool_is_claimed_once_and_a_foreign_workspace_is_refused() {
    let workspace = std::path::Path::new("/tinymemory-module/queue-pool-claim");
    let elsewhere = std::path::Path::new("/tinymemory-module/queue-pool-elsewhere");

    assert_eq!(
        crate::claim_queue_pool(workspace),
        crate::WorkspaceClaim::Start,
        "the first claim must be the one that starts the pool"
    );
    assert_eq!(
        crate::claim_queue_pool(workspace),
        crate::WorkspaceClaim::AlreadyRunning,
        "a second claim for the same workspace must not start a second pool"
    );
    assert_eq!(
        crate::claim_queue_pool(elsewhere),
        crate::WorkspaceClaim::Foreign,
        "a claim for another workspace must be named, not silently swallowed — \
         `queue::start` would no-op and that store's queue would never drain"
    );
}

/// The periodic sync loops are claimed the same way, and for the same reason.
///
/// Asserted through `claim_sync_loops` rather than `start_sync_loops` for the
/// reason above and one more: starting them for real spawns two 20-minute tick
/// loops that reload config and walk the source registry for the rest of the
/// test binary's life.
///
/// This also pins that the two services claim *independent* cells, without a
/// fourth test that would have to assume an execution order. The workspace here
/// differs from the queue pool's, so a single shared cell would make whichever
/// of these two tests ran second read `Foreign` where it expects `Start`.
///
/// The `Foreign` outcome is what a second module setup in one process would hit.
/// `claim_process_setup` already refuses that, so this is a second guard on a
/// case the first one covers — kept because the cost is one `OnceLock` and the
/// failure it guards is a store that silently never syncs.
#[test]
fn the_sync_loops_are_claimed_once_and_a_foreign_workspace_is_refused() {
    let workspace = std::path::Path::new("/tinymemory-module/sync-loops-claim");
    let elsewhere = std::path::Path::new("/tinymemory-module/sync-loops-elsewhere");

    assert_eq!(
        crate::claim_sync_loops(workspace),
        crate::WorkspaceClaim::Start,
        "the first claim must be the one that starts the loops"
    );
    assert_eq!(
        crate::claim_sync_loops(workspace),
        crate::WorkspaceClaim::AlreadyRunning,
        "a second claim for the same workspace must not start a second pair"
    );
    assert_eq!(
        crate::claim_sync_loops(elsewhere),
        crate::WorkspaceClaim::Foreign,
        "a claim for another workspace must be named, not silently swallowed — \
         both loops guard themselves process-wide and that store would never sync"
    );
}

/// A second store opens normally, and needs no pool of its own to do it.
///
/// The pairing with the test above is the point. `queue::start` is guarded by a
/// process-global `Once`, so the obvious failure of moving the pool into the
/// module is a second store silently getting no worker at all. It cannot happen
/// here: the engine's queue is rooted at the workspace — `queue::store` resolves
/// its database through `engine_config`, which is `memory_config_from(config,
/// config.workspace_dir())` — while `memory_subdir` reaches only
/// `UnifiedMemory::new_with_memory_dir`. Both stores below therefore share the
/// one queue `setup` started a pool for.
#[tokio::test]
async fn a_second_store_opens_under_the_one_workspace_queue() {
    use std::sync::Arc;

    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let config = test_config(workspace.path());
    let _embedding_host = EmbeddingHostRestore::install(connection.clone(), &config);
    let opener = test_opener(connection, config);
    let service = super::MemoryService::root(test_provider(), Arc::clone(&opener));

    let first = service
        .open_store("profile-one".to_string())
        .await
        .expect("the first store must open");
    let second = service
        .open_store("profile-two".to_string())
        .await
        .expect("a second store must open rather than panic or be refused");

    assert_ne!(first, second, "each subtree gets its own object path");
    assert_eq!(opener.served.lock().await.len(), 2);
}

/// Every method the service implements must also be declared in the manifest.
///
/// The manifest's `methods` list is admission surface: the host may only call a
/// member the artifact declared, so an implemented-but-undeclared method is
/// simply unreachable — no error, no warning, just a family that is silently
/// missing from the bus.
///
/// This is not hypothetical. Thirty-one methods sat in exactly that state: the
/// whole of People, Chunks, Retrieval and Profile, plus `RecallDocuments`,
/// which predates them. The E2E `the_manifest_declares_every_method_the_module
/// _serves` did not catch it, and could not — it compares the manifest against
/// a hand-written list, so a method missing from *both* is invisible to it, and
/// it is `#[ignore]`d besides because it needs a real dlopen'ed artifact.
///
/// Comparing against the implementation removes the hand-written list from the
/// loop entirely: `members()` is generated by `#[interface]` from the `impl`
/// block itself, so it cannot drift from what is really served. The manifest is
/// read out of `lib.rs` because the macro consumes those literals and offers no
/// constant to inspect.
#[test]
fn every_served_method_is_declared_in_the_manifest() {
    let source = include_str!("../lib.rs");
    let list = source
        .split_once("methods = [")
        .expect("the module_export! block declares methods")
        .1
        .split_once(']')
        .expect("the methods list is closed")
        .0;
    let declared: std::collections::BTreeSet<&str> = list
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // Skip the group comments; only quoted names count.
            line.strip_prefix('"')?
                .split_once('"')
                .map(|(name, _)| name)
        })
        .collect();

    let service = super::MemoryService::new(std::sync::Arc::new(
        tinymemory_api::null::NullMemoryProvider,
    ));
    let served: std::collections::BTreeSet<String> = tinybus::service::Interface::members(&service)
        .iter()
        .map(|member| member.as_str().to_string())
        .collect();
    let served: std::collections::BTreeSet<&str> = served.iter().map(String::as_str).collect();

    let undeclared: Vec<_> = served.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "these methods are served but not declared in the manifest, so no host can call them: \
         {undeclared:?}"
    );

    // The converse is a different failure — a host admitted for a method that
    // answers `unknown_method` — so it is worth pinning in the same place.
    let unserved: Vec<_> = declared.difference(&served).collect();
    assert!(
        unserved.is_empty(),
        "these methods are declared in the manifest but not served: {unserved:?}"
    );
}

/// The members served here are exactly the ones `tinymemory-bus` publishes, in
/// the same order.
///
/// `tinymemory-bus` is what a host compiles against: it carries one constant
/// and one typed call struct per member. Nothing links the two — this crate
/// derives its members from the `#[tinybus::interface]` block, that one lists
/// them by hand — so a method added here without a matching entry there is a
/// capability no host can reach, and an entry there with no method here is a
/// call that fails at runtime with `UnknownMethod`.
///
/// Neither failure has a compile error anywhere, which is why it is asserted.
/// The comparison is on sequences rather than sets on purpose: `members()`
/// returns declaration order, `METHODS` is written in declaration order, and
/// pinning the order too means the two lists stay readable side by side.
#[test]
fn the_served_members_are_exactly_the_published_contract() {
    let service = super::MemoryService::new(std::sync::Arc::new(
        tinymemory_api::null::NullMemoryProvider,
    ));
    let served: Vec<String> = tinybus::service::Interface::members(&service)
        .iter()
        .map(|member| member.as_str().to_string())
        .collect();
    let published: Vec<String> = tinymemory_bus::METHODS
        .iter()
        .map(|member| (*member).to_string())
        .collect();

    // Reported as differences rather than as a 109-element inequality, so the
    // failure names the method that moved instead of printing both lists.
    let missing: Vec<&String> = served.iter().filter(|m| !published.contains(m)).collect();
    assert!(
        missing.is_empty(),
        "served here but absent from tinymemory-bus, so no host can call them: {missing:?}"
    );
    let extra: Vec<&String> = published.iter().filter(|m| !served.contains(m)).collect();
    assert!(
        extra.is_empty(),
        "published by tinymemory-bus but not served here, so a host calling them gets \
         UnknownMethod: {extra:?}"
    );
    assert_eq!(
        served, published,
        "the two lists hold the same members in different orders"
    );
}

#[tokio::test]
async fn the_two_new_families_are_gated_on_their_own_capability() {
    // `test_provider` wraps a bare `Memory` backend through the mandatory
    // composition, so it advertises Core/Recall/Portability and nothing else.
    // The gate has to be per family: a method reached on a driver that does not
    // serve its family must refuse by name, not fall through to whatever the
    // trait's default body happens to return.
    let service = super::MemoryService::new(test_provider());

    let refusal = |error: BusError| match error {
        BusError::MethodFailed { name, .. } => name,
        other => panic!("expected a named MethodFailed, got {other:?}"),
    };

    let error = service
        .run_connection_sync("gmail".to_string(), "conn-1".to_string())
        .await
        .expect_err("a driver without the source-sync family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .bootstrap_connection("gmail".to_string(), "conn-1".to_string())
        .await
        .expect_err("a driver without the source-sync family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .coding_session_status()
        .await
        .expect_err("a driver without the coding-sessions family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    // The two members added to *existing* families refuse through their own
    // family's gate — Tree and Maintenance — rather than through a new one.
    let error = service
        .flush_source_tree("gmail:conn-1".to_string())
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .diagnose()
        .await
        .expect_err("a driver without the maintenance family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);
}

#[tokio::test]
async fn the_runtime_tree_doors_refuse_through_the_tree_gate() {
    // The seven members of the shed's second round, reached on a driver that
    // does not serve their family: `test_provider` advertises Core/Recall/
    // Portability and nothing else, so every one of them must refuse.
    //
    // What this pins is narrower than the family wiring, and worth stating so
    // the next reader does not credit it with more: a door gated on the *wrong*
    // family cannot be caught here, because `require_family!` names the
    // accessor, and an accessor whose trait lacks the method is a compile
    // error rather than a test failure. What it does catch is the shape of the
    // refusal — a member that answers `Ok` with a default instead of refusing,
    // one that panics or hangs on a family it cannot serve, and one whose error
    // leaves under a wire name other than the contract's `UNSUPPORTED`. Each of
    // those is a live-at-runtime bug with no compile error anywhere, which is
    // the same reason the round before this one asserted it.
    let service = super::MemoryService::new(test_provider());

    let refusal = |error: BusError| match error {
        BusError::MethodFailed { name, .. } => name,
        other => panic!("expected a named MethodFailed, got {other:?}"),
    };

    let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp");

    let error = service
        .runtime_buffer_write("team".to_string(), "standup".to_string(), at, None)
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .runtime_read_node("team".to_string(), "root".to_string())
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .runtime_read_children("team".to_string(), "root".to_string())
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .runtime_tree_status("team".to_string())
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .runtime_summarize("team".to_string(), at)
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .runtime_rebuild("team".to_string())
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);

    let error = service
        .flavour_profile("persona/communication".to_string())
        .await
        .expect_err("a driver without the tree family must refuse");
    assert_eq!(refusal(error), wire::UNSUPPORTED);
}

#[tokio::test]
async fn the_runtime_tree_doors_carry_the_engine_answers_back_through_the_port() {
    // The other half of the pair above: the same seven members on a driver that
    // *does* serve Tree, so the delegation past the gate is what runs. The
    // conformance suite pins these shapes at the engine; what is pinned here is
    // that this port carries them out unchanged — an absent node still arrives
    // as `None` and not as a refusal, a fresh namespace still has a status, and
    // a buffered write still answers the path it landed at.
    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let config = test_config(workspace.path());
    // Opening the store requires the process-global host to be installed, even
    // though nothing below embeds anything: the guard is what makes that safe
    // to do from a test, and it restores the previous host on the way out.
    let _embedding_host = EmbeddingHostRestore::install(connection, &config);
    let client = std::sync::Arc::new(
        tinymemory_core::store::MemoryClient::from_workspace_dir(workspace.path().to_path_buf())
            .expect("open the workspace store"),
    );
    let service = super::MemoryService::new(std::sync::Arc::new(crate::provider::provider(
        &config, client,
    )));

    let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp");

    // Absence is data, not a refusal — the distinction the host's RPC surface
    // depends on, and the one a gate-only test cannot see.
    assert!(service
        .runtime_read_node("team".to_string(), "root".to_string())
        .await
        .expect("an absent node is not an error")
        .is_none());
    assert!(service
        .runtime_read_children("team".to_string(), "root".to_string())
        .await
        .expect("an absent parent has no children")
        .is_empty());

    let status = service
        .runtime_tree_status("team".to_string())
        .await
        .expect("a namespace with no tree still has a status");
    assert_eq!(status.namespace, "team");
    assert_eq!(status.total_nodes, 0);
    assert_eq!(status.last_run_at, None);

    // Nothing has been distilled, so the profile is not built — `None` rather
    // than an empty string, which is what stops a caller handing a model a
    // blank persona.
    assert_eq!(
        service
            .flavour_profile("persona/communication".to_string())
            .await
            .expect("an unbuilt profile is not an error"),
        None
    );

    // The write answers a path that names a real file inside the workspace the
    // module was given, which is the reply the host reports verbatim.
    let path = service
        .runtime_buffer_write("team".to_string(), "standup".to_string(), at, None)
        .await
        .expect("a buffered write answers its landing path");
    let landed = std::path::Path::new(&path);
    assert!(landed.is_file(), "the reported path names a real file");
    assert!(
        landed.starts_with(workspace.path()),
        "the buffer file lands inside the module's own workspace"
    );

    // A bad namespace is refused by the engine and leaves under the contract's
    // name for it, not the family gate's.
    let error = service
        .runtime_buffer_write("../escape".to_string(), "x".to_string(), at, None)
        .await
        .expect_err("a traversal namespace is refused");
    assert_eq!(
        match error {
            BusError::MethodFailed { name, .. } => name,
            other => panic!("expected a named MethodFailed, got {other:?}"),
        },
        wire::INVALID
    );

    // No summariser is configured here, so both provider-backed passes must
    // fail rather than report a run that never happened.
    service
        .runtime_summarize("team".to_string(), at)
        .await
        .expect_err("an unresolvable summariser is a failure, not an empty pass");
    service
        .runtime_rebuild("team".to_string())
        .await
        .expect_err("an unresolvable summariser fails a rebuild");

    // The budget check is actually wired, not merely present as a helper.
    // Seeded through the engine's own writer so the node comes back out of a
    // real read: a summary well inside the hour budget, and a metadata blob
    // over the response ceiling — the shape a drained hour with a large
    // pending-fold receipt produces. Deleting the `ensure_response_fits` call
    // from `runtime_read_node` makes this fail, which is the point of asserting
    // it here rather than only against the helper.
    let engine_config =
        tinymemory_tinycortex::engine::EngineRuntimeConfig::from(&test_config(workspace.path()));
    tinymemory_core::tree::tree_runtime::store::write_node(
        &engine_config,
        &tinymemory_api::tree::TreeNode {
            node_id: "2024/03/15/09".to_string(),
            namespace: "team".to_string(),
            level: NodeLevel::Hour,
            parent_id: Some("2024/03/15".to_string()),
            summary: "a summary comfortably inside the hour budget".to_string(),
            token_count: 9,
            child_count: 0,
            created_at: at,
            updated_at: at,
            metadata: Some("m".repeat(super::MAX_RESPONSE_BYTES)),
        },
    )
    .expect("seed an oversized node");

    let error = service
        .runtime_read_node("team".to_string(), "2024/03/15/09".to_string())
        .await
        .expect_err("a node over the response ceiling must be refused, not encoded");
    assert_eq!(
        match error {
            BusError::MethodFailed { name, .. } => name,
            other => panic!("expected a named MethodFailed, got {other:?}"),
        },
        wire::BUDGET_EXCEEDED
    );
}

#[test]
fn a_tree_node_within_its_level_budget_can_still_overrun_the_response_ceiling() {
    // The reason `RuntimeReadNode`/`RuntimeReadChildren`/`RuntimeSummarize`
    // are size-checked at all, pinned as a fact rather than left to prose.
    //
    // A level's `max_tokens` bounds the node's *summary* — `token_count` is
    // documented as the count of `summary`, and the fold passes
    // `NodeLevel::max_tokens` to the summariser for the body alone. It says
    // nothing about `metadata`, which the engine fills with a serialized
    // pending-fold receipt naming every buffer file the pass drained. That
    // list grows with how much was buffered into the hour, not with any
    // level's budget.
    //
    // So a node can sit comfortably inside the hour budget and still be too
    // large to cross a frame. Constructed here rather than driven through the
    // engine because the point is the *shape* being possible: reaching it via
    // a real fold would mean buffering megabytes of entries, which is slow and
    // would pass for the wrong reason if the receipt format ever changed.
    let summary = "x".repeat(NodeLevel::Hour.max_tokens() as usize);
    assert!(
        summary.len() < super::MAX_RESPONSE_BYTES,
        "the summary alone must be nowhere near the ceiling, or this proves nothing"
    );

    let node = TreeNode {
        node_id: "2024/03/15/09".to_string(),
        namespace: "team".to_string(),
        level: NodeLevel::Hour,
        parent_id: Some("2024/03/15".to_string()),
        summary,
        token_count: NodeLevel::Hour.max_tokens(),
        child_count: 0,
        created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp"),
        updated_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp"),
        metadata: Some("m".repeat(super::MAX_RESPONSE_BYTES)),
    };

    let error = super::ensure_response_fits(&Some(node), "RuntimeReadNode")
        .expect_err("a node whose metadata overruns the ceiling must be refused");
    match error {
        BusError::MethodFailed { name, message } => {
            assert_eq!(name, wire::BUDGET_EXCEEDED);
            assert!(message.contains("RuntimeReadNode"), "{message}");
        }
        other => panic!("expected MethodFailed, got {other:?}"),
    }
}

#[test]
fn an_ordinary_tree_node_read_is_not_refused() {
    // The other side of the ceiling: the check must not fire on the shape the
    // host actually reads back, or every tree read becomes a budget error.
    let node = TreeNode {
        node_id: "2024/03/15/09".to_string(),
        namespace: "team".to_string(),
        level: NodeLevel::Hour,
        parent_id: Some("2024/03/15".to_string()),
        summary: "the morning standup, folded".to_string(),
        token_count: 7,
        child_count: 0,
        created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp"),
        updated_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp"),
        metadata: Some(r#"{"buffer_filenames":["1700000000000_a.md"]}"#.to_string()),
    };

    assert!(super::ensure_response_fits(&Some(node.clone()), "RuntimeReadNode").is_ok());
    // A full calendar month of children is the realistic worst case for the
    // child read, and it must pass.
    let children: Vec<TreeNode> = (0..31).map(|_| node.clone()).collect();
    assert!(super::ensure_response_fits(&children, "RuntimeReadChildren").is_ok());
}

#[tokio::test]
async fn override_member_opens_a_window_that_outranks_a_paused_gate() {
    use tinymemory_core::scheduler_gate::{self as gate, PauseReason, Policy};

    // A paused gate stands in for "the host said mode = off".
    #[derive(Debug)]
    struct PausedGate;
    #[async_trait::async_trait]
    impl gate::SchedulerGate for PausedGate {
        fn current_policy(&self) -> Policy {
            Policy::Paused {
                reason: PauseReason::UserDisabled,
            }
        }
        fn resume_notify(&self) -> std::sync::Arc<tokio::sync::Notify> {
            std::sync::Arc::new(tokio::sync::Notify::new())
        }
        async fn wait_for_capacity(&self) -> Option<Box<dyn Send>> {
            None
        }
    }

    let _seams = crate::seam_lock::hold_global_seams_async().await;
    gate::clear_manual_override();
    gate::set_scheduler_gate(std::sync::Arc::new(PausedGate));
    assert!(matches!(gate::current_policy(), Policy::Paused { .. }));

    // The member is the host's "process now" lever: through the service impl,
    // exactly as a bus dispatch would reach it, the window opens and
    // user-requested work outranks the pause -- clamped, so an absurd ask is
    // an hour, not forever.
    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let config = test_config(workspace.path());
    let opener = test_opener(connection, config);
    let service = super::MemoryService::root(test_provider(), std::sync::Arc::clone(&opener));
    service
        .override_scheduler_gate(7 * 24 * 3600)
        .await
        .expect("override member answers");
    assert_eq!(gate::current_policy(), Policy::Normal);

    gate::clear_manual_override();
    assert!(matches!(gate::current_policy(), Policy::Paused { .. }));
    gate::clear_scheduler_gate();
}

/// openhuman#6012: the backfill reaches core *through this port*, with a real
/// store underneath — not only in `tinymemory-core`'s own suite.
///
/// Worth having here for two independent reasons. The module workspace runs
/// only its own tests, so core's backfill suite never executes in this lane
/// while the coverage gate still measures core's production source. And more to
/// the point: nothing else exercises service → provider → core for this member,
/// which is the path a host actually calls.
///
/// The document is written straight through the store client, deliberately.
/// Storing it with `accept_source_items` would tree it on the way in — that is
/// what #134 fixed — and then there would be nothing left for a backfill to do.
/// A document in the namespace store with no tree row *is* the state this
/// feature exists to repair.
#[tokio::test]
async fn the_backfill_door_files_a_stored_connector_document_through_the_port() {
    use tinymemory_api::provider::types::BackfillTreesRequest;

    let workspace = tempfile::tempdir().expect("tempdir");
    let connection = test_connection().await;
    let mut config = test_config(workspace.path());
    // The source registry is written beside the host's config file, so the
    // default's absent path has to be given a real one or the walk finds no
    // namespaces to sweep.
    config.config_path = Some(workspace.path().join("config.toml"));
    let _embedding_host = EmbeddingHostRestore::install(connection, &config);

    let client = std::sync::Arc::new(
        tinymemory_core::store::MemoryClient::from_workspace_dir(workspace.path().to_path_buf())
            .expect("open the workspace store"),
    );

    // One connected account, so the namespace is derivable and the legacy
    // `skill-` one is unambiguous rather than skipped.
    let host = tinymemory_tinycortex::engine::EngineRuntimeConfig::from(&config);
    let source: tinymemory_core::sources::MemorySourceEntry =
        serde_json::from_value(serde_json::json!({
            "id": "src_gmail",
            "kind": "composio",
            "label": "Gmail",
            "enabled": true,
            "toolkit": "gmail",
            "connection_id": "conn-1",
        }))
        .expect("a valid composio source entry");
    tinymemory_core::sources::registry::replace_sources_in(&host, &[source])
        .expect("write the source registry");

    client
        .put_doc(tinymemory_api::types::NamespaceDocumentInput {
            namespace: "source:gmail:conn-1".to_string(),
            key: "msg-1".to_string(),
            title: "Quarterly planning".into(),
            content: "Let's finalise the Q3 roadmap and align on the launch date.".into(),
            source_type: "composio".into(),
            priority: "medium".into(),
            tags: vec!["gmail".into()],
            metadata: serde_json::json!({}),
            category: "core".into(),
            session_id: None,
            document_id: None,
            taint: tinymemory_api::types::MemoryTaint::ExternalSync,
        })
        .await
        .expect("store a connector document the way the sync path stored one");

    let service = super::MemoryService::new(std::sync::Arc::new(crate::provider::provider(
        &config,
        std::sync::Arc::clone(&client),
    )));

    let report = service
        .backfill_connector_trees(BackfillTreesRequest {
            limit: None,
            dry_run: false,
        })
        .await
        .expect("the backfill door answers");

    assert_eq!(
        report.ingested, 1,
        "the stored document must reach the tree through this port: {report:?}"
    );
    assert_eq!(
        report.already_present, 0,
        "nothing was treed before this ran: {report:?}"
    );

    // Idempotence, asserted here as well as in core, because it is the property
    // that makes this safe for a host to offer as a button someone can press
    // twice.
    let again = service
        .backfill_connector_trees(BackfillTreesRequest {
            limit: None,
            dry_run: false,
        })
        .await
        .expect("a second pass answers");
    assert_eq!(
        again.ingested, 0,
        "a second pass must write nothing: {again:?}"
    );
    assert_eq!(
        again.already_present, 1,
        "and must say why it wrote nothing: {again:?}"
    );
}
