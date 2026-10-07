//! Where an item's scope is: the legacy layout, or v3 below a host's root.
//!
//! **Legacy** (the default) keeps every kind scope below the fixed TinyMemory
//! root, one `app:{kind}` leaf per kind at every node ([`scope_path`]):
//! `app:tinymemory/agent:writer/app:learnings`.
//!
//! **V3** keeps them below a root the host names, usually one person's
//! (`user:<id>`), so one person's memory is one subtree that can be read,
//! confined and erased as a whole. Every kind still has a leaf of its own, so
//! no event sits on an inner node, and erasing a node never takes a sibling
//! kind with it:
//!
//! ```text
//! learning at N                     {root}/{N}/app:learnings
//! conversation at N                 {root}/{N}/app:conversations
//! document at N, N has a source:    {root}/…/app:brain/source:…  (no leaf)
//! document at N, no source:         {root}/{N}/app:documents
//! any N with a service:             app:flows before the first service:
//! ```
//!
//! So `ws:main/agent:a`'s turns are `user:42/ws:main/agent:a/app:conversations`,
//! a Gmail document `user:42/app:brain/source:gmail`, and a workflow's
//! learnings `user:42/ws:main/app:flows/service:newsletter/app:learnings`. The
//! `app` type never names a namespace node, so a path reads back to exactly
//! one node and kind ([`ScopeLayout::parse`]).

use tinymemory_api::{ItemKind, Namespace, SegmentKind};

use super::{ROOT_SCOPE, parse_scope, scope_path};
use crate::cortex::error::{Error, Result};

/// The scope types CortexDB's hosted API admits (`cloud_shared_saas`), the
/// narrowest of its presets: a v3 root uses only these.
const HOSTED_SCOPE_TYPES: [&str; 10] = [
    "org", "dept", "team", "app", "user", "agent", "service", "ws", "project", "source",
];

/// The grouping node a brain's sourced documents sit under.
const BRAIN: &str = "app:brain";

/// The grouping node every workflow's service node sits under.
const FLOWS: &str = "app:flows";

/// The leaf of a learning scope.
const LEARNINGS: &str = "app:learnings";

/// The leaf of a conversation scope.
const CONVERSATIONS: &str = "app:conversations";

/// The leaf of an unsourced document scope.
const DOCUMENTS: &str = "app:documents";

/// How a CortexDB engine lays its items out as scopes. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ScopeLayout {
    /// Below `app:tinymemory`, with an `app:{kind}` leaf at every node.
    #[default]
    Legacy,
    /// Below `root`, with the v3 leaves.
    V3 {
        /// The root path, `type:id` segments joined by `/`.
        root: String,
        /// Whether paths read back may carry a tenant prefix before the root
        /// (the hosted backend's); CortexDB's own API never adds one.
        prefixed: bool,
    },
}

impl ScopeLayout {
    /// The v3 layout below `root`, whose paths read back may carry a tenant
    /// prefix when `prefixed`.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for a root that is not `type:id` segments of the
    /// hosted scope types with `[A-Za-z0-9_-]` ids, or that holds the legacy
    /// root.
    pub(crate) fn v3(root: &str, prefixed: bool) -> Result<Self> {
        let root = root.trim().trim_matches('/');
        let refuse = |why: &str| Error::Config(format!("scope root `{root}` {why}"));
        if root.is_empty() {
            return Err(refuse("is empty"));
        }
        for segment in root.split('/') {
            let Some((kind, id)) = segment.split_once(':') else {
                return Err(refuse("must be type:id segments"));
            };
            if !HOSTED_SCOPE_TYPES.contains(&kind) {
                return Err(refuse(&format!(
                    "uses `{kind}`, which CortexDB's hosted API refuses"
                )));
            }
            let id_ok = !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if !id_ok {
                return Err(refuse("needs ids of A-Z, a-z, 0-9, `_` or `-`"));
            }
            if segment == ROOT_SCOPE {
                return Err(refuse("holds the legacy root"));
            }
        }
        Ok(Self::V3 {
            root: root.to_string(),
            prefixed,
        })
    }

    /// The prefix every scope of this layout is listed under.
    pub(crate) fn root(&self) -> &str {
        match self {
            Self::Legacy => ROOT_SCOPE,
            Self::V3 { root, .. } => root,
        }
    }

