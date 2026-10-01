//! Guarded `EpisodicPortability`: the whole episodic record moving in or out.
//!
//! An export is a read and an import a write, each admitted once per page.
//! An import carries the user's conversation, so every turn and event it
//! hands the driver is redacted exactly as the episodic family's own
//! `insert_turn` and `insert_event` redact them — a copy must not become the
//! path around the rules a recorded turn obeys. Segments and embeddings pass
//! as `set_segment_summary` and `upsert_segment_embedding` pass them.

use async_trait::async_trait;
use tinymemory_api::capabilities::Capability;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{
    EpisodicExportPage, EpisodicImportOutcome, EpisodicPart, EpisodicRecords,
    MemoryEpisodicPortability,
};

use super::types::GuardedEpisodicPortability;
use crate::audit::NO_NAMESPACE;
use crate::policy::GuardPolicy;

#[async_trait]
impl<P: GuardPolicy> MemoryEpisodicPortability for GuardedEpisodicPortability<P> {
    async fn export_episodic(
        &self,
        part: EpisodicPart,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<EpisodicExportPage, MemoryError> {
        self.policy.admit_read(
            Capability::EpisodicPortability,
            "episodic_portability.export_episodic",
            NO_NAMESPACE,
            true,
        )?;
        self.family()?.export_episodic(part, cursor, limit).await
    }

    async fn import_episodic(
        &self,
        records: EpisodicRecords,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        self.policy.admit_write(
            Capability::EpisodicPortability,
            "episodic_portability.import_episodic",
            NO_NAMESPACE,
            true,
        )?;
        let records = match records {
            EpisodicRecords::Turns(mut turns) => {
                for turn in &mut turns {
                    turn.content = self.policy.redact_outbound(&turn.content).into_owned();
                }
                EpisodicRecords::Turns(turns)
            }
            EpisodicRecords::Events(mut events) => {
                for event in &mut events {
                    event.content = self.policy.redact_outbound(&event.content).into_owned();
                }
                EpisodicRecords::Events(events)
            }
            unchanged @ (EpisodicRecords::Segments(_) | EpisodicRecords::SegmentEmbeddings(_)) => {
                unchanged
            }
        };
        self.family()?.import_episodic(records).await
    }
}
