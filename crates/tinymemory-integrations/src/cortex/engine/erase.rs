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
//! On the TinyHumans wire only the whole tree erases (`whole_tree`: the
//! root, its descendants, every kind), in one `DELETE memory` that erases the
//! caller's entire hosted memory, every scope under its tenant (including any
//! another layout or client wrote there). The backend proxies no per-scope
//! erasure, so a narrower request refuses with `Unsupported` and sends
//! nothing.

use tinymemory_api::{EraseReport, EraseRequest, ItemKind};

use super::CortexEngine;
use crate::cortex::descriptor::CortexWire;
use crate::cortex::error::{Error, Result};

impl CortexEngine {
    /// See the module docs.
    pub(super) async fn erase_scopes(&self, req: EraseRequest) -> Result<EraseReport> {
        req.validate()?;
        if self.log.client.wire() == CortexWire::TinyHumans {
            if !is_whole_tree(&req) {
                return Err(Error::Unsupported(
                    "the TinyHumans backend erases only the whole memory (whole_tree)"
                        .to_string(),
                ));
            }
            let erased_scopes = self.log.erase_all().await?;
            log::debug!("[cortex] erased the whole hosted memory ({erased_scopes} scopes)");
            return Ok(EraseReport {
                erased_scopes,
                receipts: Vec::new(),
            });
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

/// Whether `req` erases everything: the root, its descendants, every kind.
/// [`EraseRequest::validate`] has already refused it without `whole_tree`.
fn is_whole_tree(req: &EraseRequest) -> bool {
    req.whole_tree && req.reach.at.is_root() && req.reach.descendants && req.kinds.is_empty()
}