    /// The scope items of `kind` at `namespace` live in.
    pub(crate) fn path(&self, namespace: &Namespace, kind: ItemKind) -> String {
        let Self::V3 { root, .. } = self else {
            return scope_path(namespace, kind);
        };
        let mut parts = vec![root.clone()];
        let (mut brain, mut flows) = (false, false);
        for segment in namespace.segments() {
            if segment.kind() == SegmentKind::Service && !flows {
                parts.push(FLOWS.to_string());
                flows = true;
            }
            if segment.kind() == SegmentKind::Source && kind == ItemKind::Document && !brain {
                parts.push(BRAIN.to_string());
                brain = true;
            }
            parts.push(segment.to_string());
        }
        match kind {
            ItemKind::Learning => parts.push(LEARNINGS.to_string()),
            ItemKind::Conversation => parts.push(CONVERSATIONS.to_string()),
            ItemKind::Document if !brain => parts.push(DOCUMENTS.to_string()),
            ItemKind::Document => {}
        }
        parts.join("/")
    }

    /// The prefixes the scopes at or below `namespace` are listed under.
    /// Legacy: the node's own path. V3: the node's path with its grouping
    /// nodes (`app:flows` before a `service:`), and, for a node holding a
    /// `source:`, the same with `app:brain` before it too, where that
    /// node's sourced documents sit. A node without a `source:` needs one
    /// prefix, since a sourced document below it is grouped after it.
    pub(crate) fn node_prefixes(&self, namespace: &Namespace) -> Vec<String> {
        let Self::V3 { root, .. } = self else {
            if namespace.is_root() {
                return vec![ROOT_SCOPE.to_string()];
            }
            let mut path = scope_path(namespace, ItemKind::Document);
            path.truncate(path.rfind('/').unwrap_or(path.len()));
            return vec![path];
        };
        let render = |brain: bool| {
            let mut parts = vec![root.clone()];
            let (mut grouped_brain, mut grouped_flows) = (false, false);
            for segment in namespace.segments() {
                if segment.kind() == SegmentKind::Service && !grouped_flows {
                    parts.push(FLOWS.to_string());
                    grouped_flows = true;
                }
                if brain && segment.kind() == SegmentKind::Source && !grouped_brain {
                    parts.push(BRAIN.to_string());
                    grouped_brain = true;
                }
                parts.push(segment.to_string());
            }
            parts.join("/")
        };
        let sourced = namespace
            .segments()
            .iter()
            .any(|segment| segment.kind() == SegmentKind::Source);
        if sourced {
            vec![render(false), render(true)]
        } else {
            vec![render(false)]
        }
    }

    /// The namespace and kind of a scope path of this layout, wherever it is
    /// rooted (the hosted backend prefixes the caller's tenant); `None` for
    /// any other scope, including one of the other layout. Unprefixed (the
    /// direct wire), a path must start at the root, so a namespace that
    /// begins like the root (`user:42/user:42/app:learnings`) is read as
    /// that namespace. Prefixed (hosted), the root is matched at its last
    /// occurrence, so a tenant prefix spelled like the root reads as a
    /// prefix; only there would a namespace repeating the root's own
    /// segments read back as the root, and a layout never builds one.
    pub(crate) fn parse(&self, path: &str) -> Option<(Namespace, ItemKind)> {
        let Self::V3 { root, prefixed } = self else {
            return parse_scope(path);
        };
        let parts: Vec<&str> = path.split('/').collect();
        let wanted: Vec<&str> = root.split('/').collect();
        let start = if *prefixed {
            parts
                .windows(wanted.len())
                .rposition(|window| window == wanted.as_slice())?
        } else {
            parts.starts_with(&wanted).then_some(0)?
        };
        let rest = &parts[start + wanted.len()..];
        let (kind, nodes) = match rest.split_last()? {
            (&LEARNINGS, nodes) => (ItemKind::Learning, nodes),
            (&CONVERSATIONS, nodes) => (ItemKind::Conversation, nodes),
            (&DOCUMENTS, nodes) => (ItemKind::Document, nodes),
            _ if rest.contains(&BRAIN) => (ItemKind::Document, rest),
            _ => return None,
        };
        let namespace: Namespace = nodes
            .iter()
            .filter(|part| **part != BRAIN && **part != FLOWS)
            .copied()
            .collect::<Vec<_>>()
            .join("/")
            .parse()
            .ok()?;
        // Only the one canonical spelling reads back: a grouping node out of
        // place is somebody else's scope.
        let canonical = self.path(&namespace, kind);
        (canonical == parts[start..].join("/")).then_some((namespace, kind))
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
