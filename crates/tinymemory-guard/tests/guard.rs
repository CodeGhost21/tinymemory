//! The decorator's behaviour over a scripted [`GuardPolicy`], driven against
//! the conformance crate's recording driver.
//!
//! The policy here is a plain struct whose knobs stand in for what a host
//! resolves at runtime (tier, ambient scope, redaction, budgets), so every step
//! of the enforcement chain is exercised without a host.

// A panic in a test IS the failure report.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use tinymemory_api::capabilities::{Capabilities, Capability};
use tinymemory_api::error::MemoryError;
use tinymemory_api::null::NullMemoryProvider;
use tinymemory_api::provider::types::SourceScope;
use tinymemory_api::provider::{
    audit_provider, MemoryCore, MemoryPortability, MemoryProvider, MemoryRecall, MemoryTree,
};
use tinymemory_api::recall::OwnedRecallOpts;
use tinymemory_api::types::{MemoryCategory, MemoryEntry, MemoryTaint, NamespaceDocumentInput};
use tinymemory_conformance::RecordingProvider;
use tinymemory_guard::{GuardPolicy, GuardedProvider, GUARD_DENIED_PREFIX};

/// A policy whose answers are fields.
struct TestPolicy {
    driver_id: &'static str,
    ambient: Option<Vec<String>>,
    deny_writes: bool,
    /// Replaces `SECRET` with `[REDACTED]`, standing in for an external
    /// driver's scrubber.
    redact: bool,
    recall_budget: Option<usize>,
    capture_budget: Option<usize>,
    denials: Mutex<Vec<(String, String)>>,
    allowed: Mutex<Vec<(String, String, usize)>>,
}

impl TestPolicy {
    fn new() -> Self {
        Self {
            driver_id: "recording",
            ambient: None,
            deny_writes: false,
            redact: false,
            recall_budget: None,
            capture_budget: None,
            denials: Mutex::new(Vec::new()),
            allowed: Mutex::new(Vec::new()),
        }
    }

    fn scoped(mut self, allow: &[&str]) -> Self {
        self.ambient = Some(allow.iter().map(|s| (*s).to_string()).collect());
        self
    }

    fn readonly(mut self) -> Self {
        self.deny_writes = true;
        self
    }

    fn redacting(mut self) -> Self {
        self.redact = true;
        self
    }

    fn budgets(mut self, recall: usize, capture: usize) -> Self {
        self.recall_budget = Some(recall);
        self.capture_budget = Some(capture);
        self
    }
}

impl GuardPolicy for TestPolicy {
    fn driver_id(&self) -> &str {
        self.driver_id
    }

    fn enforce_read(&self, _operation: &str) -> Result<(), MemoryError> {
        Ok(())
    }

    fn enforce_write(&self, operation: &str) -> Result<(), MemoryError> {
        if self.deny_writes {
            return Err(self.denied(operation, "read-only tier"));
        }
        Ok(())
    }

    fn check_egress(&self, _method: &str, _carries_content: bool) -> Result<(), MemoryError> {
        Ok(())
    }

    fn ambient_scope(&self) -> Option<SourceScope> {
        self.ambient.clone().map(SourceScope::new)
    }

    fn redact_outbound<'a>(&self, content: &'a str) -> Cow<'a, str> {
        if self.redact {
            Cow::Owned(content.replace("SECRET", "[REDACTED]"))
        } else {
            Cow::Borrowed(content)
        }
    }

    fn redact_outbound_json(&self, value: serde_json::Value) -> serde_json::Value {
        value
    }

    fn recall_budget(&self) -> Option<usize> {
        self.recall_budget
    }

    fn capture_budget(&self) -> Option<usize> {
        self.capture_budget
    }

    fn on_denied(&self, method: &str, reason: &str) {
        self.denials
            .lock()
            .unwrap()
            .push((method.to_string(), reason.to_string()));
    }

    fn on_allowed(&self, method: &str, namespace: &str, chars: usize) {
        self.allowed
            .lock()
            .unwrap()
            .push((method.to_string(), namespace.to_string(), chars));
    }
}

fn guarded(
    policy: TestPolicy,
) -> (
    Arc<RecordingProvider>,
    Arc<TestPolicy>,
    GuardedProvider<TestPolicy>,
) {
    guarded_with(RecordingProvider::new(), policy)
}

