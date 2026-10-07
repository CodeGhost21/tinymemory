# CortexDB: no local path leaves the machine

A desktop host fills `MemoryMeta` with local paths: `file_path` is the file a
document was read from (`/Users/<name>/Documents/…/pricing.pdf`), `folder` its
folder, and `workspace` often the agent's working folder. A path names the
person (their home folder) and what they keep, so the CortexDB engine never
sends one, on either wire. Every write goes through one place
(`Envelope::new` in `src/cortex/envelope/mod.rs`), so the rule holds for the
v3 envelope parts (`tm:e:<NN>:`), the v2 JSON text, and the readable labels.

## What is sent

| Field | Sent as | Filters match |
| --- | --- | --- |
| `file_path` | the file's name (`pricing.pdf`), split at `/` or `\` | the full path or a folder above it, by digest; or the name |
| `folder` | not sent | the folder or a folder above it, by digest |
| `workspace`, absolute (`/…`, `~…`, `\\…`, `C:…`) | not sent | the full value, by digest |
| `workspace`, a logical id (`team-handbook`) | unchanged | as before |

The readable label is `file:<name>`. The title and the body are the
converter's output, which names the file at most, never its folders. The
lookup labels were digests already (`tm:w:` is still written for an absolute
workspace, from the value the host gave).

## Filters still match

The envelope carries what the dropped values would be matched by, so a
`MetaFilter` behaves as it did:

- `"ws"`: the digest of an absolute `workspace`. A workspace filter matches
  when its digest is equal.
- `"fp"` and `"fd"`: the digests of `file_path` and of `folder`, and of every
  prefix of each that ends before a `/`. These are exactly the values a
  `MetaFilter` path filter (an exact path, or a folder above it at a `/`) can
  name and still match, so an exact file or a folder prefix matches by digest,
  and a prefix that stops inside a folder name does not.

A path filter also matches what the envelope kept, so filtering by a file's
name (what reads give back) works. Digests are the 64-bit `labels::digest`;
a collision could admit an extra row, as for the lookup labels.

## What reads give back

Reads return the file's name as `file_path`, and no `folder` or absolute
`workspace`. This is a deliberate exception to the round trip
`tinymemory_api::conformance` checks, whose fixtures use logical workspace
ids and relative paths, which this engine keeps meaningful. A host that needs
the full path (to open a file, or re-read a folder) keeps it locally; nothing
in the engine needs it.

## Events written before

Events written by earlier versions still carry full paths in their envelope
and their `file:` label. Re-storing such an item does not rewrite it: the
item id is unchanged, and the store finds the item's events by it and writes
nothing (every store but the Direct turn-logging hot path looks up first). Clearing them means
forgetting each such item and storing it again.
