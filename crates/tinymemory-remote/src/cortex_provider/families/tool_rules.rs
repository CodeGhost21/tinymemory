//! `MemoryToolMemory` over the hosted wire.
//!
//! Tool rules are ordinary keyed records in the layout the contract documents
//! and the embedded engine uses: namespace [`tool_memory_namespace`], key
//! [`ToolMemoryRule::storage_key`], category `tool_memory`, the rule as JSON.
//! Hosts read and write that layout through the keyed store directly — a
//! prompt prefetch, a capture hook — so this family goes through the same
//! store rather than a layout of its own, and both views see the same rules.
//! The validation and ordering match the embedded engine's rule store.

use async_trait::async_trait;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{MemoryCore, MemoryToolMemory};
use tinymemory_api::tool_memory::{tool_memory_namespace, ToolMemoryRule};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use crate::cortex_provider::CortexProvider;

/// The prefix every rule's key starts with, as [`ToolMemoryRule::storage_key`]
/// builds it.
const RULE_KEY_PREFIX: &str = "rule/";

/// The category rules are stored under, as the embedded engine stores them.
fn rule_category() -> MemoryCategory {
    MemoryCategory::Custom("tool_memory".to_string())
}

#[async_trait]
impl MemoryToolMemory for CortexProvider {
    async fn tool_rules(&self, tool_name: &str) -> Result<Vec<ToolMemoryRule>, MemoryError> {
        let namespace = tool_memory_namespace(tool_name);
        let entries = MemoryCore::list(self, Some(&namespace), None, None).await?;
        // A row in the namespace that is not a rule is somebody else's; it is
        // skipped, not an error, exactly as the engine's rule store skips it.
        let mut rules: Vec<ToolMemoryRule> = entries
            .into_iter()
            .filter(|entry| entry.key.starts_with(RULE_KEY_PREFIX))
            .filter_map(|entry| serde_json::from_str(&entry.content).ok())
            .collect();
        rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        Ok(rules)
    }

    async fn put_tool_rule(&self, mut rule: ToolMemoryRule) -> Result<(), MemoryError> {
        if rule.tool_name.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "a tool rule needs a tool name".to_string(),
            ));
        }
        if rule.rule.trim().is_empty() {
            return Err(MemoryError::Invalid("a tool rule needs a body".to_string()));
        }
        if rule.id.trim().is_empty() {
            rule.id = ToolMemoryRule::generate_id();
        }
        rule.tool_name = rule.tool_name.trim().to_lowercase();
        let namespace = tool_memory_namespace(&rule.tool_name);
        let key = ToolMemoryRule::storage_key(&rule.id);
        if let Some(existing) = MemoryCore::get(self, &namespace, &key)
            .await?
            .and_then(|entry| serde_json::from_str::<ToolMemoryRule>(&entry.content).ok())
        {
            rule.created_at = existing.created_at;
        }
        rule.updated_at = chrono::Utc::now().to_rfc3339();
        MemoryCore::store(
            self,
            &namespace,
            &key,
            &serde_json::to_string(&rule)?,
            rule_category(),
            None,
            MemoryTaint::Internal,
        )
        .await
    }

    async fn delete_tool_rule(&self, tool_name: &str, rule_id: &str) -> Result<bool, MemoryError> {
        MemoryCore::forget(
            self,
            &tool_memory_namespace(tool_name),
            &ToolMemoryRule::storage_key(rule_id),
        )
        .await
    }
}

#[cfg(test)]
#[path = "tool_rules_tests.rs"]
mod test;
