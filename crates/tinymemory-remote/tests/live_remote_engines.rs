//! The contract suite against a real hosted service, not a double.
//!
//! Every other test in this crate runs an adapter over a double written from
//! the same documentation the adapter was. That agreement is worth having, but
//! it cannot catch a service that behaves differently from its documentation —
//! and when it does, the adapter and the double are wrong together and the
//! suite stays green. Issue #80 is exactly that: Supermemory strips two
//! characters from stored content server-side, and nothing here could see it
//! because nothing here ever spoke to Supermemory.
//!
//! So this target exists to be pointed at the real thing. It is skipped unless
//! the credentials are present, which keeps `cargo test` offline,
//! deterministic, and independent of a vendor's uptime by default.
//!
//! ```sh
//! TINYMEMORY_TEST_SUPERMEMORY_URL=https://api.supermemory.ai \
//! TINYMEMORY_TEST_SUPERMEMORY_KEY=sm_... \
//!   cargo test -p tinymemory-remote --test live_remote_engines
//! ```
//!
//! The suite writes and deletes records under its own namespaces in whatever
//! account the key belongs to. Point it at a scratch account rather than one
//! holding anything you would miss.

use std::sync::Arc;

use reqwest::Method;
use serde_json::json;
use tinymemory_api::chunks::DataSource;
use tinymemory_api::error::MemoryError;
use tinymemory_api::goals::{GoalItem, GoalsDoc};
use tinymemory_api::health::MemoryHealth;
use tinymemory_api::provider::types::{IngestItem, SourceItem};
use tinymemory_api::provider::{
    EpisodicTurn, FacetState, FacetType, MemoryCore, MemoryProvider, MemoryRecall, ProfileFacet,
    UserState,
};
use tinymemory_api::recall::OwnedRecallOpts;
use tinymemory_api::tool_memory::{ToolMemoryPriority, ToolMemoryRule, ToolMemorySource};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};
use tinymemory_remote::{
    cortex_provider, supermemory_provider, tinyhumans_provider, CortexMemory, StaticBearer,
    SupermemoryMemory,
};

/// Reads one engine's endpoint and key, or `None` when either is unset.
///
/// Both are required rather than defaulting the URL: a live test that invents
/// its own endpoint can end up silently exercising the wrong service.
fn credentials(engine: &str) -> Option<(String, String)> {
    let url = std::env::var(format!("TINYMEMORY_TEST_{engine}_URL")).ok()?;
    let key = std::env::var(format!("TINYMEMORY_TEST_{engine}_KEY")).ok()?;
    (!url.is_empty() && !key.is_empty()).then_some((url, key))
}

/// Runs the full provider contract against the live Supermemory API.
///
/// Skipped without `TINYMEMORY_TEST_SUPERMEMORY_URL` and `..._KEY`.
#[tokio::test]
async fn live_supermemory_upholds_the_provider_contract() -> anyhow::Result<()> {
    let Some((url, key)) = credentials("SUPERMEMORY") else {
        return Ok(());
    };
    let provider = supermemory_provider(SupermemoryMemory::api(&url, &key)?);
    tinymemory_conformance::assert_provider(Arc::new(provider)).await;
    Ok(())
}

/// Runs the full provider contract against a live CortexDB.
///
/// Worth more here than for the keyed engines. The Cortex adapter emulates
/// replacement over an append-only log, so almost everything the contract
/// checks is reconstructed on the read side against behaviour the double can
/// only assert from documentation — paging, listing duplicates, the shape of
/// the destructive selector. Each of those was wrong in the double at some
/// point, and the offline suite was green throughout.
///
/// Skipped without `TINYMEMORY_TEST_CORTEX_URL` and `..._KEY`.
#[tokio::test]
async fn live_cortex_upholds_the_provider_contract() -> anyhow::Result<()> {
    let Some((url, key)) = credentials("CORTEX") else {
        return Ok(());
    };
    let provider = cortex_provider(CortexMemory::api(&url, &key)?);
    tinymemory_conformance::assert_provider(Arc::new(provider)).await;
    Ok(())
}

