//! Registry listing and every `build_engine` refusal.

use async_trait::async_trait;

use super::*;

fn settings(endpoint: Option<&str>) -> EngineSettings {
    EngineSettings {
        endpoint: endpoint.map(str::to_string),
        ..EngineSettings::default()
    }
}

fn config_error(result: Result<Arc<dyn MemoryEngine>>) -> String {
    match result {
        Err(Error::Config(message)) => message,
        Err(other) => panic!("expected a config error, got {other}"),
        Ok(_) => panic!("expected a config error, got an engine"),
    }
}

struct Session;

#[async_trait]
impl BearerSource for Session {
    async fn bearer(&self) -> Result<String> {
        Ok("session-jwt".to_string())
    }
}

#[test]
fn both_cortex_engines_are_listed_with_hybrid_fetch_only() {
    let engines = list_engines();
    let ids: Vec<&str> = engines.iter().map(|d| d.id).collect();
    assert_eq!(ids, ["cortexdb", "tinyhumans"]);
    for descriptor in &engines {
        assert_eq!(descriptor.fetch_modes, [tinymemory_api::FetchMode::Hybrid]);
        assert!(descriptor.needs_key);
        assert!(descriptor.default_endpoint.is_some());
    }
}

#[test]
fn an_unknown_id_is_refused() {
    let message = config_error(build_engine(
        "mem0",
        &settings(None),
        EngineCredential::None,
    ));
    assert!(message.contains("unknown memory engine `mem0`"));
}

#[test]
fn a_missing_credential_is_refused() {
    for credential in [
        EngineCredential::None,
        EngineCredential::Static("  ".into()),
    ] {
        let message = config_error(build_engine("cortexdb", &settings(None), credential));
        assert!(message.contains("needs a credential"), "{message}");
    }
}

#[test]
fn credentialed_cleartext_is_refused_off_loopback_only() {
    let message = config_error(build_engine(
        "cortexdb",
        &settings(Some("http://cortex.example.test")),
        EngineCredential::Static("secret-key".into()),
    ));
    assert!(message.contains("https"));
    assert!(!message.contains("secret-key"));
    for loopback in [
        "http://127.0.0.1:7000",
        "http://localhost/",
        "http://[::1]:8080",
    ] {
        assert!(
            build_engine(
                "cortexdb",
                &settings(Some(loopback)),
                EngineCredential::Static("k".into())
            )
            .is_ok(),
            "{loopback}"
        );
    }
}

#[test]
fn a_non_http_endpoint_is_refused() {
    for endpoint in [
        "ftp://cortex.example.test",
        "cortex.example.test",
        "https://",
    ] {
        config_error(build_engine(
            "tinyhumans",
            &settings(Some(endpoint)),
            EngineCredential::Static("k".into()),
        ));
    }
}

#[test]
fn engines_build_with_default_endpoints_and_either_credential() {
    let cortex = build_engine(
        "cortexdb",
        &settings(None),
        EngineCredential::Static("key".into()),
    )
    .unwrap();
    assert_eq!(cortex.descriptor().id, "cortexdb");
    let hosted = build_engine(
        "tinyhumans",
        &settings(Some("  ")),
        EngineCredential::Dynamic(Arc::new(Session)),
    )
    .unwrap();
    assert_eq!(hosted.descriptor().id, "tinyhumans");
    let static_hosted = build_engine(
        "tinyhumans",
        &settings(Some("https://api.example.test")),
        EngineCredential::Static("tiny_live_x".into()),
    )
    .unwrap();
    assert!(static_hosted.descriptor().hosted);
    let dynamic_direct = build_engine(
        "cortexdb",
        &settings(Some("https://cortex.example.test")),
        EngineCredential::Dynamic(Arc::new(Session)),
    )
    .unwrap();
    assert!(!dynamic_direct.descriptor().hosted);
}

#[test]
fn credential_debug_never_shows_the_token() {
    let rendered = format!("{:?}", EngineCredential::Static("hunter2".into()));
    assert!(!rendered.contains("hunter2"));
    assert_eq!(
        format!("{:?}", EngineCredential::default()),
        "EngineCredential::None"
    );
}

#[test]
fn fixed_headers_are_applied_and_a_credential_header_is_refused() {
    let mut with_headers = settings(Some("https://cortex.example.test"));
    with_headers
        .headers
        .insert("x-sdk-name".to_string(), "openhuman".to_string());
    assert!(
        build_engine(
            CORTEXDB_ENGINE_ID,
            &with_headers,
            EngineCredential::Static("ctx_key".to_string())
        )
        .is_ok()
    );

    with_headers
        .headers
        .insert("Authorization".to_string(), "Bearer smuggled".to_string());
    let refused = build_engine(
        CORTEXDB_ENGINE_ID,
        &with_headers,
        EngineCredential::Static("ctx_key".to_string()),
    )
    .err()
    .expect("a credential header is refused");
    assert!(matches!(refused, Error::Config(_)), "{refused:?}");
    assert!(!refused.to_string().contains("smuggled"));
}

#[test]
fn a_configured_consolidation_overrides_the_endpoint_default() {
    let self_hosted = settings(Some("https://cortex.example.test"));
    let key = || EngineCredential::Static("key".into());
    let default = build_engine("cortexdb", &self_hosted, key()).unwrap();
    assert_eq!(
        default.descriptor().consolidation,
        tinymemory_api::Consolidation::OnDemand
    );
    let managed = build_engine("cortexdb", &settings(None), key()).unwrap();
    assert_eq!(
        managed.descriptor().consolidation,
        tinymemory_api::Consolidation::Automatic
    );
    let told = EngineSettings {
        consolidation: Some(tinymemory_api::Consolidation::Automatic),
        ..self_hosted
    };
    let automatic = build_engine("cortexdb", &told, key()).unwrap();
    assert_eq!(
        automatic.descriptor().consolidation,
        tinymemory_api::Consolidation::Automatic
    );
    let message = config_error(build_engine(
        "tinyhumans",
        &EngineSettings {
            consolidation: Some(tinymemory_api::Consolidation::OnDemand),
            ..settings(None)
        },
        EngineCredential::Dynamic(Arc::new(Session)),
    ));
    assert!(message.contains("cannot consolidate"), "{message}");
}
