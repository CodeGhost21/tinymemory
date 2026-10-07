//! Erase: every registered kind scope in reach, deepest first.
//!
//! Only kind scopes are erased, never a node's own path: CortexDB erases a
//! scope's events for good but only redacts the scopes below it, which keep
//! their write keys for 24 hours (a re-sent write then replays and stores
//! nothing; measured on 0.10.5). A kind scope is a leaf (`…/agent:a/app:learnings`
//! and `…/agent:a/agent:b/app:learnings` are siblings), so erasing them
//! deletes every event and releases every key. They are still taken deepest
//! first, so a layout that ever put a scope below another would not strand
//! redacted events behind held keys.
//!
//! Only the Direct wire erases: the TinyHumans backend proxies no erasure
//! route, so the hosted engine refuses with `Unsupported` and sends nothing.

use tinymemory_api::{EraseReport, EraseRequest, ItemKind};

use super::CortexEngine;
use crate::cortex::descriptor::CortexWire;
use crate::cortex::error::{Error, Result};

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn erase_scopes(&self, req: EraseRequest) -> Result<EraseReport> {
        req.validate()?;
        if self.log.client.wire() == CortexWire::TinyHumans {
            return Err(Error::Unsupported(
                "the TinyHumans backend has no erasure route".to_string(),
            ));
        }
        let kinds: Vec<ItemKind> = if req.kinds.is_empty() {
            ItemKind::ALL.to_vec()
        } else {
            req.kinds.clone()
        };
        let mut scopes = self.held_all(&req.reach, &kinds).await?;
        scopes.sort_by_key(|scope| std::cmp::Reverse(scope.path.matches('/').count()));
        let mut report = EraseReport::default();
        for scope in scopes {
            let receipt = self.log.erase(&scope.path).await?;
            log::debug!("[cortex] erased {} ({receipt})", scope.path);
            report.erased_scopes += 1;
            report.receipts.push(receipt);
        }
        Ok(report)
    }
}
