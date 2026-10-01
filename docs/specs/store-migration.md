# Copying a store between drivers

Status: Implemented. Owner: tinymemory maintainers. Tracks openhuman#6718.

## Problem

`migrate::copy` moves keyed records through the mandatory portability family,
and nothing else. A host that switches a user from the embedded engine to
hosted memory, or back, leaves behind:

- the titles, tags, source types, priorities and metadata of documents — the
  export carries a document's content as a keyed record, not its details;
- the goals document and the learned profile, which live in families of
  their own;
- every past conversation: the episodic family can record a turn and read a
  session back, but it cannot enumerate what it holds;
- everything ingested into the summary tree, which the export does not see.

## Goals and non-goals

Goals:

- Copy each of those between any two drivers that serve the families
  involved, engine-neutrally, from the facade's `migrate` module.
- Keep every step idempotent, so a copy that stopped part-way is finished by
  running it again, and non-destructive towards the source.
- Keep turn ids where the target can, and rewrite every reference to a turn
  the target had to move.

Non-goals:

- Copying derived data — summary nodes, chunk and entity embeddings, the
  engine's graph. A target derives its own; vectors from another embedding
  space are not comparable.
- Deciding whether a user may copy, or what it costs. That is host policy.

## Proposed behavior

### A new family: `EpisodicPortability`

Contract 4.3 adds `Capability::EpisodicPortability` (`episodic_portability`)
and `MemoryEpisodicPortability`, reached through
`MemoryProvider::as_episodic_portability`. A new family, not new members of
`Episodic`: negotiation is per family, so new members there would be a major
bump. Two bus members are appended to the wire table: `ExportEpisodic` (slot
144) and `ImportEpisodic` (slot 145).

- `export_episodic(part, cursor, limit)` returns one `EpisodicExportPage` of
  an `EpisodicPart` — `turns`, `segments`, `events` or `segment_embeddings` —
  in an order the driver keeps stable from page to page. A page holds at most
  `limit` records and may hold fewer; only a missing `next_cursor` ends a walk.
  A zero limit or a cursor the driver did not issue for that part is `Invalid`.
- `import_episodic(records)` writes `EpisodicRecords` of one part:
  - a turn keeps its id when the target holds no turn there; a turn the target
    holds exactly is skipped; one the target holds exactly under another id —
    a turn an earlier copy moved — is skipped and reported in `remapped` at
    that id; otherwise it takes a fresh id from above every turn recorded so
    far (the present, in microseconds), so it cannot meet a turn still to come
    in the same copy, and is reported in `remapped`;
  - a segment is written whole, replacing the one with its id;
  - an event replaces the one with its id;
  - a segment embedding replaces the one for its segment and model signature;
  - a refused record counts as failed, with a reason naming it, never its
    content; a backend failure fails the call.

Drivers:

| Driver | Export | Import |
| --- | --- | --- |
| Embedded (TinyCortex) | primary-key range scans over `episodic_log`, `conversation_segments`, `event_log`, `segment_embeddings`; pages also stop at 4 MiB | turns sanitized as `insert_turn` sanitizes them, and checked against the stored, sanitized text |
| Hosted (TinyHumans) | folds the part's bookkeeping scope per page, ordered by key; pages also stop at 4 MiB | appends without per-record waits, pauses through rate limiting as a record import does, waits once per batch, retires replaced versions |

The guard admits an export as a read and an import as a write, and redacts
turn and event text as `insert_turn` and `insert_event` do.

### `migrate::copy_all`

```rust
pub async fn copy_all(
    from: &dyn MemoryProvider,
    to: &dyn MemoryProvider,
    options: &CopyOptions,
    progress: impl FnMut(CopyProgress),
) -> anyhow::Result<CopyAllReport>;
```

Runs `copy`, then each `MigrateStep` in order. A step whose family one side
does not serve reports `skipped_because` and moves on.

| Step | Needs | Does |
| --- | --- | --- |
| `records` | mandatory | `copy` |
| `documents` | `Documents` both sides | re-puts each document whose details are not the defaults (title = key, `chat`, `medium`, no tags, empty metadata), unless the target holds it with the same details; namespaces starting with a `skip_namespace_prefixes` entry (default `source:`, `sources/`, where synced items live) are left out |
| `goals` | `Goals` both sides | appends the source's goals after the target's own, skipping a goal the target already states (case-insensitive) and giving a used id a free `g{n}` |
| `profile` | `Profile` both sides | upserts each facet unless the target holds the same key seen as recently or later |
| `episodic` | `EpisodicPortability` both sides | turns, then segments, events and embeddings, rewriting every reference to a moved turn — one lookup per reference, never chained |
| `content` | `Chunks` on the source, `Ingest` on the target | re-sends each logical source's chunks, oldest first: a document joined back into one `ingest_document`, mail by message through `ingest_email` (`ingest_chat` where a target does not split mail), chat by message through `ingest_chat`; sources whose id starts with a `skip_source_prefixes` entry are left out; off when `replay_content` is false |

The chunks do not record a source's provider or taint, so the replay infers
them: the provider from the source id's prefix (`gmail`, `notion`, …), the
taint as `external_sync` for everything but the agent's own conversations
(`conversations:…`).

`CopyAllReport` carries the `copy` report and one `StepReport` per later step:
`read`, `written`, `unchanged`, `failed`, and at most 20 reasons.

## Invariants and constraints

- No step deletes from the source.
- A second `copy_all` over the same pair writes nothing in `documents`,
  `goals`, `profile` or `episodic`; `content` relies on the target's own
  ingest dedupe (the embedded engine's source gate, hosted memory's
  content-addressed message keys).
- An import never overwrites a different turn under the id it carries.
- Failure reasons name records, never their content.

## Acceptance criteria

- Embedded-to-embedded: the full record moves, a colliding turn is moved and
  reported, and a second pass imports nothing
  (`tinymemory-tinycortex/tests/full_provider_conformance.rs`).
- Hosted-to-hosted over the `/memory/*` double: every part round-trips
  unchanged, ids survive, a colliding turn moves once
  (`tinymemory-remote/src/cortex_provider/families/episodic_portability_test.rs`).
- `copy_all` between fakes: every step moves what the target lacks, references
  follow moved turns, the replay rejoins documents in sequence and honours
  skip prefixes, and a rerun writes nothing (`tinymemory/src/migrate/test_steps.rs`).

## Open questions

None.