fn guarded_with(
    driver: RecordingProvider,
    policy: TestPolicy,
) -> (
    Arc<RecordingProvider>,
    Arc<TestPolicy>,
    GuardedProvider<TestPolicy>,
) {
    let driver = Arc::new(driver);
    let policy = Arc::new(policy);
    let guard = GuardedProvider::new(
        Arc::clone(&driver) as Arc<dyn MemoryProvider>,
        Arc::clone(&policy),
    );
    (driver, policy, guard)
}

fn entry(content: &str) -> MemoryEntry {
    MemoryEntry {
        id: "id".into(),
        key: "key".into(),
        content: content.into(),
        namespace: Some("ns".into()),
        category: MemoryCategory::Core,
        timestamp: "2026-01-01T00:00:00Z".into(),
        session_id: None,
        score: None,
        taint: MemoryTaint::Internal,
    }
}

fn document(content: &str, taint: MemoryTaint) -> NamespaceDocumentInput {
    NamespaceDocumentInput {
        namespace: "ns".into(),
        key: "k".into(),
        title: "t".into(),
        content: content.into(),
        source_type: "chat".into(),
        priority: "normal".into(),
        tags: vec![],
        metadata: serde_json::Value::Null,
        category: "core".into(),
        session_id: None,
        document_id: None,
        taint,
    }
}

async fn store(guard: &GuardedProvider<TestPolicy>, content: &str, taint: MemoryTaint) {
    guard
        .store("ns", "k", content, MemoryCategory::Core, None, taint)
        .await
        .expect("store");
}

// ── Identity + capability mirroring ─────────────────────────────────────────

#[tokio::test]
async fn guard_reports_the_wrapped_drivers_identity() {
    let (_driver, _policy, guard) = guarded(TestPolicy::new());
    assert_eq!(
        guard.driver_id(),
        "recording",
        "the guard is a policy layer, not a driver"
    );
    assert_eq!(guard.capabilities(), Capabilities::all());
}

#[tokio::test]
async fn guard_passes_audit_provider_against_its_own_capabilities() {
    let (_driver, _policy, guard) = guarded(TestPolicy::new());
    audit_provider(&guard).expect("advertised set and reachable accessors must agree");
}

#[tokio::test]
async fn guard_accessor_presence_mirrors_inner_provides_for_every_family() {
    let (_driver, _policy, guard) = guarded(TestPolicy::new());
    for capability in Capability::ALL {
        assert!(
            guard.provides(capability),
            "{capability} must be reachable through the guard"
        );
    }

    // The other direction: a driver with only the mandatory three must not
    // acquire families merely by being guarded.
    let inner = Arc::new(NullMemoryProvider::new());
    let null = GuardedProvider::new(
        Arc::clone(&inner) as Arc<dyn MemoryProvider>,
        Arc::new(TestPolicy::new()),
    );
    for capability in Capability::ALL {
        assert_eq!(
            null.provides(capability),
            inner.provides(capability),
            "{capability} presence must mirror the inner driver exactly"
        );
    }
    audit_provider(&null).expect("mandatory-only driver stays consistent when guarded");
}

// ── The wrapped-accessor property ───────────────────────────────────────────

#[tokio::test]
async fn guard_as_tree_is_not_the_raw_driver_handle() {
    let (driver, _policy, guard) = guarded(TestPolicy::new());
    let via_guard = guard.as_tree().expect("tree family") as *const dyn MemoryTree;
    let raw = driver.as_tree().expect("tree family") as *const dyn MemoryTree;
    assert!(
        !std::ptr::eq(via_guard, raw),
        "the accessor handed out the driver's own handle — the guard is bypassable"
    );
}

