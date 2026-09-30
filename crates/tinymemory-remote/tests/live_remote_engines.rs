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
use tinymemory_api::error::MemoryError;
use tinymemory_api::health::MemoryHealth;
use tinymemory_api::provider::{MemoryCore, MemoryProvider, MemoryRecall};
use tinymemory_api::recall::OwnedRecallOpts;
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
    // The secret appears only in a's content — never in a scope, key or
    // query, which the engine echoes back — so finding it in b's answers is
    // a leak.
    let place = format!("iso{}", nonce());
    let secret = format!("secret{}", nonce());
    let namespace = format!("tinymemory-isolation/{place}");
    a.store(
        &namespace,
        "secret",
        &format!("The vault word is {secret}."),
        MemoryCategory::Core,
        None,
        MemoryTaint::Internal,
    )
    .await?;

    // Through the adapter, naming the same namespace.
    assert!(
        b.get(&namespace, "secret").await?.is_none(),
        "b read a's record"
    );
    assert!(
        b.list(Some(&namespace), None, None).await?.is_empty(),
        "b listed a's namespace"
    );
    let opts = OwnedRecallOpts {
        namespace: Some(namespace.clone()),
        ..OwnedRecallOpts::default()
    };
    assert!(
        b.recall("vault word", 10, &opts, None).await?.is_empty(),
        "b recalled a's record"
    );
    assert!(
        !b.namespaces()
            .await?
            .iter()
            .any(|s| s.namespace == namespace),
        "b's namespaces include a's"
    );
    assert!(
        !b.forget(&namespace, "secret").await?,
        "b's forget found a's record"
    );

    // Raw, shaped like the engine's known leaks. The adapter writes a
    // namespace segment `s` as the scope segment `tm:s`.
    let http = reqwest::Client::new();
    let scope = format!("tm:tinymemory-isolation/tm:{place}");
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
        assert!(
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
        assert!(
            !recalled.contains(&secret),
            "b recalled a's event through `{probe}`"
        );
    }

    // A write aimed at a's scope lands in b's own tenant: b can see it, a never can.
    let planted = format!("planted{}", nonce());
    raw(
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
    assert!(landed, "b's own write never became visible to b");
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
    assert!(
        !seen_by_a.contains(&planted),
        "b's write landed in a's scope"
    );

    assert!(
        a.get(&namespace, "secret").await?.is_some(),
        "a lost its own record"
    );
    a.forget(&namespace, "secret").await?;
    Ok(())
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