/// The TinyHumans backend origin and one account's bearer, from
/// `TINYMEMORY_TEST_TINYHUMANS_URL` and `TINYMEMORY_TEST_TINYHUMANS_{token_var}`,
/// or `None` when either is unset.
fn tinyhumans_credentials(token_var: &str) -> Option<(String, String)> {
    let url = std::env::var("TINYMEMORY_TEST_TINYHUMANS_URL").ok()?;
    let token = std::env::var(format!("TINYMEMORY_TEST_TINYHUMANS_{token_var}")).ok()?;
    (!url.is_empty() && !token.is_empty()).then_some((url, token))
}

/// Runs the full provider contract against CortexDB hosted by the TinyHumans
/// backend (`/memory/*`), then checks the two things the offline double cannot:
/// that the health probe is one the real memory API accepts, and that a real
/// model answers from what it was given.
///
/// `TINYMEMORY_TEST_TINYHUMANS_URL` is the backend origin (for example
/// `https://api.tinyhumans.ai`) and `TINYMEMORY_TEST_TINYHUMANS_TOKEN` a session
/// JWT or `tiny_live_` API key. Skipped unless both are set. This one spends the
/// account's credits and is bound by the backend's rate limit (300/min/user),
/// so use a scratch account.
#[tokio::test]
async fn live_tinyhumans_upholds_the_provider_contract() -> anyhow::Result<()> {
    let Some((url, token)) = tinyhumans_credentials("TOKEN") else {
        return Ok(());
    };
    let provider = Arc::new(tinyhumans_provider(
        &url,
        Arc::new(StaticBearer::new(token)),
    )?);
    let health = provider.health().await;
    assert!(
        matches!(health, MemoryHealth::Ready),
        "the hosted health probe must be one the memory API answers: {health:?}"
    );
    tinymemory_conformance::assert_provider(provider.clone()).await;
    tinymemory_conformance::assert_answer_is_grounded(provider.as_ref()).await;
    Ok(())
}

/// The hosted families against the real backend: goals, a tool rule, a source
/// batch and its removal, and a healthy diagnosis. The contract run above
/// already covers documents, because the suite checks every family a provider
/// advertises.
///
/// Needs the same variables as the contract run. It restores the account's own
/// goals and removes everything else it writes.
#[tokio::test]
async fn live_tinyhumans_serves_its_families() -> anyhow::Result<()> {
    let Some((url, token)) = tinyhumans_credentials("TOKEN") else {
        return Ok(());
    };
    let provider = tinyhumans_provider(&url, Arc::new(StaticBearer::new(token)))?;
    let run = nonce();

    let goals = provider
        .as_goals()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve goals"))?;
    let before = goals.goals().await?;
    let probe = GoalsDoc {
        items: vec![GoalItem {
            id: format!("live-{run}"),
            text: "a live probe goal".to_string(),
        }],
    };
    goals.set_goals(probe.clone()).await?;
    let read_back = goals.goals().await;
    goals.set_goals(before).await?;
    anyhow::ensure!(read_back? == probe, "the goals document did not round-trip");

    let rules = provider
        .as_tool_memory()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve tool rules"))?;
    let tool = format!("live-{run}");
    rules
        .put_tool_rule(ToolMemoryRule {
            id: "r1".to_string(),
            ..ToolMemoryRule::new(
                &tool,
                "a live probe rule",
                ToolMemoryPriority::High,
                ToolMemorySource::default(),
            )
        })
        .await?;
    let listed = rules.tool_rules(&tool).await?;
    let removed = rules.delete_tool_rule(&tool, "r1").await?;
    anyhow::ensure!(
        listed.len() == 1 && removed,
        "the tool rule did not round-trip: listed {listed:?}, removed {removed}"
    );

    let sink = provider
        .as_sources()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve the source sink"))?;
    let source = format!("folder:live-{run}");
    let batch = vec![SourceItem {
        item_id: "1".to_string(),
        title: "Live probe".to_string(),
        content: format!("a synced item for run {run}"),
        mime: None,
        url: None,
        updated_at_ms: None,
        tags: Vec::new(),
    }];
    let first = sink
        .accept_source_items(&source, "folder", batch.clone(), MemoryTaint::ExternalSync)
        .await?;
    let again = sink
        .accept_source_items(&source, "folder", batch, MemoryTaint::ExternalSync)
        .await?;
    let forgotten = sink.forget_source(&source).await?;
    anyhow::ensure!(
        first.written == 1 && again.already_ingested && forgotten == 1,
        "the source batch did not round-trip: {first:?}, {again:?}, forgot {forgotten}"
    );

    let diagnosis = provider
        .as_maintenance()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve maintenance"))?
        .diagnose()
        .await?;
    anyhow::ensure!(
        diagnosis.healthy,
        "the hosted service diagnosed unhealthy: {diagnosis:?}"
    );
    Ok(())
}

