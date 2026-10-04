//! What a context document contains: [`ContextSpec`] and its [`Brief`]s.

use serde::{Deserialize, Serialize};
use tinymemory_api::{MetaFilter, Reach};

use crate::error::{Error, Result};

/// Default token budget for the whole document.
pub const DEFAULT_BUDGET_TOKENS: usize = 1_500;

/// Default number of learnings listed after the briefs.
pub const DEFAULT_LEARNINGS_LIMIT: usize = 20;

/// How a context document is compiled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSpec {
    /// The most tokens the whole document may take, estimated at four
    /// characters per token.
    pub budget_tokens: usize,
    /// The sections, in order; each is answered by one recall.
    pub briefs: Vec<Brief>,
    /// The most learnings listed after the briefs.
    pub learnings_limit: usize,
    /// Whose memory the document is about: every brief and the learnings
    /// read only within this reach (an agent's own node and the nodes it
    /// inherits). `None` reads every namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
}

impl Default for ContextSpec {
    /// The default budget and learnings limit, and the four default briefs.
    fn default() -> Self {
        Self {
            budget_tokens: DEFAULT_BUDGET_TOKENS,
            briefs: Brief::defaults(),
            learnings_limit: DEFAULT_LEARNINGS_LIMIT,
            reach: None,
        }
    }
}

impl ContextSpec {
    /// Checks the spec can produce a document.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidSpec`] for a zero budget, or a brief with a blank
    /// heading or question.
    pub fn validate(&self) -> Result<()> {
        if self.budget_tokens == 0 {
            return Err(Error::InvalidSpec(
                "budget_tokens must be positive".to_string(),
            ));
        }
        for brief in &self.briefs {
            if brief.heading.trim().is_empty() || brief.question.trim().is_empty() {
                return Err(Error::InvalidSpec(
                    "every brief needs a heading and a question".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// One section of the document: a heading and the question that fills it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    /// The section heading.
    pub heading: String,
    /// The question recalled to fill the section.
    pub question: String,
    /// Which items the answer may draw on.
    #[serde(default)]
    pub filter: MetaFilter,
}

impl Brief {
    /// A brief over every stored item.
    #[must_use]
    pub fn new(heading: impl Into<String>, question: impl Into<String>) -> Self {
        Self {
            heading: heading.into(),
            question: question.into(),
            filter: MetaFilter::default(),
        }
    }

    /// The four default briefs, in order: about the user, active work,
    /// preferences and standing instructions, recent important events.
    #[must_use]
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new(
                "About the user",
                "Who is the user: their identity, their role, and how they like to work?",
            ),
            Self::new(
                "Active work",
                "What are the user's current projects, workspaces and repositories?",
            ),
            Self::new(
                "Preferences and standing instructions",
                "What preferences and standing instructions has the user given?",
            ),
            Self::new(
                "Recent important events",
                "What important events happened recently?",
            ),
        ]
    }
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
