# CortexDB lookup labels and digests

The digest labels every CortexDB event carries for server-side narrowing.
The rest of the wire is in [cortex-wire.md](cortex-wire.md).

Each event carries up to eight `context.labels`, each `tm:<tag>:` followed by
the first 16 lowercase hex digits (64 bits) of the SHA-256 of the value:

| Label | Value hashed | On |
| --- | --- | --- |
| `tm:i:` | the item id | every event |
| `tm:t:` | `meta.thread_id`, as stored (see below) | when set |
| `tm:s:` | `meta.source.id`, as stored (see below) | when set |
| `tm:r:` | `meta.repo` | when set |
| `tm:w:` | `meta.workspace` (kept even when the envelope drops it) | when set |
| `tm:a:` | `meta.agent_id` | when set |
| `tm:l:` | `meta.language` | when set |
| `tm:k:` | the source kind (`meta.source.kind`) | every event |

A label holds a **digest**, not the value, because the engine splits a label
filter on commas and bounds a label's length, and a path or source id may be
long or hold a comma.

Reads use labels two ways:

- **Item lookup.** Replay detection, `get`, conversation assembly and forget
  by id ask the listing for `tm:i:<digest(id)>` labels. Because a label is a
  digest, every hit is re-checked against the envelope's real `id`.
- **Narrowing.** A read whose filter has a labelled field sends **one** label
  filter to narrow server-side: the first set field of thread, source id, repo,
  workspace, agent, language (in that order, most selective first), else the
  filter's source kinds (several `tm:k:` labels, which the engine reads as
  any-of). Only labels of one field may be sent together, since the engine
  keeps events carrying *any* of the labels.

The label only ever narrows. Every reader **always** re-applies the full
`MetaFilter` to the decoded envelope, so a digest collision costs a wasted row
and never a wrong answer. `folder` and `file_path` match as prefixes, which a
label cannot, so they are filtered client-side only. No local path is sent:
see [cortex-local-paths.md](cortex-local-paths.md).

## Readable labels

Beside the lookup labels, a v3 event carries labels a person browsing the
events can read. They are never filtered on.

| Label | When |
| --- | --- |
| `kind:<kind>` | every event |
| `agent:<agent_id>` | `meta.agent_id` is set |
| `thread:<thread_id>` | `meta.thread_id` is an app thread, `thread-<uuid>` (lowercase hex). A channel's thread (`channel:…`) never gets one: it can name the person at the other end |
| `file:<name>`, `page:<n>[-<m>]`, `section:<title>` | a document, a piece |

## Phone numbers are never sent

A channel's thread id, and the source id a host derives from it, can hold the
sender's phone number (`channel:whatsapp_+15551234567_…`). An id **holds a
phone number** when, after removing every canonical UUID
(`8-4-4-4-12` hex digits), it has a run of at least seven digits, where spaces,
`-`, `.`, `(`, `)` and `+` between the digits do not break the run
(`tinymemory_api::holds_phone_number`). Such a `thread_id` or `source.id` is
stored, in the envelope and so in its `tm:t:`/`tm:s:` label, as
`tm-redacted:` followed by the first 32 hex digits of its SHA-256
(`tinymemory_api::redacted_id`). An id that is already redacted is left as it
is. An email address is not a phone number and is stored unchanged.

Reads return the redacted form. Filters still match the original id:

- `MetaFilter` and the tools' "leave out the live thread" step compare with
  `tinymemory_api::same_id`, which accepts the id itself or its redacted form.
- Narrowing by such an id sends the digests of both forms, so events written
  before this change, which hold the id itself, are found as well.

Events written before are not rewritten.
