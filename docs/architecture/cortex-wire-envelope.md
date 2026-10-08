# CortexDB engine: the envelope and CortexDB behaviours

The second half of the wire reference: how an item is wrapped as an event
envelope, the lookup labels and digests, and the CortexDB behaviours the engine
is shaped around. Part of the CortexDB engine docs: [overview and
transport](cortex.md) · [endpoints and scope layout](cortex-wire.md) ·
[operation flows](cortex-flows.md).

## The envelope (v3, and v2)

A learning is one event; a conversation is one event per turn, appended in
order. A document is one event unless its envelope would pass 256 KiB; then it
is one event per piece (see [cortex-chunks.md](cortex-chunks.md)).
CortexDB's experience schema is closed (an unknown field is a 422), and its
`context.labels` are the app-metadata extension point: stored verbatim,
returned on listings and on recall's `layers.events`, and not counted toward
the 1 MiB text limit.

**v3 (written now).** An event's `content.text` is the item's own text: the
document body or piece, the turn's text, or the learning's statement. This is
what CortexDB extracts from and splits for search at sentence boundaries,
which a JSON text lacks. Its `context.labels` are, in order:

- the lookup labels (below), `tm:i:` first;
- readable labels, each at most 256 bytes or left out: `kind:<kind>`,
  `file:<name>` (the file's name only), and a piece's `page:<n>` or `page:<first>-<last>` and
  `section:<title>`. Never filtered on; no `lang:` label is ever written
  (CortexDB reserves it);
- the **envelope parts**: the envelope below with `"v": 3` and an empty
  `text`, as compact JSON cut at char boundaries into slices of at most 240
  bytes, each written `tm:e:<NN>:<slice>`. `NN` is the part's number as a
  decimal integer, zero-padded to two digits (`00`, `01`, …); an event has
  at most 64 labels, so there are at most 63 parts. A reader orders parts by
  that number, never by the label's text, and needs every number from 0 up.

An event whose text is empty (CortexDB requires message text), or whose
labels would be more than 64, is written as v2 instead.

**v2 (every event before v3).** `content.text` is the whole envelope as JSON,
with `"v": 2`. Both layouts can live in one scope, and every reader takes
either: an event whose labels hold envelope parts numbered `0..n` that join to
a v3 envelope is v3, with its text as the envelope's `text`; otherwise its
text is tried as a v2 envelope. The envelope:

```json
{ "v": 2, "id": "<40-hex fingerprint>", "kind": "conversation",
  "text": "<body | turn text | statement>",
  "meta": { "...": "the item's whole MemoryMeta, on every event" },
  "title": "...", "mime": "...",
  "learning_kind": "preference", "confidence": 0.8, "evidence": "...",
  "turn": { "index": 0, "count": 3, "role": "user", "at": "...", "tool_calls": [] } }
```

| Field | Present on | Meaning |
| --- | --- | --- |
| `v` | every event | `3` in envelope parts, `2` in a v2 text; any other value is ignored |
| `id` | every event | the item id, `StoreItem::fingerprint()` (a content digest) |
| `kind` | every event | `document`, `conversation` or `learning` |
| `text` | every event | body, the turn's text, or the learning's statement |
| `meta` | every event | the whole `MemoryMeta`, including the namespace |
| `title`, `mime` | documents, when set | |
| `learning_kind`, `confidence`, `evidence` | learnings (`evidence` when set) | |
| `turn` | conversation turns | `index` (0-based), `count`, `role`, `at`, `tool_calls` |
| `chunk` | pieces of a chunked document only (a document written whole has none) | `index` (0-based), `count`; optional `pages` (`[first, last]`, only when the text marks pages) and `section` (only when the piece starts under a heading) |

An event that is neither is someone else's and is ignored by every reader. A
v2 text is first tried as written, then without a leading `[role] ` (an older
recall rendered one; 0.10.3 and 0.10.4 return the stored text in
`layers.events`, the marker only in `context_block`). A document whose body is still an unresolved URI is
refused at write time as `Error::InvalidRequest`.

Rebuilding an item from envelopes: a learning, or a document written whole,
takes the first envelope; a chunked document orders its pieces by `index`,
keeps one per index and concatenates their text (the pieces are contiguous
slices, so all of them give back the body exactly, and one gives that piece);
a conversation orders turns by `index` and keeps one per index (so a
duplicated or re-written turn does not repeat, and a turn that was never
written is absent). A learning with no `learning_kind` reads back as `Other`.

## Lookup labels and digests

The labels every event carries for server-side narrowing, and the digests
behind them, are in [cortex-labels.md](cortex-labels.md).

## CortexDB behaviours the engine is shaped around

Each was measured against a live CortexDB and was wrong in the first adapter.
The loopback doubles reproduce all of them (see [testing](testing.md)).

- **Append-only.** There is no update route. A reused body `idempotency_key`
  with the same body is a replay for 24 hours, and with another body a 409
  (which this crate's keys, derived from the body, never produce).
  Forget by `memory_ids` releases the keys of what it removes (measured on
  0.10.4; the v1 adapter's notes said the opposite, on an unrecorded build).
- **Accepted is not readable.** An append answers `202` and indexes afterwards.
  The status route and the lifecycle stream are not readiness signals, so the
  engine waits on the listing and on recall itself (see
  [flows](cortex-flows.md#waiting-for-a-write-to-be-readable)).
- **The listing emits every event twice**, and `limit` counts the copies.
- **Unknown query parameters are ignored.**
- **Recall and the listing return different bytes** (`[role] {...}` versus the
  stored text).
- **The forget selector field is `memory_ids`**, and anything else reads as
  empty, which means the whole scope.
- **TinyHumans** rate-limits a user to 300 requests a minute and has no bulk,
  `?wait=indexed` or health route.
