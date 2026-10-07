//! The standard memory layout: where the brain, each agent's conversations
//! and the learnings live, as namespaces and filters.
//!
//! Every host that adopts the layout reads and writes the same tree, so an
//! engine can be swapped without moving anything:
//!
//! ```text
//! root                              core — holistic recall reads it all
//! ├── source:files      documents   core/brain/files
//! ├── source:notion     documents   core/brain/notion
//! ├── source:github     documents   core/brain/github
//! │   └── project:acme-api  one collection of a large source
//! ├── agent:support-01  conversations   core/conversations/support-01
//! ├── agent:coder-42    conversations   core/conversations/coder-42
//! └── (root itself)     learnings   core/learnings
//! ```
//!
//! - **Brain** — documents, global to every agent: no agent id, one
//!   `source:<id>` node per [`BrainSource`] (the connector or app), and
//!   optionally `project:<collection>` nodes below it
//!   ([`MemoryLayout::brain_collection`]).
//! - **Conversations** — each agent's turns at its own `agent:<id>` node.
//! - **Learnings** — beliefs and facts. Shared ones live at the root; an
//!   engine that consolidates writes its beliefs into the scope it built
//!   them from, and the learnings scope reads the whole tree.
//!
//! The root is [`Namespace::ROOT`] unless a host scopes the whole layout
//! below a node of its own (`team:acme`), which keeps tenants apart on one
//! engine.
//!
//! **Core scopes** share memory beyond one layout. A host that nests every
//! tenant under one company node (`ws:acme/team:hive`) can name an ancestor
//! of the root as a [`CoreScope`]: a hive-wide core or a company brain that
//! every agent below it recalls, read exactly so sibling tenants stay apart.
//!
//! # Example
//!
//! ```
//! use tinymemory_api::{ItemKind, Namespace};
//! use tinymemory_tools::{BrainSource, MemoryLayout};
//!
//! let layout = MemoryLayout::default();
//! assert_eq!(layout.brain(&BrainSource::Files)?.to_string(), "source:files");
//! assert_eq!(layout.conversations("support-01")?.to_string(), "agent:support-01");
//! assert_eq!(layout.learnings(), &Namespace::ROOT);
//!
//! let team = MemoryLayout::new("team:acme".parse()?)?;
//! assert_eq!(team.brain(&BrainSource::Notion)?.to_string(), "team:acme/source:notion");
//! assert_eq!(team.brain_filter(None).kinds, [ItemKind::Document]);
//! # Ok::<(), tinymemory_api::Error>(())
//! ```

mod source;
mod types;

use tinymemory_api::{Error, ItemKind, MetaFilter, Namespace, Reach, Result, Segment, SegmentKind};

pub use source::BrainSource;
pub use types::{CoreScope, DEFAULT_CORE_LIMIT};

/// Deepest a layout root may be: one level must remain for the brain's and
/// the agents' nodes.
const MAX_ROOT_DEPTH: usize = 7;

/// Where every part of an agent's memory lives. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MemoryLayout {
    root: Namespace,
}

impl MemoryLayout {
    /// A layout below `root`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when `root` is so deep no node fits below
    /// it.
    pub fn new(root: Namespace) -> Result<Self> {
        if root.depth() > MAX_ROOT_DEPTH {
            return Err(Error::InvalidRequest(format!(
                "a memory layout root nests at most {MAX_ROOT_DEPTH} deep, `{root}` is deeper"
            )));
        }
        Ok(Self { root })
    }

    /// The layout's root: `core`.
    #[must_use]
    pub fn root(&self) -> &Namespace {
        &self.root
    }

    /// The root's strict ancestors, root first: the nodes a [`CoreScope`]
    /// may name. Empty for a layout at [`Namespace::ROOT`].
    #[must_use]
    pub fn ancestors(&self) -> Vec<Namespace> {
        let mut nodes = Reach::of(self.root.clone()).nodes();
        nodes.pop();
        nodes
    }

    /// Checks `at` may be a core scope of this layout: a strict ancestor of
    /// the root.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for the root itself, a node below it, or a
    /// node beside it.
    pub fn admits_core(&self, at: &Namespace) -> Result<()> {
        if at != &self.root && Reach::of(self.root.clone()).admits(at) {
            Ok(())
        } else {
            Err(Error::InvalidRequest(format!(
                "a core scope must be an ancestor of the layout root `{}`, `{at}` is not",
                self.root
            )))
        }
    }

    /// The node `source`'s documents live at.
    ///
    /// # Errors
    ///
    /// Never for a layout built by [`MemoryLayout::new`]; the depth check is
    /// the namespace's own.
    pub fn brain(&self, source: &BrainSource) -> Result<Namespace> {
        self.root
            .child(Segment::sanitized(SegmentKind::Source, source.id()))
    }

    /// The node one collection of `source` lives at (a repository, a
    /// workspace): a `project:<collection>` child of the source's node, so a
    /// large source can be split into scopes that are each fast to search,
    /// while erasing the source still erases them all. The id is sanitized
    /// ([`Segment::sanitized`]).
    ///
    /// # Errors
    ///
    /// The collection node would pass the namespace's depth limit (a root at
    /// [`MAX_ROOT_DEPTH`]).
    pub fn brain_collection(&self, source: &BrainSource, collection: &str) -> Result<Namespace> {
        self.brain(source)?
            .child(Segment::sanitized(SegmentKind::Project, collection))
    }

    /// The node `agent_id`'s conversations live at. The id is sanitized
    /// ([`Segment::sanitized`]).
    ///
    /// # Errors
    ///
    /// As [`MemoryLayout::brain`].
    pub fn conversations(&self, agent_id: &str) -> Result<Namespace> {
        self.root
            .child(Segment::sanitized(SegmentKind::Agent, agent_id))
    }

    /// The node shared learnings are written to: the root.
    #[must_use]
    pub fn learnings(&self) -> &Namespace {
        &self.root
    }

    /// The brain's documents: one source's, or every source's.
    ///
    /// With no source this reads every document in the layout, including any
    /// an agent stored at its own node.
    #[must_use]
    pub fn brain_filter(&self, source: Option<&BrainSource>) -> MetaFilter {
        let at = source
            .and_then(|source| self.brain(source).ok())
            .unwrap_or_else(|| self.root.clone());
        MetaFilter {
            reach: Some(Reach::subtree(at)),
            ..MetaFilter::kinds([ItemKind::Document])
        }
    }

    /// Conversations: one agent's (and its sub-agents'), or every agent's.
    #[must_use]
    pub fn conversations_filter(&self, agent_id: Option<&str>) -> MetaFilter {
        let at = agent_id
            .and_then(|agent| self.conversations(agent).ok())
            .unwrap_or_else(|| self.root.clone());
        MetaFilter {
            reach: Some(Reach::subtree(at)),
            ..MetaFilter::kinds([ItemKind::Conversation])
        }
    }

    /// Every learning in the layout: shared ones at the root and the beliefs
    /// an engine built at any node below.
    #[must_use]
    pub fn learnings_filter(&self) -> MetaFilter {
        MetaFilter {
            reach: Some(Reach::subtree(self.root.clone())),
            ..MetaFilter::kinds([ItemKind::Learning])
        }
    }

    /// Everything in the layout: the holistic scope.
    #[must_use]
    pub fn holistic_filter(&self) -> MetaFilter {
        MetaFilter {
            reach: Some(Reach::subtree(self.root.clone())),
            ..MetaFilter::default()
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
