//! OpenHuman LLM adapter for tinycortex tree summarization.

use async_trait::async_trait;
use std::sync::Arc;
use tinycortex::memory::tree::{
    Summariser, SummaryCall, SummaryContext, SummaryInput, SummaryOutput,
};

use crate::Config;

#[derive(Clone)]
pub struct HostSummariser {
    config: Arc<Config>,
}

impl HostSummariser {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    async fn call(
        &self,
        inputs: &[SummaryInput],
        context: &SummaryContext<'_>,
    ) -> anyhow::Result<SummaryCall> {
        let output = crate::tree::summarise::summarise(&*self.config, inputs, context).await?;
        Ok(SummaryCall {
            output: SummaryOutput {
                content: output.content,
                token_count: output.token_count,
                entities: output.entities,
                topics: output.topics,
            },
            input_tokens: 0,
            output_tokens: 0,
            charged_amount_usd: None,
        })
    }
}

#[async_trait]
impl Summariser for HostSummariser {
    fn name(&self) -> &str {
        "openhuman"
    }

    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        context: &SummaryContext<'_>,
    ) -> anyhow::Result<SummaryOutput> {
        Ok(self.call(inputs, context).await?.output)
    }

    async fn summarise_with_usage(
        &self,
        inputs: &[SummaryInput],
        context: &SummaryContext<'_>,
    ) -> anyhow::Result<SummaryCall> {
        self.call(inputs, context).await
    }
}
