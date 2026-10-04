//! Namespaces: whose memory an item is, and how far a reader reaches.
//!
//! Memory is a tree of **nodes**. The root holds what every agent shares;
//! below it sit agents, teams, users, workspaces and projects, nested as deep
//! as a host needs (`team:acme/agent:writer`). Every stored item lives at
//! exactly one node, its [`MemoryMeta::namespace`](crate::MemoryMeta), and
//! inside a node each [`ItemKind`](crate::ItemKind) (learnings, documents,
//! conversations) is kept apart, so an engine can hold, recall and erase each
//! on its own.
//!
//! A reader names a [`Reach`]: the node it reads *at*, whether it also reads
//! that node's ancestors (`inherit`, on by default, so an agent sees the
//! memory its team and the root share), and whether it reads the node's
//! descendants. Siblings are never in reach: one agent's memory is invisible
//! to another unless it was written to a node both inherit.
//!
//! The namespace is part of an item's identity, so the same text learned by
//! two agents is two items.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// Deepest a namespace may nest.
pub(crate) const MAX_DEPTH: usize = 8;

/// Longest a segment id may be.
pub(crate) const MAX_SEGMENT_ID: usize = 128;

/// How the root namespace is written.
pub(crate) const ROOT_LABEL: &str = "root";

/// What a namespace segment names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// An agent (or a sub-agent, nested under its parent).
    Agent,
    /// A team of agents sharing memory.
    Team,
    /// A human user.
    User,
    /// A shared workspace.
    Workspace,
    /// A project.
    Project,
}

impl SegmentKind {
    /// The stable wire prefix (`agent`, `team`, `user`, `ws`, `project`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Team => "team",
            Self::User => "user",
            Self::Workspace => "ws",
            Self::Project => "project",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "agent" => Ok(Self::Agent),
            "team" => Ok(Self::Team),
            "user" => Ok(Self::User),
            "ws" => Ok(Self::Workspace),
            "project" => Ok(Self::Project),
            _ => Err(Error::InvalidRequest(format!(
                "`{value}` is not a namespace segment kind"
            ))),
        }
    }
}

/// One step of a namespace path: `agent:researcher`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Segment {
    kind: SegmentKind,
    id: String,
}

impl Segment {
    /// A segment, checking its id: 1 to 128 characters of `[A-Za-z0-9_-]`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for an id outside that charset or length.
    pub fn new(kind: SegmentKind, id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if !valid_id(&id) {
            return Err(Error::InvalidRequest(format!(
                "namespace id `{id}` must be 1 to {MAX_SEGMENT_ID} characters of A-Z, a-z, 0-9, `_` or `-`"
            )));
        }
        Ok(Self { kind, id })
    }

    /// A segment for any host id. A valid id is kept as is; anything else
    /// has its illegal characters replaced with `-` and a short hash of the
    /// original appended, so distinct ids stay distinct. An empty id becomes
    /// `_`.
    #[must_use]
    pub fn sanitized(kind: SegmentKind, raw: &str) -> Self {
        if valid_id(raw) {
            return Self {
                kind,
                id: raw.to_string(),
            };
        }
        let cleaned: String = raw
            .chars()
            .map(|c| if id_char(c) { c } else { '-' })
            .take(MAX_SEGMENT_ID - 9)
            .collect();
        let id = if raw.is_empty() {
            "_".to_string()
        } else {
            format!("{cleaned}-{:08x}", fnv1a(raw))
        };
        Self { kind, id }
    }

    /// What the segment names.
    #[must_use]
    pub fn kind(&self) -> SegmentKind {
        self.kind
    }

    /// The segment's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.id)
    }
}

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_SEGMENT_ID && id.chars().all(id_char)
}

