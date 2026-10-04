//! [`CoreScope`]: a shared node above a layout that agents recall from.

use serde::{Deserialize, Serialize};
use tinymemory_api::{ItemKind, MetaFilter, Namespace, Reach};

use crate::context::Brief;

/// Default number of items a core section shows.
pub const DEFAULT_CORE_LIMIT: usize = 6;

/// A node above a layout's root whose memory every agent below it recalls:
/// a hive-wide core, or a company brain shared by every team.
///
/// The node is read exactly ([`Reach::exact`]), never as a subtree, so a
/// core read sees what was written at the node itself and nothing from a
/// sibling tenant below it. A host that shares two levels (say the root and
/// `ws:acme`) names two scopes.
///
/// ```
/// use tinymemory_api::{ItemKind, Namespace};
/// use tinymemory_tools::CoreScope;
///
/// let company = CoreScope::new("ws:acme".parse()?, "Company").limit(4);
/// let reach = company.filter().reach.unwrap();
/// assert!(reach.admits(&"ws:acme".parse()?));
/// assert!(!reach.admits(&"ws:acme/team:hive".parse()?));
/// assert_eq!(company.kinds, [ItemKind::Learning, ItemKind::Document]);
/// # Ok::<(), tinymemory_api::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreScope {
    /// The shared node: a strict ancestor of the layout's root.
    pub at: Namespace,
    /// The heading its section carries in a pack.
    pub heading: String,
    /// The kinds of item read from it.
    #[serde(default = "default_kinds")]
    pub kinds: Vec<ItemKind>,
    /// The most items its section shows; `0` leaves the section out.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_kinds() -> Vec<ItemKind> {
    vec![ItemKind::Learning, ItemKind::Document]
}

fn default_limit() -> usize {
    DEFAULT_CORE_LIMIT
}

impl CoreScope {
    /// Learnings and documents at `at`, under `heading`, up to
    /// [`DEFAULT_CORE_LIMIT`] items.
    #[must_use]
    pub fn new(at: Namespace, heading: impl Into<String>) -> Self {
        Self {
            at,
            heading: heading.into(),
            kinds: default_kinds(),
            limit: DEFAULT_CORE_LIMIT,
        }
    }

    /// The same scope reading only `kinds`; none reads every kind.
    #[must_use]
    pub fn kinds(mut self, kinds: impl IntoIterator<Item = ItemKind>) -> Self {
        self.kinds = kinds.into_iter().collect();
        self
    }

    /// The same scope showing at most `limit` items.
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// What the scope reads: its kinds, at exactly its node.
    #[must_use]
    pub fn filter(&self) -> MetaFilter {
        MetaFilter {
            reach: Some(Reach::exact(self.at.clone())),
            ..MetaFilter::kinds(self.kinds.iter().copied())
        }
    }

    /// A `context.md` section answering `question` from this scope alone.
    /// Leave [`crate::context::ContextSpec::reach`] unset (or set it to an
    /// agent's [`Reach::of`], which already covers its ancestors) or the
    /// spec's reach replaces this brief's.
    #[must_use]
    pub fn brief(&self, question: impl Into<String>) -> Brief {
        Brief {
            filter: self.filter(),
            ..Brief::new(self.heading.clone(), question)
        }
    }
}
