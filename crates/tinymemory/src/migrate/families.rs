//! The keyed families a copy moves whole: document details, goals and the
//! learned profile.
//!
//! Each reads everything the source holds, compares it with what the target
//! holds, and writes only the difference, so a second run writes nothing.

use crate::capabilities::Capability;
use crate::goals::{GoalItem, GoalsDoc};
use crate::provider::MemoryProvider;
use crate::types::NamespaceDocumentInput;

use super::{unserved, CopyOptions, CopyProgress, MigrateStep, StepReport};

/// The details a document has when nothing set them: the embedded engine's
/// defaults for a plain `store`. A document with exactly these is fully
/// carried by the keyed records already.
fn has_default_details(title: &str, key: &str, source_type: &str, priority: &str) -> bool {
    title == key && source_type == "chat" && priority == "medium"
}

/// Whether `namespace` lies under one of `prefixes`, in either spelling a
/// driver may list it in: as written, or as the embedded engine stores it,
/// with every character outside `[A-Za-z0-9_/-]` turned to `_`. That engine
/// lists the stored spelling, so a synced `source:gmail:…` namespace comes
/// back as `source_gmail_…` and a written-only comparison never matches it.
/// The engine already reads both spellings as one namespace.
pub(super) fn skipped_namespace(namespace: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|prefix| {
        namespace.starts_with(prefix.as_str()) || namespace.starts_with(&stored_spelling(prefix))
    })
}

