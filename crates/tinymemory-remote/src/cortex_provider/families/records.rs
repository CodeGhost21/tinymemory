//! The keyed record layer the hosted families build on.
//!
//! A record is the storage tier's own: one event per version, the adapter's
//! JSON envelope in `content.text`, newest `wal_offset` wins. A family record
//! adds three things `store` does not write, and nothing else:
//!
//! - lookup labels, so reading one key lists that key's versions instead of the
//!   whole scope;
//! - provenance, in the envelope's `x.prov`;
//! - for bookkeeping, directives telling the engine not to embed or extract it.
//!
//! Every write goes through [`CortexDialect::append_envelope`], so it keeps the
//! one-claim retry and the outcome-unknown recovery `store` has.
//!
//! # Retiring superseded versions
//!
//! The engine keeps every version, and ranked recall ranks all of them, so a
//! rewritten key's older text keeps surfacing in recall long after reads stop
//! returning it. [`Records::put`] therefore removes the versions it saw before
//! it wrote, once the new one is readable. It never removes a version it did
//! not see, so a concurrent newer write survives. A removal that fails is not
//! the write's failure: the new version is already the one every read returns,
//! and the key's next write or removal retires whatever was left.

use std::collections::HashMap;

use serde_json::{json, Value};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use crate::cortex::{taint_of, taint_wire, AppendedEvent, CortexDialect, Envelope, KeyedWrite};

use crate::cortex_labels as labels;

/// Keys looked up in one labelled listing. The labels share one comma-separated
/// parameter, so this bounds the URL rather than the engine.
const LOOKUP_BATCH: usize = 40;

/// The audit note on a removal of superseded versions.
pub(super) const SUPERSEDED: &str = "tinymemory: superseded by a newer write";

/// The audit note on a removal of a removed key's versions.
const REMOVED: &str = "tinymemory: removed";

/// The audit note on clearing a scope.
const CLEARED: &str = "tinymemory: cleared";

/// Where a record lives, and how its scope is read and written.
#[derive(Clone, Debug)]
pub(super) struct Place {
    /// The scope path written to and listed.
    pub(super) scope: String,
    /// Whether every record here carries lookup labels. A user namespace may
    /// not: `store` wrote none before it labelled its hosted writes, so a
    /// labelled read that misses a key there must walk the scope.
    labelled_only: bool,
    /// Whether records here are bookkeeping: written inert, and waited on by
    /// the listing alone because nothing recalls them.
    bookkeeping: bool,
}

impl Place {
    /// A user namespace, which the storage tier may also write.
    ///
    /// # Errors
    ///
    /// When the namespace does not fit a hosted scope.
    pub(super) fn namespace(dialect: &CortexDialect, namespace: &str) -> anyhow::Result<Self> {
        Ok(Self {
            scope: dialect.scope_for(namespace)?,
            labelled_only: false,
            bookkeeping: false,
        })
    }

    /// A namespace only the families write, so every record in it is labelled.
    ///
    /// # Errors
    ///
    /// When the namespace does not fit a hosted scope.
    pub(super) fn family_namespace(
        dialect: &CortexDialect,
        namespace: &str,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            labelled_only: true,
            ..Self::namespace(dialect, namespace)?
        })
    }

    /// A bookkeeping scope, checked against the hosted scope depth.
    ///
    /// # Errors
    ///
    /// When the scope holds more segments than a hosted scope may.
    pub(super) fn bookkeeping(dialect: &CortexDialect, scope: String) -> anyhow::Result<Self> {
        let limit = dialect.wire.max_scope_segments();
        let segments = scope.split('/').filter(|s| !s.is_empty()).count();
        anyhow::ensure!(
            segments <= limit,
            "bookkeeping scope has {segments} segments; a hosted scope holds at most {limit}"
        );
        Ok(Self {
            scope,
            labelled_only: true,
            bookkeeping: true,
        })
    }
}

/// Where a record came from, kept in the envelope's `x.prov`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Provenance {
    /// The source the record was synced from.
    pub(super) source: Option<String>,
    /// The record's reference within its source: a URL or an item id.
    pub(super) reference: Option<String>,
    /// The document this record is the content of.
    pub(super) document: Option<String>,
}