/// 32-bit FNV-1a: a stable, dependency-free disambiguator.
fn fnv1a(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// A node of the memory tree, as the path from the root.
///
/// Written `team:acme/agent:writer`; the root is the empty path, written
/// `root`. On the wire a namespace is that string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Namespace(Vec<Segment>);

impl Namespace {
    /// The root: memory every agent shares.
    pub const ROOT: Self = Self(Vec::new());

    /// A namespace from its segments.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when deeper than 8 segments.
    pub fn new(segments: Vec<Segment>) -> Result<Self> {
        if segments.len() > MAX_DEPTH {
            return Err(Error::InvalidRequest(format!(
                "a namespace nests at most {MAX_DEPTH} deep"
            )));
        }
        Ok(Self(segments))
    }

    /// The node for one agent directly under the root, its id sanitized
    /// ([`Segment::sanitized`]).
    #[must_use]
    pub fn agent(id: &str) -> Self {
        Self(vec![Segment::sanitized(SegmentKind::Agent, id)])
    }

    /// Whether this is the root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path's segments, root first.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.0
    }

    /// How deep the node is; the root is `0`.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.0.len()
    }

    /// The root, every ancestor, then this node.
    fn ancestors_and_self(&self) -> Vec<Self> {
        (0..=self.0.len())
            .map(|depth| Self(self.0[..depth].to_vec()))
            .collect()
    }

    /// Whether this node is `other` or lies below it.
    fn is_within(&self, other: &Self) -> bool {
        self.0.starts_with(&other.0)
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            return f.write_str(ROOT_LABEL);
        }
        for (index, segment) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("/")?;
            }
            write!(f, "{segment}")?;
        }
        Ok(())
    }
}

impl FromStr for Namespace {
    type Err = Error;

    /// Parses `team:acme/agent:writer`; `""` and `root` are the root.
    fn from_str(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.is_empty() || value == ROOT_LABEL {
            return Ok(Self::ROOT);
        }
        let segments = value
            .split('/')
            .map(|part| {
                let (kind, id) = part.split_once(':').ok_or_else(|| {
                    Error::InvalidRequest(format!(
                        "namespace segment `{part}` must be written kind:id"
                    ))
                })?;
                Segment::new(SegmentKind::parse(kind)?, id)
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(segments)
    }
}

impl Serialize for Namespace {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Namespace {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// How far a reader reaches through the namespace tree.
///
/// A reader at `at` sees `at` itself, every ancestor of it when `inherit`
/// (the default), and everything below it when `descendants`. It never sees a
/// sibling: an agent reading with `Reach::of(agent)` sees its own memory and
/// what the root (and any team above it) shares, not another agent's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reach {
    /// The node read from.
    #[serde(default)]
    pub at: Namespace,
    /// Also read every ancestor of `at`.
    #[serde(default = "yes")]
    pub inherit: bool,
    /// Also read everything below `at`.
    #[serde(default)]
    pub descendants: bool,
}

fn yes() -> bool {
    true
}

impl Default for Reach {
    fn default() -> Self {
        Self::of(Namespace::ROOT)
    }
}

impl Reach {
    /// An agent's ordinary reach: `at` and its ancestors.
    #[must_use]
    pub fn of(at: Namespace) -> Self {
        Self {
            at,
            inherit: true,
            descendants: false,
        }
    }

    /// Exactly one node.
    #[must_use]
    pub fn exact(at: Namespace) -> Self {
        Self {
            at,
            inherit: false,
            descendants: false,
        }
    }

    /// A node and everything below it, without its ancestors.
    #[must_use]
    pub fn subtree(at: Namespace) -> Self {
        Self {
            at,
            inherit: false,
            descendants: true,
        }
    }

    /// Whether an item at `namespace` is in reach.
    #[must_use]
    pub fn admits(&self, namespace: &Namespace) -> bool {
        namespace == &self.at
            || (self.inherit && self.at.is_within(namespace))
            || (self.descendants && namespace.is_within(&self.at))
    }

    /// The nodes read exactly, root first: `at` and, when `inherit`, its
    /// ancestors. Descendants are not enumerable here; an engine reads them
    /// as one subtree below `at`.
    #[must_use]
    pub fn nodes(&self) -> Vec<Namespace> {
        if self.inherit {
            self.at.ancestors_and_self()
        } else {
            vec![self.at.clone()]
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
