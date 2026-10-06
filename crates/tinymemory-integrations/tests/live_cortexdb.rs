//! The CortexDB engines against a real server, on each wire configured:
//!
//! - `cortexdb` (Direct): when `TINYMEMORY_LIVE_CORTEXDB_URL` names a CortexDB
//!   server. The harness in `integration/cortexdb/` boots a pinned one, and
//!   `scripts/cortexdb-live.sh` runs this whole file against it. The key
//!   defaults to the harness's (`TINYMEMORY_TEST_CORTEX_KEY`).
//! - `tinyhumans` (hosted, behind the TinyHumans backend): when
//!   `TINYMEMORY_LIVE_TINYHUMANS_URL` names the backend and
//!   `TINYMEMORY_TEST_TINYHUMANS_TOKEN` holds a session JWT or `tiny_live_` key
//!   for a test account. This writes to that account's hosted memory, which
//!   is billed, and forgets what it wrote.
//!
//! With neither set, every test skips.
//!
//! Two passes: the shared conformance suite, then the three stores the host
//! uses (a document, a conversation with a tool call, a learning) read back
//! through `list`, `fetch` and `recall`, compiled into `context.md`, and
//! forgotten.

// The helpers outside `#[test]` fns fail the test by panicking, like the tests.
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use std::sync::Arc;
use tinymemory_api::{
    FetchMode, FetchRequest, ForgetTarget, ItemKind, LearningKind, ListRequest, MemoryEngine,
    MemoryMeta, MetaFilter, RecallRequest, Role, SourceKind, SourceRef, StoreItem, ToolCallRef,
    Turn,
};

use tinymemory_integrations::cortex::{CortexCredential, CortexEngine, StaticBearer};
use tinymemory_tools::context::{ContextSpec, compile};

const DEFAULT_KEY: &str = "tinymemory-cortex-test";

/// How long a stored item may take to become readable. CortexDB indexes
/// asynchronously, so a write is not visible to the very next read.
const VISIBILITY: Duration = Duration::from_secs(60);

/// Every live engine the environment configures, labelled by wire.
fn live_engines() -> Vec<(&'static str, CortexEngine)> {
    let mut engines = Vec::new();
    if let Ok(url) = std::env::var("TINYMEMORY_LIVE_CORTEXDB_URL") {
        let key =
            std::env::var("TINYMEMORY_TEST_CORTEX_KEY").unwrap_or_else(|_| DEFAULT_KEY.into());
        engines.push((
            "cortexdb",
            CortexEngine::direct(&url, CortexCredential::api_key(key))
                .expect("a valid live CortexDB endpoint"),
        ));
    }
    if let Ok(url) = std::env::var("TINYMEMORY_LIVE_TINYHUMANS_URL") {
        let token = std::env::var("TINYMEMORY_TEST_TINYHUMANS_TOKEN")
            .expect("TINYMEMORY_LIVE_TINYHUMANS_URL needs TINYMEMORY_TEST_TINYHUMANS_TOKEN");
        engines.push((
            "tinyhumans",
            CortexEngine::tinyhumans(&url, Arc::new(StaticBearer::new(token)))
                .expect("a valid live TinyHumans endpoint"),
        ));
    }
    if engines.is_empty() {
        eprintln!(
            "neither TINYMEMORY_LIVE_CORTEXDB_URL nor TINYMEMORY_LIVE_TINYHUMANS_URL set; skipping"
        );
    }
    engines
}

fn run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    format!("live-{nanos}")
}

fn meta(workspace: &str, source: SourceKind) -> MemoryMeta {
    MemoryMeta {
        workspace: Some(workspace.to_string()),
        source: SourceRef {
            kind: source,
            id: Some(format!("{workspace}-{}", source.as_str())),
        },
        ..MemoryMeta::default()
    }
}

