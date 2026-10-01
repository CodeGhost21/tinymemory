//! `MemoryGoals` over the hosted wire: one bookkeeping record, replaced whole.

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::goals::GoalsDoc;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::MemoryGoals;

use super::records::{Place, Record, Records};
use super::scopes::GOALS;
use crate::cortex_provider::CortexProvider;

/// The one key the goals document is kept under.
const GOALS_KEY: &str = "goals";

impl CortexProvider {
    fn goals_place(&self) -> Result<Place, MemoryError> {
        Place::bookkeeping(&self.dialect, GOALS.to_string()).map_err(engine_error)
    }
}

#[async_trait]
impl MemoryGoals for CortexProvider {
    async fn goals(&self) -> Result<GoalsDoc, MemoryError> {
        let place = self.goals_place()?;
        let Some(live) = Records::new(&self.dialect)
            .live(&place, GOALS_KEY)
            .await
            .map_err(engine_error)?
        else {
            return Ok(GoalsDoc::default());
        };
        serde_json::from_str(&live.record.content).map_err(|error| {
            MemoryError::Backend(format!(
                "hosted memory holds an unreadable goals document: {error}"
            ))
        })
    }

    async fn set_goals(&self, goals: GoalsDoc) -> Result<(), MemoryError> {
        let place = self.goals_place()?;
        let record = Record::plain(GOALS_KEY, serde_json::to_string(&goals)?);
        Records::new(&self.dialect)
            .put(&place, &record, None)
            .await
            .map_err(engine_error)
    }
}

#[cfg(test)]
#[path = "goals_test.rs"]
mod test;