/// `name` as the embedded engine stores a namespace: characters outside
/// `[A-Za-z0-9_/-]` become `_`.
fn stored_spelling(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// The `(namespace, key)` of every document a `list_documents` page names.
fn listed(page: &serde_json::Value) -> Vec<(String, String)> {
    page.get("documents")
        .and_then(serde_json::Value::as_array)
        .map(|documents| {
            documents
                .iter()
                .filter_map(|document| {
                    Some((
                        document.get("namespace")?.as_str()?.to_string(),
                        document.get("key")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// [`MigrateStep::Documents`]: re-puts every document whose details are not
/// the defaults, so its title, tags, source type, priority and metadata
/// arrive with it.
pub(super) async fn documents(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    options: &CopyOptions,
    progress: &mut impl FnMut(CopyProgress),
) -> anyhow::Result<StepReport> {
    let step = MigrateStep::Documents;
    let (Some(source), Some(target)) = (from.as_documents(), to.as_documents()) else {
        let side = if from.as_documents().is_none() {
            "source"
        } else {
            "target"
        };
        return Ok(StepReport::skipped(
            step,
            unserved(side, Capability::Documents),
        ));
    };
    let mut report = StepReport::new(step);
    let mut namespaces = source.list_namespaces().await?;
    namespaces.retain(|namespace| !skipped_namespace(namespace, &options.skip_namespace_prefixes));
    namespaces.sort();
    namespaces.dedup();
    for namespace in namespaces {
        let page = source.list_documents(Some(&namespace)).await?;
        for (listed_namespace, key) in listed(&page) {
            // A driver may answer a namespace's listing with its sanitised
            // spelling; the document is addressed the way it was listed.
            let Some(document) = source.get_document(&listed_namespace, &key).await? else {
                continue;
            };
            report.read += 1;
            let defaults = has_default_details(
                &document.title,
                &document.key,
                &document.source_type,
                &document.priority,
            ) && document.tags.is_empty()
                && document
                    .metadata
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty);
            if defaults {
                report.unchanged += 1;
                continue;
            }
            if let Some(held) = target
                .get_document(&document.namespace, &document.key)
                .await?
            {
                if held.title == document.title
                    && held.content == document.content
                    && held.source_type == document.source_type
                    && held.priority == document.priority
                    && held.tags == document.tags
                    && held.metadata == document.metadata
                {
                    report.unchanged += 1;
                    continue;
                }
            }
            let label = format!("document {}/{}", document.namespace, document.key);
            let input = NamespaceDocumentInput {
                namespace: document.namespace,
                key: document.key,
                title: document.title,
                content: document.content,
                source_type: document.source_type,
                priority: document.priority,
                tags: document.tags,
                metadata: document.metadata,
                category: document.category,
                session_id: document.session_id,
                document_id: Some(document.document_id),
                taint: document.taint,
            };
            match target.put_document(input).await {
                Ok(_) => report.written += 1,
                Err(error @ crate::error::MemoryError::Invalid(_)) => {
                    report.fail(format!("{label}: {error}"));
                }
                Err(error) => return Err(error.into()),
            }
            progress(CopyProgress {
                step,
                read: report.read,
                written: report.written,
            });
        }
    }
    Ok(report)
}

/// The lowest goal id `g{n}` not in `items`. One of the first `len + 1` is
/// always free.
fn free_goal_id(items: &[GoalItem]) -> String {
    (1..=items.len() + 1)
        .map(|n| format!("g{n}"))
        .find(|id| !items.iter().any(|item| &item.id == id))
        .unwrap_or_default()
}

fn same_goal(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// [`MigrateStep::Goals`]: the source's goals, added after the target's own.
/// A goal the target already states is not added again, and one whose id the
/// target already uses takes a free one.
pub(super) async fn goals(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    progress: &mut impl FnMut(CopyProgress),
) -> anyhow::Result<StepReport> {
    let step = MigrateStep::Goals;
    let (Some(source), Some(target)) = (from.as_goals(), to.as_goals()) else {
        let side = if from.as_goals().is_none() {
            "source"
        } else {
            "target"
        };
        return Ok(StepReport::skipped(step, unserved(side, Capability::Goals)));
    };
    let mut report = StepReport::new(step);
    let wanted = source.goals().await?;
    report.read = wanted.items.len();
    let mut merged = target.goals().await?;
    let mut added = 0usize;
    for item in wanted.items {
        if merged
            .items
            .iter()
            .any(|held| same_goal(&held.text, &item.text))
        {
            report.unchanged += 1;
            continue;
        }
        let id = if merged.items.iter().any(|held| held.id == item.id) {
            free_goal_id(&merged.items)
        } else {
            item.id
        };
        merged.items.push(GoalItem {
            id,
            text: item.text,
        });
        added += 1;
    }
    if added > 0 {
        match target
            .set_goals(GoalsDoc {
                items: merged.items,
            })
            .await
        {
            Ok(()) => report.written = added,
            Err(error @ crate::error::MemoryError::Invalid(_)) => {
                report.failed = added;
                report.note(format!("goals: {error}"));
            }
            Err(error) => return Err(error.into()),
        }
    }
    progress(CopyProgress {
        step,
        read: report.read,
        written: report.written,
    });
    Ok(report)
}

/// [`MigrateStep::Profile`]: every facet the source holds, unless the target
/// holds the same key seen as recently or later — it is the newer claim.
pub(super) async fn profile(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    progress: &mut impl FnMut(CopyProgress),
) -> anyhow::Result<StepReport> {
    let step = MigrateStep::Profile;
    let (Some(source), Some(target)) = (from.as_profile(), to.as_profile()) else {
        let side = if from.as_profile().is_none() {
            "source"
        } else {
            "target"
        };
        return Ok(StepReport::skipped(
            step,
            unserved(side, Capability::Profile),
        ));
    };
    let mut report = StepReport::new(step);
    for facet in source.list_all_facets().await? {
        report.read += 1;
        if let Some(held) = target.get_facet(&facet.key).await? {
            if held == facet || held.last_seen_at >= facet.last_seen_at {
                report.unchanged += 1;
                continue;
            }
        }
        match target.upsert_facet(&facet).await {
            Ok(()) => report.written += 1,
            Err(error @ crate::error::MemoryError::Invalid(_)) => {
                report.fail(format!("facet {}: {error}", facet.key));
            }
            Err(error) => return Err(error.into()),
        }
        progress(CopyProgress {
            step,
            read: report.read,
            written: report.written,
        });
    }
    Ok(report)
}
