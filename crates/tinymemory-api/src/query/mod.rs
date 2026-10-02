//! Requests and responses for recall, fetch, list and forget.
//!
//! - **Recall** ([`RecallRequest`] → [`RecallAnswer`]) asks a question and gets
//!   a synthesised answer with [`Citation`]s.
//! - **Fetch** ([`FetchRequest`] → [`FetchPage`]) is raw retrieval in a
//!   [`FetchMode`], filtered by metadata.
//! - **List** ([`ListRequest`] → [`ListPage`]) pages through stored items
//!   with no query.
//! - **Forget** ([`ForgetTarget`] → [`ForgetReport`]) removes items by id or by
//!   a non-empty filter.
//!
//! Each request has a `validate` method engines call first, so every engine
//! refuses the same malformed requests the same way.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::item::{ItemId, ItemKind};
use crate::meta::{MemoryMeta, MetaFilter};

/// A question for [`crate::MemoryEngine::recall`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallRequest {
    /// The question.
    pub question: String,
    /// Which items the answer may draw on.
    #[serde(default)]
    pub filter: MetaFilter,
    /// Most citations to gather; must be positive.
    pub limit: usize,
    /// Extra instructions for how to answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl RecallRequest {
    /// A question over everything, gathering at most `limit` citations.
    #[must_use]
    pub fn new(question: impl Into<String>, limit: usize) -> Self {
        Self {
            question: question.into(),
            filter: MetaFilter::default(),
            limit,
            instructions: None,
        }
    }

    /// Checks the request is answerable.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank question or a zero limit.
    pub fn validate(&self) -> Result<()> {
        non_blank("recall question", &self.question)?;
        positive("recall limit", self.limit)
    }
}

/// A synthesised answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallAnswer {
    /// The answer text.
    pub answer: String,
    /// The items it drew on.
    #[serde(default)]
    pub citations: Vec<Citation>,
    /// The model that answered, when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One item an answer drew on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    /// The cited item's id, resolvable through [`crate::MemoryEngine::list`].
    pub id: ItemId,
    /// The cited item's kind.
    pub kind: ItemKind,
    /// The relevant excerpt.
    pub snippet: String,
    /// The cited item's metadata.
    pub meta: MemoryMeta,
    /// Relevance, when the engine scores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

/// How [`crate::MemoryEngine::fetch`] ranks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchMode {
    /// Lexical match.
    Keyword,
    /// Embedding similarity.
    Vector,
    /// The engine's blend of both.
    Hybrid,
}

impl FetchMode {
    /// Every mode, in declaration order.
    pub const ALL: [Self; 3] = [Self::Keyword, Self::Vector, Self::Hybrid];

    /// The stable snake_case wire string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Vector => "vector",
            Self::Hybrid => "hybrid",
        }
    }
}

/// A raw retrieval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FetchRequest {
    /// The query text.
    pub query: String,
    /// How to rank.
    pub mode: FetchMode,
    /// Which items to search.
    #[serde(default)]
    pub filter: MetaFilter,
    /// Page size; must be positive.
    pub limit: usize,
    /// Continue from a previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl FetchRequest {
    /// A first-page fetch over everything.
    #[must_use]
    pub fn new(query: impl Into<String>, mode: FetchMode, limit: usize) -> Self {
        Self {
            query: query.into(),
            mode,
            filter: MetaFilter::default(),
            limit,
            cursor: None,
        }
    }

    /// Checks the request is answerable.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank query or a zero limit.
    pub fn validate(&self) -> Result<()> {
        non_blank("fetch query", &self.query)?;
        positive("fetch limit", self.limit)
    }
}

/// One page of fetch results, best first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FetchPage {
    /// The hits.
    pub hits: Vec<Hit>,
    /// Pass back as [`FetchRequest::cursor`] for the next page; `None` at the
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One stored item as a read returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// The item's id.
    pub id: ItemId,
    /// The item's kind.
    pub kind: ItemKind,
    /// The item's text ([`crate::StoreItem::render_text`] form).
    pub text: String,
    /// The item's metadata.
    pub meta: MemoryMeta,
    /// Relevance; `0.0` in a listing.
    pub score: f32,
}

/// A query-free listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListRequest {
    /// Which items to list.
    #[serde(default)]
    pub filter: MetaFilter,
    /// Page size; must be positive.
    pub limit: usize,
    /// Continue from a previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl ListRequest {
    /// A first page of `limit` items matching `filter`.
    #[must_use]
    pub fn new(filter: MetaFilter, limit: usize) -> Self {
        Self {
            filter,
            limit,
            cursor: None,
        }
    }

    /// Checks the request is answerable.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a zero limit.
    pub fn validate(&self) -> Result<()> {
        positive("list limit", self.limit)
    }
}

/// One page of a listing. Every hit's score is `0.0`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListPage {
    /// The items.
    pub items: Vec<Hit>,
    /// Pass back as [`ListRequest::cursor`] for the next page; `None` at the
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// What [`crate::MemoryEngine::forget`] removes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "the contract names the filter by value; a target is built once per call"
)]
pub enum ForgetTarget {
    /// These items.
    Ids(Vec<ItemId>),
    /// Every item matching the filter, which must not be empty.
    Filter(MetaFilter),
}

impl ForgetTarget {
    /// Checks the target cannot mean "everything".
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for no ids or an empty filter.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Ids(ids) if ids.is_empty() => Err(Error::InvalidRequest(
                "forget needs at least one id".to_string(),
            )),
            Self::Filter(filter) if filter.is_empty() => Err(Error::InvalidRequest(
                "forget refuses an empty filter, which would mean everything".to_string(),
            )),
            _ => Ok(()),
        }
    }
}

/// What a forget removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetReport {
    /// How many items were removed. Ids that named nothing are not counted.
    pub forgotten: usize,
}

fn non_blank(what: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::InvalidRequest(format!("{what} must not be empty")));
    }
    Ok(())
}

fn positive(what: &str, value: usize) -> Result<()> {
    if value == 0 {
        return Err(Error::InvalidRequest(format!("{what} must be positive")));
    }
    Ok(())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