impl Provenance {
    /// The envelope's `x`, or `None` when there is nothing to carry, so a
    /// record without provenance is exactly what `store` writes.
    fn to_extension(&self) -> Option<Value> {
        let mut prov = serde_json::Map::new();
        for (name, value) in [
            ("src", &self.source),
            ("ref", &self.reference),
            ("doc", &self.document),
        ] {
            if let Some(value) = value {
                prov.insert(name.to_string(), json!(value));
            }
        }
        (!prov.is_empty()).then(|| json!({ "prov": prov }))
    }

    /// Reads `x.prov`. Anything else in `x` — an ingestion payload — is not
    /// provenance and reads as none.
    fn from_extension(extension: Option<&Value>) -> Self {
        let field = |name: &str| {
            extension
                .and_then(|x| x.pointer(&format!("/prov/{name}")))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        Self {
            source: field("src"),
            reference: field("ref"),
            document: field("doc"),
        }
    }
}

/// One record, as written and as read back.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Record {
    /// The logical key.
    pub(super) key: String,
    /// The content, untouched.
    pub(super) content: String,
    /// The category.
    pub(super) category: MemoryCategory,
    /// The session, when there is one.
    pub(super) session_id: Option<String>,
    /// The provenance taint.
    pub(super) taint: MemoryTaint,
    /// Where the record came from.
    pub(super) provenance: Provenance,
}

impl Record {
    /// An internal core record under `key` holding `content`, with no session
    /// or provenance: the shape every bookkeeping record has.
    pub(super) fn plain(key: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            content: content.into(),
            category: MemoryCategory::Core,
            session_id: None,
            taint: MemoryTaint::Internal,
            provenance: Provenance::default(),
        }
    }

    fn envelope(&self, deleted: bool) -> Envelope {
        Envelope {
            k: self.key.clone(),
            c: self.content.clone(),
            cat: Some(self.category.to_string()),
            s: self.session_id.clone(),
            t: Some(taint_wire(self.taint).to_string()),
            d: deleted,
            x: self.provenance.to_extension(),
        }
    }
}

/// One version of a record, as the log holds it.
#[derive(Clone, Debug)]
pub(super) struct Version {
    /// The engine's id for the event.
    pub(super) event_id: String,
    /// The event's `wal_offset`: newer versions are higher.
    pub(super) order: u64,
    /// Whether this version is a tombstone.
    pub(super) deleted: bool,
    /// When the engine recorded it, RFC 3339, or empty.
    pub(super) recorded_at: String,
    /// When its content was true, RFC 3339, when the write said.
    pub(super) observed_at: Option<String>,
    /// The scope the event lives in, as the read path reported it; empty when
    /// the read path named none.
    pub(super) scope: String,
    /// The record it carries.
    pub(super) record: Record,
}

impl Version {
    /// The version an event carries, or `None` for an event the adapter did
    /// not write.
    pub(super) fn of(event: &Value) -> Option<Self> {
        let text = event.pointer("/content/text")?.as_str()?;
        let envelope = CortexDialect::envelope_of(text)?;
        Some(Self {
            event_id: event.get("id")?.as_str()?.to_string(),
            order: event
                .get("wal_offset")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            deleted: envelope.d,
            recorded_at: event
                .pointer("/context/recorded_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            observed_at: event
                .pointer("/context/observed_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            scope: event
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            record: Record {
                key: envelope.k,
                content: envelope.c,
                category: crate::common::category(envelope.cat.as_deref()),
                session_id: envelope.s,
                taint: taint_of(envelope.t.as_deref()),
                provenance: Provenance::from_extension(envelope.x.as_ref()),
            },
        })
    }
}

/// The newest of `versions`, unless it is a tombstone.
pub(super) fn newest_live(versions: &[Version]) -> Option<&Version> {
    versions
        .iter()
        .max_by_key(|version| version.order)
        .filter(|version| !version.deleted)
}

/// Every key's versions in `events`, each list oldest first.
fn by_key(events: &[Value]) -> HashMap<String, Vec<Version>> {
    let mut out: HashMap<String, Vec<Version>> = HashMap::new();
    for version in events.iter().filter_map(Version::of) {
        out.entry(version.record.key.clone())
            .or_default()
            .push(version);
    }
    for versions in out.values_mut() {
        versions.sort_by_key(|version| version.order);
    }
    out
}

/// Reads and writes keyed records through one dialect.
pub(super) struct Records<'a> {
    dialect: &'a CortexDialect,
}

impl<'a> Records<'a> {
    pub(super) fn new(dialect: &'a CortexDialect) -> Self {
        Self { dialect }
    }