#[tokio::test]
async fn guard_as_tree_still_applies_policy_reached_through_the_accessor() {
    let (driver, policy, guard) = guarded(TestPolicy::new().readonly());
    let err = guard
        .as_tree()
        .expect("tree family")
        .seal("ns")
        .await
        .expect_err("a read-only tier must refuse a tree write");
    assert!(err.to_string().contains(GUARD_DENIED_PREFIX), "{err}");
    assert_eq!(driver.call_count(), 0, "the driver must not be reached");
    assert_eq!(policy.denials.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn every_optional_family_accessor_enforces_the_write_tier() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().readonly());

    // One representative *write* per optional family. Each must be refused
    // before the driver sees it — a family whose decorator forwarded raw would
    // record a call here.
    guard
        .as_ingest()
        .unwrap()
        .ingest_chat(vec![])
        .await
        .expect_err("ingest");
    guard.as_tree().unwrap().seal("ns").await.expect_err("tree");
    guard
        .as_entities()
        .unwrap()
        .touch_entities("ns", &[])
        .await
        .expect_err("entities");
    guard
        .as_graph()
        .unwrap()
        .kv_put(None, "k", serde_json::Value::Null)
        .await
        .expect_err("graph");
    guard
        .as_diff()
        .unwrap()
        .capture_snapshot("src")
        .await
        .expect_err("diff");
    guard
        .as_goals()
        .unwrap()
        .set_goals(Default::default())
        .await
        .expect_err("goals");
    guard
        .as_tool_memory()
        .unwrap()
        .delete_tool_rule("t", "r")
        .await
        .expect_err("tool_memory");
    guard
        .as_sources()
        .unwrap()
        .forget_source("src")
        .await
        .expect_err("sources");
    guard
        .as_maintenance()
        .unwrap()
        .compact()
        .await
        .expect_err("maintenance");

    assert_eq!(
        driver.call_count(),
        0,
        "at least one family decorator forwarded an unguarded handle: {:?}",
        driver.calls()
    );
}

// ── The mandatory three ─────────────────────────────────────────────────────

#[tokio::test]
async fn guard_stamps_taint_on_store_rather_than_trusting_the_caller() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().scoped(&["slack:#eng"]));
    store(&guard, "hello", MemoryTaint::Internal).await;
    assert_eq!(driver.only_call().taint, Some(MemoryTaint::ExternalSync));
}

#[tokio::test]
async fn guard_never_lowers_a_callers_external_taint() {
    let (driver, _policy, guard) = guarded(TestPolicy::new());
    store(&guard, "hello", MemoryTaint::ExternalSync).await;
    assert_eq!(driver.only_call().taint, Some(MemoryTaint::ExternalSync));
}

#[tokio::test]
async fn guard_leaves_internal_taint_alone_outside_a_scope() {
    let (driver, _policy, guard) = guarded(TestPolicy::new());
    store(&guard, "hello", MemoryTaint::Internal).await;
    assert_eq!(driver.only_call().taint, Some(MemoryTaint::Internal));
}

#[tokio::test]
async fn guard_redacts_stored_content_before_truncating_it() {
    // Redaction lengthens `SECRET` (6) into `[REDACTED]` (10); the budget must
    // apply to what actually leaves.
    let (driver, _policy, guard) = guarded(TestPolicy::new().redacting().budgets(1000, 12));
    store(&guard, "a SECRET b SECRET", MemoryTaint::Internal).await;
    assert_eq!(driver.only_call().content.as_deref(), Some("a [REDACTED]"));
}

#[tokio::test]
async fn guard_truncates_stored_content_to_the_capture_budget() {
    let (driver, policy, guard) = guarded(TestPolicy::new().budgets(1000, 5));
    store(&guard, "hello world", MemoryTaint::Internal).await;
    assert_eq!(driver.only_call().content.as_deref(), Some("hello"));
    assert_eq!(
        policy.allowed.lock().unwrap().as_slice(),
        [("core.store".to_string(), "ns".to_string(), 5)],
        "the trace reports the post-budget char count"
    );
}

#[tokio::test]
async fn guard_truncates_recall_results_to_the_recall_budget() {
    let (_driver, _policy, guard) = guarded_with(
        RecordingProvider::new().with_recall_result(vec![
            entry("aaaa"),
            entry("bbbb"),
            entry("cccc"),
        ]),
        TestPolicy::new().budgets(6, 500),
    );
    let hits = guard
        .recall("q", 10, &OwnedRecallOpts::default(), None)
        .await
        .expect("recall");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].content, "aaaa");
    assert_eq!(hits[1].content, "bb");
}

#[tokio::test]
async fn guard_redacts_the_recall_query() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().redacting());
    let _ = guard
        .recall("find SECRET", 3, &OwnedRecallOpts::default(), None)
        .await;
    assert_eq!(driver.call_count(), 1);
}

