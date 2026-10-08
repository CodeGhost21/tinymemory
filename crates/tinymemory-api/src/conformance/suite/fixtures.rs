//! The items the suite stores, and the per-field filter probes.
//!
//! Every item carries the run's workspace and its marker word, so the suite
//! can isolate its own items on an engine that already holds data and find
//! them with any fetch mode.

use crate::chrono::{TimeZone, Utc};
use crate::{
    DocumentBody, ItemKind, LearningKind, MemoryMeta, MetaFilter, Role, SourceKind, SourceRef,
    StoreItem, ToolCallRef, Turn, TurnRange,
};

/// One run's identity: the workspace it writes under and the word every item
/// carries.
#[derive(Debug, Clone)]
pub(crate) struct Run {
    pub(crate) workspace: String,
    /// The probes' own workspace, so no other item can satisfy a probe filter.
    pub(crate) probe_workspace: String,
    pub(crate) marker: String,
}

impl Run {
    /// A run with a fresh nonce, so two runs never see each other's items.
    pub(crate) fn fresh() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let nonce = format!("{nanos:x}{:x}", SEQ.fetch_add(1, Ordering::Relaxed));
        let workspace = format!("tinymemory-conformance/{nonce}");
        Self {
            probe_workspace: format!("{workspace}/probes"),
            workspace,
            marker: format!("tmconf{nonce}"),
        }
    }

    /// Metadata naming only the run's workspace.
    pub(crate) fn meta(&self) -> MemoryMeta {
        MemoryMeta {
            workspace: Some(self.workspace.clone()),
            ..MemoryMeta::default()
        }
    }

    /// A filter naming only the run's workspace.
    pub(crate) fn filter(&self) -> MetaFilter {
        MetaFilter {
            workspace: Some(self.workspace.clone()),
            ..MetaFilter::default()
        }
    }

    /// A filter naming only the probes' workspace.
    pub(crate) fn probe_filter(&self) -> MetaFilter {
        MetaFilter {
            workspace: Some(self.probe_workspace.clone()),
            ..MetaFilter::default()
        }
    }

    /// A plain document carrying the marker and `label`.
    pub(crate) fn document(&self, label: &str, meta: MemoryMeta) -> StoreItem {
        StoreItem::document(format!("{} {label} note", self.marker), meta)
    }

    /// One item of each kind, with every optional field of the variant set.
    pub(crate) fn round_trip_items(&self) -> Vec<StoreItem> {
        let at = Utc.with_ymd_and_hms(2025, 5, 6, 7, 8, 9).single();
        let mut doc_meta = self.meta();
        doc_meta.tags = vec!["roundtrip".to_string()];
        let mut conv_meta = self.meta();
        conv_meta.source = SourceRef {
            kind: SourceKind::Conversation,
            id: Some(format!("{}-thread", self.marker)),
        };
        vec![
            StoreItem::Document {
                title: Some("Round trip".to_string()),
                body: DocumentBody::Text(format!("{} document body", self.marker)),
                mime: Some("text/markdown".to_string()),
                meta: doc_meta,
            },
            StoreItem::Conversation {
                turns: vec![
                    Turn {
                        role: Role::User,
                        text: format!("{} what is in the plan", self.marker),
                        at,
                        tool_calls: Vec::new(),
                    },
                    Turn {
                        role: Role::Assistant,
                        text: "the plan has three steps".to_string(),
                        at,
                        tool_calls: vec![ToolCallRef {
                            name: "read_plan".to_string(),
                            id: Some("call-1".to_string()),
                        }],
                    },
                ],
                meta: conv_meta,
            },
            StoreItem::Learning {
                text: format!("{} the user prefers short answers", self.marker),
                kind: LearningKind::Preference,
                confidence: 0.75,
                evidence: Some("said so twice".to_string()),
                meta: self.meta(),
            },
        ]
    }

    /// One item per metadata field, each distinguished by that field alone,
    /// paired with the filter that must select exactly it.
    pub(crate) fn probes(&self) -> Vec<Probe> {
        let m = &self.marker;
        let root = format!("/{m}");
        let probe_meta = || MemoryMeta {
            workspace: Some(self.probe_workspace.clone()),
            ..MemoryMeta::default()
        };
        let with = |edit: &dyn Fn(&mut MemoryMeta)| {
            let mut meta = probe_meta();
            edit(&mut meta);
            meta
        };
        let base = self.probe_filter();
        let conversation = |label: &str, meta: MemoryMeta| StoreItem::Conversation {
            turns: vec![
                Turn::new(Role::User, format!("{m} {label} question")),
                Turn::new(Role::Assistant, format!("{label} answer")),
            ],
            meta,
        };
        let early = Utc.with_ymd_and_hms(2001, 1, 1, 0, 0, 0).single();
        vec![
            Probe::new(
                "folder",
                self.document(
                    "folder",
                    with(&|meta| meta.folder = Some(format!("{root}/src/app"))),
                ),
                MetaFilter {
                    folder: Some(format!("{root}/src")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "file_path",
                self.document(
                    "file",
                    with(&|meta| meta.file_path = Some(format!("{root}/docs/guide.md"))),
                ),
                MetaFilter {
                    file_path: Some(format!("{root}/docs")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "language",
                self.document(
                    "language",
                    with(&|meta| meta.language = Some(format!("{m}-lang"))),
                ),
                MetaFilter {
                    language: Some(format!("{m}-lang")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "repo",
                self.document("repo", with(&|meta| meta.repo = Some(format!("owner/{m}")))),
                MetaFilter {
                    repo: Some(format!("owner/{m}")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "commit",
                self.document(
                    "commit",
                    with(&|meta| meta.commit = Some(format!("{m}c0ffee"))),
                ),
                MetaFilter {
                    commit: Some(format!("{m}c0ffee")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "url",
                self.document(
                    "url",
                    with(&|meta| meta.url = Some(format!("https://{m}.test/a"))),
                ),
                MetaFilter {
                    url: Some(format!("https://{m}.test/a")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "thread_id",
                conversation(
                    "thread",
                    with(&|meta| meta.thread_id = Some(format!("{m}-t1"))),
                ),
                MetaFilter {
                    thread_id: Some(format!("{m}-t1")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "turns",
                conversation(
                    "turns",
                    with(&|meta| meta.turns = Some(TurnRange { first: 4, last: 5 })),
                ),
                MetaFilter {
                    turns: Some(TurnRange { first: 4, last: 5 }),
                    ..base.clone()
                },
            ),
            Probe::new(
                "agent_id",
                self.document(
                    "agent",
                    with(&|meta| meta.agent_id = Some(format!("{m}-agent"))),
                ),
                MetaFilter {
                    agent_id: Some(format!("{m}-agent")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "tool_call",
                self.document(
                    "tool",
                    with(&|meta| {
                        meta.tool_call = Some(ToolCallRef {
                            name: format!("{m}_tool"),
                            id: None,
                        });
                    }),
                ),
                MetaFilter {
                    tool_call: Some(format!("{m}_tool")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "source_id",
                self.document(
                    "source",
                    with(&|meta| {
                        meta.source = SourceRef {
                            kind: SourceKind::File,
                            id: Some(format!("{m}-src")),
                        };
                    }),
                ),
                MetaFilter {
                    source_id: Some(format!("{m}-src")),
                    ..base.clone()
                },
            ),
            Probe::new(
                "sources",
                self.document(
                    "feed",
                    with(&|meta| {
                        meta.source = SourceRef {
                            kind: SourceKind::File,
                            id: None,
                        }
                    }),
                ),
                MetaFilter {
                    sources: vec![SourceKind::File],
                    ..base.clone()
                },
            ),
            Probe::new(
                "tags_any",
                self.document("tagged", with(&|meta| meta.tags = vec![format!("{m}-tag")])),
                MetaFilter {
                    tags_any: vec![format!("{m}-none"), format!("{m}-tag")],
                    ..base.clone()
                },
            ),
            Probe::new(
                "kinds",
                StoreItem::learning(
                    format!("{m} kinds learning"),
                    LearningKind::Fact,
                    0.5,
                    probe_meta(),
                ),
                MetaFilter {
                    kinds: vec![ItemKind::Learning],
                    ..base.clone()
                },
            ),
            Probe::new(
                "observed_at",
                self.document("observed", with(&|meta| meta.observed_at = early)),
                MetaFilter {
                    observed_after: Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).single(),
                    observed_before: Utc.with_ymd_and_hms(2002, 1, 1, 0, 0, 0).single(),
                    ..base
                },
            ),
        ]
    }
}

/// An item distinguished by one metadata field, and the filter on that field.
#[derive(Debug, Clone)]
pub(crate) struct Probe {
    pub(crate) field: &'static str,
    pub(crate) item: StoreItem,
    pub(crate) filter: MetaFilter,
}

impl Probe {
    fn new(field: &'static str, item: StoreItem, filter: MetaFilter) -> Self {
        Self {
            field,
            item,
            filter,
        }
    }
}