    /// Every version of each of `keys` in `place`, oldest first per key. A key
    /// with no version is absent from the map.
    ///
    /// # Errors
    ///
    /// Backend failures.
    pub(super) async fn versions(
        &self,
        place: &Place,
        keys: &[&str],
    ) -> anyhow::Result<HashMap<String, Vec<Version>>> {
        self.lookup(place, keys, !place.labelled_only).await
    }

    /// [`Self::versions`], walking the whole scope for the keys the labels
    /// missed only when `walk_on_miss` is set.
    async fn lookup(
        &self,
        place: &Place,
        keys: &[&str],
        walk_on_miss: bool,
    ) -> anyhow::Result<HashMap<String, Vec<Version>>> {
        let mut found: HashMap<String, Vec<Version>> = HashMap::new();
        for batch in keys.chunks(LOOKUP_BATCH) {
            let filter = batch
                .iter()
                .map(|key| labels::key(key))
                .collect::<Vec<_>>()
                .join(",");
            let events = self
                .dialect
                .events_matching(&place.scope, Some(&filter))
                .await?;
            for (key, versions) in by_key(&events) {
                if batch.contains(&key.as_str()) {
                    found.insert(key, versions);
                }
            }
        }
        let missed = keys.iter().any(|key| !found.contains_key(*key));
        if missed && walk_on_miss {
            // A version `store` wrote before it labelled its hosted writes
            // carries no label, so its key is only found by walking the scope.
            let events = self.dialect.events_matching(&place.scope, None).await?;
            for (key, versions) in by_key(&events) {
                if keys.contains(&key.as_str()) && !found.contains_key(&key) {
                    found.insert(key, versions);
                }
            }
        }
        Ok(found)
    }

    /// The live version of `key` in `place`, if any.
    ///
    /// # Errors
    ///
    /// Backend failures.
    pub(super) async fn live(&self, place: &Place, key: &str) -> anyhow::Result<Option<Version>> {
        let versions = self.versions(place, &[key]).await?;
        Ok(versions
            .get(key)
            .and_then(|versions| newest_live(versions))
            .cloned())
    }

    /// The live version of every key in `place`, sorted by key.
    ///
    /// # Errors
    ///
    /// Backend failures.
    pub(super) async fn live_all(&self, place: &Place) -> anyhow::Result<Vec<Version>> {
        let events = self.dialect.events_matching(&place.scope, None).await?;
        let mut live: Vec<Version> = by_key(&events)
            .values()
            .filter_map(|versions| newest_live(versions).cloned())
            .collect();
        live.sort_by(|a, b| a.record.key.cmp(&b.record.key));
        Ok(live)
    }

    /// Every version in `place` that came from `source_id`, re-checked against
    /// the provenance it carries.
    ///
    /// # Errors
    ///
    /// Backend failures.
    pub(super) async fn of_source(
        &self,
        place: &Place,
        source_id: &str,
    ) -> anyhow::Result<Vec<Version>> {
        let events = self
            .dialect
            .events_matching(&place.scope, Some(&labels::source(source_id)))
            .await?;
        Ok(events
            .iter()
            .filter_map(Version::of)
            .filter(|version| version.record.provenance.source.as_deref() == Some(source_id))
            .collect())
    }

    /// The live version of every key in `place` written in `session_id`,
    /// re-checked against the session each record carries.
    ///
    /// # Errors
    ///
    /// Backend failures.
    pub(super) async fn of_session(
        &self,
        place: &Place,
        session_id: &str,
    ) -> anyhow::Result<Vec<Version>> {
        let events = self
            .dialect
            .events_matching(&place.scope, Some(&labels::session(session_id)))
            .await?;
        Ok(by_key(&events)
            .values()
            .filter_map(|versions| newest_live(versions).cloned())
            .filter(|version| version.record.session_id.as_deref() == Some(session_id))
            .collect())
    }

    /// Writes `record` under a key that has never been written — a fresh turn
    /// or event id — and waits until it can be read. Nothing is looked up or
    /// retired, because there is nothing older to find.
    ///
    /// # Errors
    ///
    /// Backend failures on the write or the wait.
    pub(super) async fn insert(&self, place: &Place, record: &Record) -> anyhow::Result<()> {
        if let Some(event) = self.append(place, record, None, false).await? {
            self.wait(place, &event).await?;
        }
        Ok(())
    }

