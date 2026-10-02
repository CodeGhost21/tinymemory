//! The suite against the reference engine, and against deliberately broken
//! wrappers of it: the reference must pass, and each fault must be caught by
//! the check written for it. Without the second half a suite that asserted
//! nothing would also be green.

use async_trait::async_trait;
use tinymemory_api::{
    EngineDescriptor, EngineHealth, FetchMode, FetchPage, FetchRequest, ForgetReport,
    ForgetTarget, ListPage, ListRequest, MemoryEngine, MetaFilter, RecallAnswer, RecallRequest,
    Result, StoreItem, StoreReceipt,
};
use tinymemory_conformance::{Error, ReferenceEngine, run};

#[tokio::test]
async fn the_reference_engine_passes_and_cleans_up() {
    let engine = ReferenceEngine::new();
    run(&engine).await.expect("the reference engine conforms");
    assert!(engine.is_empty());
}

#[tokio::test]
async fn the_suite_leaves_foreign_items_alone() {
    let engine = ReferenceEngine::new();
    engine
        .store(StoreItem::document("someone else's note", Default::default()))
        .await
        .expect("store");
    run(&engine).await.expect("conforms");
    assert_eq!(engine.len(), 1);
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    IgnoreFetchFilter,
    NeverReplay,
    AcceptEmptyForget,
    ClaimEveryMode,
    CiteUnknownIds,
    Down,
}

struct Faulty {
    inner: ReferenceEngine,
    fault: Fault,
    descriptor: EngineDescriptor,
}

impl Faulty {
    fn new(fault: Fault) -> Self {
        let inner = ReferenceEngine::new();
        let mut descriptor = inner.descriptor().clone();
        if matches!(fault, Fault::ClaimEveryMode) {
            descriptor.fetch_modes = vec![FetchMode::Hybrid];
        }
        Self {
            inner,
            fault,
            descriptor,
        }
    }
}

#[async_trait]
impl MemoryEngine for Faulty {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    async fn health(&self) -> EngineHealth {
        match self.fault {
            Fault::Down => EngineHealth::Down("broken on purpose".into()),
            _ => self.inner.health().await,
        }
    }

    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer> {
        let mut answer = self.inner.recall(req).await?;
        if matches!(self.fault, Fault::CiteUnknownIds) {
            for citation in &mut answer.citations {
                citation.id = "unknown".into();
            }
        }
        Ok(answer)
    }

    async fn fetch(&self, mut req: FetchRequest) -> Result<FetchPage> {
        match self.fault {
            Fault::IgnoreFetchFilter => req.filter = MetaFilter::default(),
            // Serves a mode it does not declare instead of refusing it.
            Fault::ClaimEveryMode => req.mode = FetchMode::Hybrid,
            _ => {}
        }
        self.inner.fetch(req).await
    }

    async fn store(&self, item: StoreItem) -> Result<StoreReceipt> {
        let mut receipt = self.inner.store(item).await?;
        if matches!(self.fault, Fault::NeverReplay) {
            receipt.replayed = false;
        }
        Ok(receipt)
    }

    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport> {
        if matches!(self.fault, Fault::AcceptEmptyForget) && target.validate().is_err() {
            return Ok(ForgetReport::default());
        }
        self.inner.forget(target).await
    }

    async fn list(&self, req: ListRequest) -> Result<ListPage> {
        self.inner.list(req).await
    }
}

#[tokio::test]
async fn each_fault_is_caught_by_its_check() {
    let cases = [
        (Fault::IgnoreFetchFilter, "fetch_filters"),
        (Fault::NeverReplay, "replay"),
        (Fault::AcceptEmptyForget, "empty_forget"),
        (Fault::ClaimEveryMode, "unsupported_modes"),
        (Fault::CiteUnknownIds, "recall"),
        (Fault::Down, "health"),
    ];
    for (fault, expected) in cases {
        let error = run(&Faulty::new(fault))
            .await
            .expect_err("a faulty engine must fail");
        let Error::Check { check, .. } = &error else {
            panic!("{fault:?}: expected a check failure, got {error}");
        };
        assert_eq!(*check, expected, "{fault:?}: {error}");
    }
}