/// The per-turn families, ingestion and the forest, against the real service.
///
/// Turns and segments have no delete in the contract, so they stay in the
/// account's bookkeeping; point this at a scratch account.
#[tokio::test]
async fn live_tinyhumans_serves_its_per_turn_families() -> anyhow::Result<()> {
    let Some((url, token)) = tinyhumans_credentials("TOKEN") else {
        return Ok(());
    };
    let provider = tinyhumans_provider(&url, Arc::new(StaticBearer::new(token)))?;
    let run = nonce();
    let session = format!("live-{run}");

    let episodic = provider
        .as_episodic()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve episodic memory"))?;
    let turn = episodic
        .insert_turn(&EpisodicTurn {
            id: None,
            session_id: session.clone(),
            timestamp: 1.0,
            role: "user".to_string(),
            content: format!("a live probe turn for run {run}"),
            lesson: None,
            tool_calls_json: None,
            cost_microdollars: 0,
        })
        .await?;
    let turns = episodic.session_turns(&session).await?;
    anyhow::ensure!(
        turns.iter().any(|t| t.id == Some(turn)),
        "the turn did not read back: {turns:?}"
    );
    episodic
        .create_segment(
            &format!("seg-{run}"),
            &session,
            "global",
            turn,
            None,
            1.0,
            1.0,
        )
        .await?;
    let open = episodic.open_segment(&session).await?;
    episodic.close_segment(&format!("seg-{run}"), 2.0).await?;
    anyhow::ensure!(open.is_some(), "the segment did not open");

    let profile = provider
        .as_profile()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve the profile"))?;
    let key = format!("live/{run}");
    profile
        .upsert_facet(&ProfileFacet {
            facet_id: format!("live-{run}"),
            facet_type: FacetType::Context,
            key: key.clone(),
            value: "a live probe facet".to_string(),
            confidence: 0.5,
            evidence_count: 1,
            source_segment_ids: None,
            first_seen_at: 1.0,
            last_seen_at: 1.0,
            state: FacetState::Active,
            stability: 0.5,
            user_state: UserState::Auto,
            evidence_refs: Vec::new(),
            class: None,
            cue_families: None,
        })
        .await?;
    let read = profile.get_facet(&key).await?;
    let deleted = profile.delete_facet(&key).await?;
    anyhow::ensure!(read.is_some() && deleted, "the facet did not round-trip");

    let source = format!("conversations:live-{run}");
    let ingested = provider
        .as_ingest()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve ingest"))?
        .ingest_chat(vec![IngestItem {
            namespace: None,
            source: DataSource::Conversation,
            source_id: source.clone(),
            owner: session.clone(),
            source_ref: None,
            content: format!("a live probe message for run {run}"),
            mime: None,
            timestamp: None,
            tags: Vec::new(),
            author: Some("user".to_string()),
            channel_label: None,
            platform: None,
            to: Vec::new(),
            cc: Vec::new(),
            subject: None,
            list_unsubscribe: None,
            taint: MemoryTaint::Internal,
            path_scope: None,
        }])
        .await?;
    let leaves = provider
        .as_retrieval()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve retrieval"))?
        .retrieve_leaves(&ingested.ids, None)
        .await?;
    let forgotten = provider
        .as_sources()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve the source sink"))?
        .forget_source(&source)
        .await?;
    anyhow::ensure!(
        ingested.written == 1 && leaves.len() == 1 && forgotten == 1,
        "the ingested message did not round-trip: {ingested:?}, {} leaves, forgot {forgotten}",
        leaves.len()
    );

    provider
        .as_tree()
        .ok_or_else(|| anyhow::anyhow!("hosted memory must serve the tree"))?
        .summary_forest(50, None)
        .await?;
    Ok(())
}