/// The driver may *refuse* a `Some(scope)` on recall, so the guard must NOT fill
/// it from the ambient scope.
#[tokio::test]
async fn guard_never_fills_scope_on_recall() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().scoped(&["slack:#eng"]));
    guard
        .recall("q", 10, &OwnedRecallOpts::default(), None)
        .await
        .expect("recall must not become an error merely by being scoped");
    assert_eq!(driver.only_call().scoped, Some(false));
}

#[tokio::test]
async fn guard_forwards_an_explicit_recall_scope_untouched() {
    let (driver, _policy, guard) = guarded(TestPolicy::new());
    let scope = SourceScope::new(["slack:#eng"]);
    let _ = guard
        .recall("q", 10, &OwnedRecallOpts::default(), Some(&scope))
        .await;
    assert_eq!(driver.only_call().scoped, Some(true));
}

#[tokio::test]
async fn guard_preserves_import_taint_rather_than_restamping_it() {
    use tinymemory_api::provider::types::ExportRecord;
    let (driver, _policy, guard) = guarded(TestPolicy::new().scoped(&["slack:#eng"]));
    // Inside a source scope, so a naive "stamp everything" would show up.
    guard
        .import_records(vec![ExportRecord {
            kind: "entry".into(),
            id: "r1".into(),
            namespace: Some("ns".into()),
            taint: MemoryTaint::Internal,
            payload: serde_json::Value::Null,
        }])
        .await
        .expect("import");
    assert_eq!(
        driver.only_call().taint,
        Some(MemoryTaint::Internal),
        "a restore must not have its provenance rewritten wholesale"
    );
}

#[tokio::test]
async fn guard_does_not_budget_trim_an_export() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().budgets(1, 1));
    guard.export_page(None, 10).await.expect("export");
    assert_eq!(driver.only_call().method, "portability.export_page");
}

// ── Refusals ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_refusal_is_prefixed_audited_and_never_reaches_the_driver() {
    let (driver, policy, guard) = guarded(TestPolicy::new().readonly());
    let err = guard
        .store(
            "ns",
            "k",
            "hello",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect_err("read-only tier must refuse a write");
    assert!(
        matches!(&err, MemoryError::Invalid(m) if m == "memory guard: read-only tier"),
        "{err:?}"
    );
    assert_eq!(driver.call_count(), 0, "the driver must never be reached");
    assert_eq!(
        policy.denials.lock().unwrap().as_slice(),
        [("core.store".to_string(), "read-only tier".to_string())]
    );
}

#[tokio::test]
async fn the_success_path_audits_no_denial() {
    let (_driver, policy, guard) = guarded(TestPolicy::new());
    store(&guard, "hello", MemoryTaint::Internal).await;
    guard
        .recall("q", 3, &OwnedRecallOpts::default(), None)
        .await
        .expect("recall");
    assert!(policy.denials.lock().unwrap().is_empty());
}

// ── Step 2 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn query_source_takes_its_scope_from_the_ambient_one() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().scoped(&["slack:#eng"]));
    guard
        .as_tree()
        .unwrap()
        .query_source("ns", "src", 10, None)
        .await
        .expect("query_source");
    let call = driver.only_call();
    assert_eq!(call.scoped, Some(true));
    assert_eq!(call.content.as_deref(), Some("slack:#eng"));
}

#[tokio::test]
async fn an_explicit_scope_is_intersected_with_the_ambient_one() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().scoped(&["slack:#eng"]));
    let explicit = SourceScope::new(["gmail:me"]);
    guard
        .as_tree()
        .unwrap()
        .query_source("ns", "src", 10, Some(&explicit))
        .await
        .expect("query_source");
    assert_eq!(
        driver.only_call().content.as_deref(),
        Some(""),
        "a request outside the ambient allowlist must fail closed"
    );
}

#[tokio::test]
async fn query_source_is_unscoped_when_nothing_restricts_it() {
    let (driver, _policy, guard) = guarded(TestPolicy::new());
    guard
        .as_tree()
        .unwrap()
        .query_source("ns", "src", 10, None)
        .await
        .expect("query_source");
    assert_eq!(driver.only_call().scoped, Some(false));
}

#[test]
fn narrow_scope_keeps_an_explicit_scope_that_the_ambient_one_allows() {
    let policy = TestPolicy::new().scoped(&["slack:#eng", "gmail:me"]);
    let narrowed = policy
        .narrow_scope(Some(&SourceScope::new(["gmail:me"])))
        .expect("scoped");
    assert_eq!(narrowed.allow, vec!["gmail:me".to_string()]);
}

