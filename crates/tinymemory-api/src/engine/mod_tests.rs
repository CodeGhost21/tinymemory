//! Descriptor mode checks and health classification.

use super::*;

fn descriptor(modes: Vec<FetchMode>) -> EngineDescriptor {
    EngineDescriptor {
        id: "test",
        label: "Test",
        description: "A test engine.",
        hosted: false,
        needs_endpoint: false,
        needs_key: false,
        default_endpoint: None,
        fetch_modes: modes,
        consolidation: Consolidation::None,
    }
}

#[test]
fn an_undeclared_mode_is_unsupported() {
    let descriptor = descriptor(vec![FetchMode::Hybrid]);
    assert!(descriptor.supports(FetchMode::Hybrid));
    assert!(descriptor.ensure_mode(FetchMode::Hybrid).is_ok());
    let error = descriptor
        .ensure_mode(FetchMode::Vector)
        .expect_err("vector is undeclared");
    assert_eq!(
        error,
        Error::Unsupported("engine `test` does not offer vector fetch".to_string())
    );
}

#[test]
fn only_down_is_not_serving() {
    assert!(EngineHealth::Ok.is_serving());
    assert!(EngineHealth::Degraded("slow".into()).is_serving());
    assert!(!EngineHealth::Down("gone".into()).is_serving());
    assert_eq!(
        serde_json::to_value(EngineHealth::Down("gone".into())).expect("json"),
        serde_json::json!({ "state": "down", "reason": "gone" })
    );
}

/// An engine implementing only the required methods, so every default is
/// the trait's own.
struct Bare(EngineDescriptor);

#[async_trait]
impl MemoryEngine for Bare {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.0
    }

    async fn health(&self) -> EngineHealth {
        EngineHealth::Ok
    }

    async fn recall(&self, _: RecallRequest) -> Result<RecallAnswer> {
        Err(Error::Unsupported("recall".into()))
    }

    async fn fetch(&self, _: FetchRequest) -> Result<FetchPage> {
        Ok(FetchPage::default())
    }

    async fn store(&self, item: StoreItem) -> Result<StoreReceipt> {
        Ok(StoreReceipt {
            id: crate::ItemId(item.fingerprint()),
            replayed: false,
        })
    }

    async fn forget(&self, _: ForgetTarget) -> Result<ForgetReport> {
        Ok(ForgetReport::default())
    }

    async fn list(&self, _: ListRequest) -> Result<ListPage> {
        Ok(ListPage::default())
    }
}

#[tokio::test]
async fn an_engine_that_does_not_export_refuses_as_unsupported() {
    let engine = Bare(descriptor(vec![FetchMode::Hybrid]));
    let refused = engine
        .export(ListRequest::new(crate::MetaFilter::default(), 10))
        .await;
    assert!(matches!(refused, Err(Error::Unsupported(_))), "{refused:?}");
    let invalid = engine
        .export(ListRequest::new(crate::MetaFilter::default(), 0))
        .await;
    assert!(
        matches!(invalid, Err(Error::InvalidRequest(_))),
        "a malformed request is refused before Unsupported: {invalid:?}"
    );
}

#[tokio::test]
async fn store_many_with_defaults_to_store_many_whatever_the_wait() {
    let engine = Bare(descriptor(vec![FetchMode::Hybrid]));
    let items = || {
        vec![
            StoreItem::document("one", crate::MemoryMeta::default()),
            StoreItem::document("two", crate::MemoryMeta::default()),
        ]
    };
    let bulk = engine.store_many(items()).await.expect("store_many");
    for options in [WriteOptions::accepted(), WriteOptions::visible()] {
        let with = engine
            .store_many_with(items(), options)
            .await
            .expect("store_many_with");
        assert_eq!(with, bulk, "{options:?}");
    }
    assert!(
        engine
            .store_many_with(Vec::new(), WriteOptions::accepted())
            .await
            .is_err(),
        "an empty batch is refused, as by store_many"
    );
}