/// A token the backend does not recognise is `Unauthorized`, not a miss.
///
/// Needs only `TINYMEMORY_TEST_TINYHUMANS_URL`.
#[tokio::test]
async fn live_tinyhumans_refuses_an_unknown_token() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("TINYMEMORY_TEST_TINYHUMANS_URL") else {
        return Ok(());
    };
    if url.is_empty() {
        return Ok(());
    }
    let provider = tinyhumans_provider(&url, Arc::new(StaticBearer::new("not-a-real-token")))?;
    let refused = provider.namespaces().await;
    assert!(
        matches!(refused, Err(MemoryError::Unauthorized(_))),
        "an unknown token must be Unauthorized, got {refused:?}"
    );
    Ok(())
}

/// Two TinyHumans accounts cannot read, find or change each other's memory.
///
/// CortexDB v0.9.9 does not enforce scope membership on every route: listing
/// another scope's events, writing into another scope, and recalling at a
/// shared ancestor with `view: descend` all leak (cortexdb-saas README). The
/// memory API compensates by re-rooting every scope under the caller's tenant.
/// This probes that boundary from a second account, through the adapter and
/// with raw requests shaped like each known leak.
///
/// Needs `TINYMEMORY_TEST_TINYHUMANS_TOKEN_B`, a second account's bearer,
/// beside the URL and token above. `TINYMEMORY_TEST_TINYHUMANS_USER_A`, the
/// first account's user id, adds probes that name its tenant root outright.
#[tokio::test]
async fn live_tinyhumans_keeps_accounts_apart() -> anyhow::Result<()> {
    let (Some((url, token_a)), Some((_, token_b))) = (
        tinyhumans_credentials("TOKEN"),
        tinyhumans_credentials("TOKEN_B"),
    ) else {
        return Ok(());
    };
    let a = tinyhumans_provider(&url, Arc::new(StaticBearer::new(token_a.clone())))?;
    let b = tinyhumans_provider(&url, Arc::new(StaticBearer::new(token_b.clone())))?;
    let http = reqwest::Client::new();
    // The secret appears only in a's content — never in a scope, key or
    // query, which the engine echoes back — so finding it in b's answers is
    // a leak.
    let place = format!("iso{}", nonce());
    let secret = format!("secret{}", nonce());
    let namespace = format!("tinymemory-isolation/{place}");
    // The adapter writes a namespace segment `s` as the scope segment `tm:s`.
    let scope = format!("tm:tinymemory-isolation/tm:{place}");
    a.store(
        &namespace,
        "secret",
        &format!("The vault word is {secret}."),
        MemoryCategory::Core,
        None,
        MemoryTaint::Internal,
    )
    .await?;

    // Every probe runs in one block, so both accounts' records are removed
    // however it ends: a's record, and the event b plants in its own tenant.
    let mut planted_id: Option<String> = None;
    let outcome = async {
        // Through the adapter, naming the same namespace.
        anyhow::ensure!(
            b.get(&namespace, "secret").await?.is_none(),
            "b read a's record"
        );
        anyhow::ensure!(
            b.list(Some(&namespace), None, None).await?.is_empty(),
            "b listed a's namespace"
        );
        let opts = OwnedRecallOpts {
            namespace: Some(namespace.clone()),
            ..OwnedRecallOpts::default()
        };
        anyhow::ensure!(
            b.recall("vault word", 10, &opts, None).await?.is_empty(),
            "b recalled a's record"
        );
        anyhow::ensure!(
            !b.namespaces()
                .await?
                .iter()
                .any(|s| s.namespace == namespace),
            "b's namespaces include a's"
        );
        anyhow::ensure!(
            !b.forget(&namespace, "secret").await?,
            "b's forget found a's record"
        );

        // Raw, shaped like the engine's known leaks.
        let mut probes = vec![scope.clone(), "tm:tinymemory-isolation".to_string()];
        if let Ok(user) = std::env::var("TINYMEMORY_TEST_TINYHUMANS_USER_A") {
            if !user.is_empty() {
                probes.push(format!("oc:u-{user}/{scope}"));
            }
        }
        for probe in &probes {
            let listed = raw(
                &http,
                &url,
                &token_b,
                Method::GET,
                "memory/events",
                &[("scope", probe)],
                None,
            )
            .await?;
            anyhow::ensure!(
                !listed.contains(&secret),
                "b listed a's event through `{probe}`"
            );
            let recalled = raw(
                &http,
                &url,
                &token_b,
                Method::POST,
                "memory/recall",
                &[],
                Some(json!({ "scope": probe, "query": "vault word", "view": "descend" })),
            )
            .await?;
            anyhow::ensure!(
                !recalled.contains(&secret),
                "b recalled a's event through `{probe}`"
            );
        }

        // A write aimed at a's scope lands in b's own tenant: b can see it,
        // a never can.
        let planted = format!("planted{}", nonce());
        let accepted = raw(
            &http,
            &url,
            &token_b,
            Method::POST,
            "memory/experience",
            &[],
            Some(json!({
                "scope": scope,
                "modality": "observation",
                "idempotency_key": format!("iso-{}", nonce()),
                "content": { "kind": "text", "text": planted },
                "context": {},
            })),
        )
        .await?;
        planted_id = serde_json::from_str::<serde_json::Value>(&accepted)?
            .pointer("/data/event_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let mut landed = false;
        for _ in 0..40 {
            let listed = raw(
                &http,
                &url,
                &token_b,
                Method::GET,
                "memory/events",
                &[("scope", &scope)],
                None,
            )
            .await?;
            if listed.contains(&planted) {
                landed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        anyhow::ensure!(landed, "b's own write never became visible to b");
        let seen_by_a = raw(
            &http,
            &url,
            &token_a,
            Method::GET,
            "memory/events",
            &[("scope", &scope)],
            None,
        )
        .await?;
        anyhow::ensure!(
            !seen_by_a.contains(&planted),
            "b's write landed in a's scope"
        );
        anyhow::ensure!(
            a.get(&namespace, "secret").await?.is_some(),
            "a lost its own record"
        );
        Ok(())
    }
    .await;

    // Best effort: a failed cleanup must not hide the probe's own result.
    let _ = a.forget(&namespace, "secret").await;
    if let Some(id) = planted_id {
        let _ = raw(
            &http,
            &url,
            &token_b,
            Method::POST,
            "memory/forget",
            &[],
            Some(json!({
                "scope": scope,
                "layers": ["events"],
                "selector": { "memory_ids": [id] },
                "audit_note": "tinymemory live isolation test",
            })),
        )
        .await;
    }
    outcome
}

/// One authenticated request to the backend, returning the body of a 2xx.
///
/// A refusal is an error rather than an empty answer: a probe that "found
/// nothing" because its token was rejected would prove nothing.
async fn raw(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    method: Method,
    path: &str,
    query: &[(&str, &str)],
    body: Option<serde_json::Value>,
) -> anyhow::Result<String> {
    let mut request = http
        .request(method, format!("{}/{path}", base.trim_end_matches('/')))
        .bearer_auth(token)
        .query(query);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    anyhow::ensure!(
        status.is_success(),
        "{path} answered {status}: {}",
        text.chars().take(200).collect::<String>()
    );
    Ok(text)
}

/// A token distinct per call and per run, made of scope-safe characters.
fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos}x{}", SEQ.fetch_add(1, Ordering::Relaxed))
}