#[test]
fn narrow_scope_without_an_ambient_scope_passes_the_request_through() {
    let policy = TestPolicy::new();
    let requested = SourceScope::new(["gmail:me"]);
    assert_eq!(policy.narrow_scope(Some(&requested)), Some(requested));
    assert_eq!(policy.narrow_scope(None), None);
}

// ── Steps 3 + 4 through a family accessor ───────────────────────────────────

#[tokio::test]
async fn family_writes_are_taint_stamped_too() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().scoped(&["slack:#eng"]));
    guard
        .as_documents()
        .unwrap()
        .put_document(document("body", MemoryTaint::Internal))
        .await
        .expect("put_document");
    assert_eq!(driver.only_call().taint, Some(MemoryTaint::ExternalSync));
}

#[tokio::test]
async fn family_writes_are_redacted_by_the_policy() {
    let (driver, _policy, guard) = guarded(TestPolicy::new().redacting());
    guard
        .as_documents()
        .unwrap()
        .put_document(document("a SECRET", MemoryTaint::Internal))
        .await
        .expect("put_document");
    assert_eq!(driver.only_call().content.as_deref(), Some("a [REDACTED]"));
}

#[tokio::test]
async fn family_writes_pass_through_a_policy_that_does_not_redact() {
    let (driver, _policy, guard) = guarded(TestPolicy::new());
    guard
        .as_documents()
        .unwrap()
        .put_document(document("a SECRET", MemoryTaint::Internal))
        .await
        .expect("put_document");
    assert_eq!(driver.only_call().content.as_deref(), Some("a SECRET"));
}

// ── Episodic portability ────────────────────────────────────────────────────

fn turn(content: &str) -> tinymemory_api::provider::EpisodicTurn {
    tinymemory_api::provider::EpisodicTurn {
        id: Some(1),
        session_id: "s1".into(),
        timestamp: 1.0,
        role: "user".into(),
        content: content.into(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 0,
    }
}

#[tokio::test]
async fn an_episodic_import_is_a_write_and_is_refused_by_a_read_only_policy() {
    use tinymemory_api::provider::{EpisodicPart, EpisodicRecords};
    let (driver, _policy, guard) = guarded(TestPolicy::new().readonly());
    let family = guard
        .as_episodic_portability()
        .expect("the guard serves what the driver serves");
    family
        .import_episodic(EpisodicRecords::Turns(vec![turn("hello")]))
        .await
        .expect_err("a read-only policy refuses an import");
    assert_eq!(driver.call_count(), 0, "{:?}", driver.calls());

    // An export only reads, so the same policy lets it through.
    let page = family
        .export_episodic(EpisodicPart::Turns, None, 10)
        .await
        .expect("export");
    assert!(page.records.is_empty());
    assert_eq!(driver.call_count(), 1);
}

#[tokio::test]
async fn an_episodic_import_is_redacted_like_a_recorded_turn() {
    use tinymemory_api::provider::{EpisodicEvent, EpisodicRecords, EventKind};
    let (driver, _policy, guard) = guarded(TestPolicy::new().redacting());
    let family = guard.as_episodic_portability().unwrap();
    family
        .import_episodic(EpisodicRecords::Turns(vec![turn("my SECRET plan")]))
        .await
        .expect("import turns");
    family
        .import_episodic(EpisodicRecords::Events(vec![EpisodicEvent {
            event_id: "ev-1".into(),
            segment_id: "seg-1".into(),
            session_id: "s1".into(),
            namespace: "global".into(),
            kind: EventKind::Fact,
            content: "the SECRET is out".into(),
            subject: None,
            timestamp_ref: None,
            confidence: 1.0,
            embedding: None,
            source_turn_ids: None,
            created_at: 1.0,
        }]))
        .await
        .expect("import events");
    let contents: Vec<String> = driver
        .calls()
        .into_iter()
        .filter(|call| call.method == "episodic_portability.import_episodic")
        .filter_map(|call| call.content)
        .collect();
    assert_eq!(
        contents,
        vec![
            "my [REDACTED] plan".to_string(),
            "the [REDACTED] is out".to_string()
        ]
    );
}