    /// Writes `record` as its key's newest version, waits until it can be
    /// read, then retires the versions it replaced.
    ///
    /// `observed_at` is when the content was true, RFC 3339.
    ///
    /// Only labelled versions are retired. Walking a user namespace to find an
    /// unlabelled version — one `store` wrote before it labelled its hosted
    /// writes — would cost every new key a whole-scope listing; such a version
    /// stays in the log, as `store`'s own rewrites always have, and the fold
    /// reads past it.
    ///
    /// # Errors
    ///
    /// Backend failures on the lookup, the write, or the wait. Retiring is
    /// best effort; see the module docs.
    pub(super) async fn put(
        &self,
        place: &Place,
        record: &Record,
        observed_at: Option<String>,
    ) -> anyhow::Result<()> {
        let older = self
            .lookup(place, &[&record.key], false)
            .await?
            .remove(&record.key)
            .unwrap_or_default();
        if let Some(event) = self.append(place, record, observed_at, false).await? {
            self.wait(place, &event).await?;
            self.retire(place, &older, SUPERSEDED).await;
        }
        Ok(())
    }

    /// Appends one version of `record` — a tombstone when `deleted` — without
    /// waiting or retiring anything. The batch path: a caller that writes many
    /// records waits once for the last and retires what each one replaced.
    ///
    /// # Errors
    ///
    /// Backend failures on the write.
    pub(super) async fn append(
        &self,
        place: &Place,
        record: &Record,
        observed_at: Option<String>,
        deleted: bool,
    ) -> anyhow::Result<Option<AppendedEvent>> {
        let mut labels = vec![labels::key(&record.key)];
        if let Some(source) = &record.provenance.source {
            labels.push(labels::source(source));
        }
        if let Some(session) = &record.session_id {
            labels.push(labels::session(session));
        }
        let write = KeyedWrite {
            labels,
            inert: place.bookkeeping,
            observed_at,
        };
        self.dialect
            .append_envelope(&place.scope, &record.envelope(deleted), &write)
            .await
    }

    /// Waits until `event` can be read: by listing for bookkeeping, by listing
    /// and then recall for a record the user's recall should find.
    ///
    /// # Errors
    ///
    /// When the event does not become listable within the visibility budget.
    pub(super) async fn wait(&self, place: &Place, event: &AppendedEvent) -> anyhow::Result<()> {
        if place.bookkeeping {
            self.dialect.await_listed(&event.scope, &event.id).await
        } else {
            self.dialect
                .await_readable(&event.scope, &event.id, &event.text)
                .await
        }
    }

    /// Removes `versions` from `place`, best effort. See the module docs.
    pub(super) async fn retire(&self, place: &Place, versions: &[Version], note: &str) {
        let ids: Vec<String> = versions
            .iter()
            .map(|version| version.event_id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        // Best effort by design: the key's next write or removal retires what
        // this one could not.
        let _ = self
            .dialect
            .forget_event_ids(&place.scope, &ids, note)
            .await;
    }

    /// Removes `key` from `place`: a tombstone first, so every read sees it
    /// gone whatever happens next, then the versions behind it. Answers whether
    /// a live version existed.
    ///
    /// # Errors
    ///
    /// Backend failures on the lookup, the tombstone, or its wait.
    pub(super) async fn remove(&self, place: &Place, key: &str) -> anyhow::Result<bool> {
        let versions = self
            .versions(place, &[key])
            .await?
            .remove(key)
            .unwrap_or_default();
        if newest_live(&versions).is_none() {
            return Ok(false);
        }
        let tombstone = Record::plain(key, "");
        if let Some(event) = self.append(place, &tombstone, None, true).await? {
            // A tombstone is never recalled, so the listing is the only read
            // path that has to see it.
            self.dialect.await_listed(&event.scope, &event.id).await?;
        }
        self.retire(place, &versions, REMOVED).await;
        Ok(true)
    }

    /// Removes every event in `place`'s scope, never its children. Answers how
    /// many keys were live.
    ///
    /// # Errors
    ///
    /// Backend failures.
    pub(super) async fn clear(&self, place: &Place) -> anyhow::Result<u64> {
        let events = self.dialect.events_matching(&place.scope, None).await?;
        let live = by_key(&events)
            .values()
            .filter(|versions| newest_live(versions).is_some())
            .count();
        let ids: Vec<String> = events
            .iter()
            .filter_map(|event| event.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        self.dialect
            .forget_event_ids(&place.scope, &ids, CLEARED)
            .await?;
        Ok(live as u64)
    }
}

#[cfg(test)]
#[path = "records_test.rs"]
mod test;