/// Lists `filter` until it holds `want` items or [`VISIBILITY`] runs out.
async fn list_until(engine: &CortexEngine, filter: &MetaFilter, want: usize) -> Vec<String> {
    let deadline = Instant::now() + VISIBILITY;
    loop {
        let page = engine
            .list(ListRequest::new(filter.clone(), 50))
            .await
            .expect("list");
        let texts: Vec<String> = page.items.into_iter().map(|hit| hit.text).collect();
        if texts.len() >= want || Instant::now() >= deadline {
            return texts;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Held by each live test for its whole run, so they reach the server one
/// at a time. Run together, one test's forgets drop the packs another is
/// about to answer from, and load the server enough that forgetting a long
/// document (seconds per ~240 KiB event on 0.10.4) outlasts the request
/// timeout.
static ONE_AT_A_TIME: futures::lock::Mutex<()> = futures::lock::Mutex::new(());

#[tokio::test]
async fn the_live_server_upholds_the_contract() {
    let _alone = ONE_AT_A_TIME.lock().await;
    for (wire, engine) in live_engines() {
        eprintln!("conformance on {wire}");
        tinymemory_api::conformance::run(&engine)
            .await
            .unwrap_or_else(|error| panic!("the live {wire} engine conforms: {error}"));
    }
}

#[tokio::test]
async fn documents_conversations_and_learnings_round_trip_into_context() {
    let _alone = ONE_AT_A_TIME.lock().await;
    for (wire, engine) in live_engines() {
        eprintln!("round trip on {wire}");
        round_trip(&engine).await;
    }
}

async fn round_trip(engine: &CortexEngine) {
    assert!(engine.health().await.is_serving(), "the server is serving");
    let workspace = run_id();

    // Document: a file read from a folder source.
    let mut doc_meta = meta(&workspace, SourceKind::Folder);
    doc_meta.folder = Some("/notes".into());
    doc_meta.file_path = Some("/notes/aurora.md".into());
    doc_meta.language = Some("en".into());
    let document = engine
        .store(StoreItem::Document {
            title: Some("Aurora plan".into()),
            body: tinymemory_api::DocumentBody::Text(
                "Project Aurora launches on Thursday from the Lisbon office.".into(),
            ),
            mime: Some("text/markdown".into()),
            meta: doc_meta,
        })
        .await
        .expect("store document");

    // Conversation: two turns, one of which called a tool.
    let mut conv_meta = meta(&workspace, SourceKind::Conversation);
    conv_meta.thread_id = Some(format!("{workspace}-thread"));
    conv_meta.agent_id = Some("orchestrator".into());
    let mut answer = Turn::new(Role::Assistant, "Booked the Lisbon venue for Thursday.");
    answer.tool_calls.push(ToolCallRef {
        name: "calendar_create".into(),
        id: Some("call-1".into()),
    });
    let conversation = engine
        .store(StoreItem::Conversation {
            turns: vec![
                Turn::new(Role::User, "Book a venue for the Aurora launch."),
                answer,
            ],
            meta: conv_meta,
        })
        .await
        .expect("store conversation");

    // Learning: explicit, from the agent, attributed to its tool call.
    let mut learn_meta = meta(&workspace, SourceKind::Agent);
    learn_meta.tool_call = Some(ToolCallRef {
        name: "memory".into(),
        id: Some("call-2".into()),
    });
    let learning = engine
        .store(StoreItem::learning(
            "The user prefers launch events in Lisbon.",
            LearningKind::Preference,
            0.9,
            learn_meta,
        ))
        .await
        .expect("store learning");

    let by_kind = |kind: ItemKind| MetaFilter {
        workspace: Some(workspace.clone()),
        kinds: vec![kind],
        ..MetaFilter::default()
    };
    let docs = list_until(engine, &by_kind(ItemKind::Document), 1).await;
    assert_eq!(docs.len(), 1, "the document lists back: {docs:?}");
    assert!(docs[0].contains("Project Aurora"));
    let convs = list_until(engine, &by_kind(ItemKind::Conversation), 1).await;
    assert_eq!(convs.len(), 1, "the conversation lists back: {convs:?}");
    assert!(
        convs[0].contains("calendar_create (call-1)"),
        "the tool call stays visible: {}",
        convs[0]
    );
    let learns = list_until(engine, &by_kind(ItemKind::Learning), 1).await;
    assert_eq!(learns.len(), 1, "the learning lists back: {learns:?}");

    // Metadata filters narrow server-side and client-side alike.
    let by_file = MetaFilter {
        workspace: Some(workspace.clone()),
        file_path: Some("/notes/aurora.md".into()),
        ..MetaFilter::default()
    };
    assert_eq!(list_until(engine, &by_file, 1).await.len(), 1);
    let by_tool = MetaFilter {
        workspace: Some(workspace.clone()),
        tool_call: Some("memory".into()),
        ..MetaFilter::default()
    };
    assert_eq!(list_until(engine, &by_tool, 1).await.len(), 1);

    // Fetch: hybrid search over the run's items finds the document.
    let mut fetch = FetchRequest::new("When does Project Aurora launch?", FetchMode::Hybrid, 10);
    fetch.filter = MetaFilter {
        workspace: Some(workspace.clone()),
        ..MetaFilter::default()
    };
    let page = engine.fetch(fetch).await.expect("fetch");
    assert!(
        page.hits.iter().any(|hit| hit.id == document.id),
        "fetch finds the document: {:?}",
        page.hits.iter().map(|hit| &hit.text).collect::<Vec<_>>()
    );

    // Recall: CortexDB's ask route answers and cites what it used.
    let mut recall = RecallRequest::new("When does Project Aurora launch?", 10);
    recall.filter = MetaFilter {
        workspace: Some(workspace.clone()),
        ..MetaFilter::default()
    };
    let answer = engine.recall(recall).await.expect("recall");
    assert!(!answer.answer.trim().is_empty(), "recall answers");
    assert!(!answer.citations.is_empty(), "recall cites its evidence");

    // context.md: compiled from the same engine, it lists the learning.
    let context = compile(engine, &ContextSpec::default())
        .await
        .expect("compile context");
    assert_eq!(context.engine, engine.descriptor().id);
    assert!(
        context
            .markdown
            .contains("The user prefers launch events in Lisbon."),
        "context.md carries the learning:\n{}",
        context.markdown
    );
    assert!(context.tokens <= ContextSpec::default().budget_tokens);

    // Forget the run.
    let report = engine
        .forget(ForgetTarget::Filter(MetaFilter {
            workspace: Some(workspace.clone()),
            ..MetaFilter::default()
        }))
        .await
        .expect("forget");
    assert_eq!(report.forgotten, 3, "all three items are forgotten");
    let _ = (conversation, learning);
}

/// A long, paged, sectioned document goes to the real server as several
/// events, each under its 1 MiB limit, and comes back whole: `get` and
/// `list` reassemble it, a `fetch` hit is one piece tagged with its page and
/// section, and `forget` removes every piece.
#[tokio::test]
async fn a_long_document_round_trips_in_pieces() {
    let _alone = ONE_AT_A_TIME.lock().await;
    for (wire, engine) in live_engines() {
        eprintln!("long document on {wire}");
        let workspace = format!("ws-long-{}", run_id());
        let pages: Vec<String> = (1..=24)
            .map(|page| {
                let filler =
                    format!("Clause {page} covers refunds and delivery terms. ").repeat(600);
                format!("# Section {page}\n\n{filler}\n")
            })
            .collect();
        let body = pages.join("\u{c}");
        // A document splits once its envelope passes the 256 KiB chunk
        // target (not the 1 MiB event limit): this one is three pieces, so
        // two boundaries are crossed. It stays under 1 MiB because CortexDB
        // 0.10.4 takes seconds to forget a ~240 KiB event (8 to 13 s
        // measured), and a longer document's forget can outlast the 60 s
        // request timeout.
        assert!(
            body.len() > 2 * 256 * 1024,
            "over twice the chunk target: {} bytes",
            body.len()
        );
        let document = StoreItem::Document {
            title: Some("Long contract".into()),
            body: tinymemory_api::DocumentBody::Text(body.clone()),
            mime: Some("application/pdf".into()),
            meta: MemoryMeta {
                workspace: Some(workspace.clone()),
                file_path: Some("/contracts/long.pdf".into()),
                ..MemoryMeta::from_source(SourceKind::File, Some("contracts".into()))
            },
        };
        let receipt = engine
            .store(document.clone())
            .await
            .expect("store in pieces");

        let filter = MetaFilter {
            workspace: Some(workspace.clone()),
            ..MetaFilter::default()
        };
        // `list` returns a chunked document only once every piece is
        // listed, so this waits for all of them, not just the first.
        let listed = list_until(&engine, &filter, 1).await;
        assert_eq!(listed.len(), 1, "one item, not one per piece");
        assert_eq!(
            listed[0],
            document.render_text(),
            "list reassembles the body"
        );
        let got = engine
            .get(tinymemory_api::GetRequest {
                ids: vec![receipt.id.clone()],
                reach: None,
            })
            .await
            .expect("get");
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].text,
            document.render_text(),
            "get reassembles the body"
        );

        // Ranking may lag the write; poll as `list_until` polls the listing.
        let mut fetch = FetchRequest::new("Clause 7 refunds and delivery", FetchMode::Hybrid, 5);
        fetch.filter = filter.clone();
        let deadline = Instant::now() + VISIBILITY;
        let hit = loop {
            let page = engine.fetch(fetch.clone()).await.expect("fetch");
            if let Some(hit) = page.hits.into_iter().find(|hit| hit.id == receipt.id) {
                break hit;
            }
            if Instant::now() >= deadline {
                panic!("fetch never found the document");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        assert!(hit.text.len() < body.len(), "a piece, not the whole");
        assert!(
            hit.meta.tags.iter().any(|tag| tag.starts_with("page:"))
                && hit
                    .meta
                    .tags
                    .iter()
                    .any(|tag| tag.starts_with("section:Section ")),
            "{:?}",
            hit.meta.tags
        );

        let report = engine
            .forget(ForgetTarget::Ids(vec![receipt.id.clone()]))
            .await
            .expect("forget");
        assert_eq!(report.forgotten, 1);
        assert!(
            engine
                .list(ListRequest::new(filter, 10))
                .await
                .expect("list after forget")
                .items
                .is_empty(),
            "the document is gone"
        );
        // `list` hides a document missing a piece, so ask ranked recall,
        // which hits single pieces: none may be left behind.
        let deadline = Instant::now() + VISIBILITY;
        loop {
            let page = engine.fetch(fetch.clone()).await.expect("fetch");
            if !page.hits.iter().any(|hit| hit.id == receipt.id) {
                break;
            }
            assert!(Instant::now() < deadline, "a piece outlived forget");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
