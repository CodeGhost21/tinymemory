# tinymemory-import

Reads a legacy (v1, embedded TinyCortex) workspace and yields TinyMemory v2
`StoreItem`s, resumably. The v1 engine that wrote the store is not linked: the
importer reads its SQLite files directly with `rusqlite`, opened read-only, and
chunk bodies with `std::fs`. It never writes to the legacy workspace.

The facade exposes this crate behind its `legacy-import` feature. The crate
itself has no features: being the legacy reader is its whole job.

## Surface

| Item | Purpose |
| --- | --- |
| `LegacyWorkspace::open(path)` | Detects a v1 store or refuses with a typed error. |
| `LegacyWorkspace::items()` / `items_from(&Checkpoint)` | Streams `Result<ImportedItem>` from the start or after a checkpoint. |
| `Items::with_page_size(n)` | Keys fetched per query (default `DEFAULT_PAGE_SIZE`, 256). Does not affect output. |
| `ImportedItem { item, checkpoint }` | An item and the checkpoint to persist once it is stored. |
| `Checkpoint` | Last yielded key per section; `to_json` / `from_json` for the host to persist. |
| `Error` / `Result` | `NotFound`, `NotLegacy`, `Sqlite`, `Io`, `Json`. |

## Detection

`open(path)` requires `<path>/memory/memory.db` to be a SQLite database with
the `memory_docs`, `episodic_log` and `user_profile` tables and the columns
the importer reads. A missing path is `NotFound`; anything else that is not a
v1 store (a file, no `memory.db`, a non-SQLite file, a different schema) is
`NotLegacy` with the reason. Columns that later v1 migrations added are probed
with `pragma_table_info` and used when present: `memory_docs.logical_namespace`,
`episodic_log.tool_calls_json`, `user_profile.state` / `user_state` / `class`
/ `evidence_refs_json`, and `mem_tree_chunks.content_path`.

`memory_tree/chunks.db` is optional. If it is absent, not SQLite, or has no
usable `mem_tree_chunks` table, the chunk section is skipped silently.

Per-profile stores (`memory-<id>/memory.db`) are not read; open each one as its
own workspace if needed.

## Mapping

Every item gets `meta.source = { kind: Import, id: <legacy id> }` and
`meta.workspace = <canonical workspace path>`.

| Section (in order) | Legacy rows | Key / legacy id | v2 item |
| --- | --- | --- | --- |
| documents | `memory_docs` in document namespaces | `document_id` / `memory_docs:<id>` | `Document` |
| chunks | `mem_tree_chunks` grouped by `(source_kind, source_id)` | the pair / `mem_tree_chunks:<kind>:<id>` | `Conversation` for `chat`, else `Document` |
| conversations | `episodic_log` grouped by `session_id` | `session_id` / `episodic_log:<id>` | `Conversation` |
| learnings | `memory_docs` in `learning:*` and `global` | `document_id` / `memory_docs:<id>` | `Learning` |
| profile | live `user_profile` facets | `facet_id` / `user_profile:<id>` | `Learning(Preference)` |

### `memory_docs` namespaces

A row's logical namespace is `logical_namespace` when that column exists and
is set, else `namespace` with the sanitiser undone for the known v1 section
prefixes (`learning_style` → `learning:style`; only the first `_` can be
restored). Then:

- `learning:<class>` or `learning` → learnings section;
- `global` → learnings section;
- `event` / `event:*` → **skipped**. These are raw event payloads the v1
  engine kept for bookkeeping; what they meant already lives in the episodic
  log and in the learnings distilled from them, and as JSON blobs they would
  only add noise to recall;
- anything else (`document:*`, `source:*`, `conversation:*`, custom
  `Memory::store` namespaces) → documents section.

Rows with blank content are skipped in every section.

### Documents

`title` from `title` (none when blank), body = `content`, `tags` = the strings
in `tags_json` plus `ns:<logical namespace>`, `observed_at` = `updated_at`,
`url` and `mime` from `metadata_json` when present.

### Chunks

Chunks of one source are ordered by `(seq_in_source, id)`. A chunk's text is
the file `memory_tree/content/<content_path>` when the column is set, the path
is a plain relative path, and the file exists; otherwise the stored preview.
A `chat` source becomes a conversation of one `User` turn per chunk (chat
chunks are transcripts of host channels, whose speakers are people), with
`thread_id` = `source_id` and `turns` = `0..=n-1`. Every other kind becomes a
document whose body is the chunks joined by blank lines. Tags are the union of
the chunks' `tags_json` plus `source_kind:<kind>`; `observed_at` is the latest
chunk timestamp. A chunk source may duplicate a `memory_docs` document; the
two carry different legacy ids, so an engine stores both.

### Conversations

Turns are ordered by `(timestamp, id)`; blank turns are dropped and a thread
with none left is skipped. Roles map case-insensitively: `user`/`human` →
`User`; `assistant`/`ai`/`agent`/`bot`/`model` → `Assistant`;
`system`/`developer` → `System`; `tool`/`function`/`tool_result` → `Tool`;
**anything else → `User`** (v1 itself wrote only `user` and `assistant`, so an
unknown role came from a host channel, whose speaker is a person). `at` =
`timestamp`; `tool_calls` from `tool_calls_json` (an array of calls, a single
call, or `{"tool_calls": [...]}`, naming the tool as `name`, `tool`,
`tool_name` or `function.name`), dropped when unparseable. `thread_id` =
`session_id`, `turns` = `0..=n-1`, `observed_at` = the last turn's time.
`lesson` and `cost_microdollars` are not imported.

### Learnings

A `learning:<class>` row's content is a JSON `LearningCandidate`. It becomes
`"<key>: <value>"` (a non-string value as compact JSON), `confidence` =
`initial_confidence` clamped to `0..=1` (0.5 when absent), `evidence` = the
`evidence` JSON, `observed_at` = the candidate's `observed_at` or else
`updated_at`, `tags` = `[class]`. The kind follows the class:

| Class | Kind |
| --- | --- |
| `style`, `channel` | `Preference` |
| `identity` | `Fact` |
| `tooling` | `Procedure` |
| `veto` | `Correction` |
| `goal`, unknown | `Other` |

Content that is not a candidate (not JSON, or no `key`/`value`) becomes
`Learning { kind: Other, confidence: 0.5, text: content }` tagged with the
namespace's class. A `global` row becomes
`Learning { kind: Fact, confidence: 0.5, text: content }` tagged `global`:
v1 kept always-relevant statements there.

`kv_global` and `kv_namespace` are **not imported**: v1 used them for engine
and host bookkeeping values, not for anything recall should surface.

### Profile

Facets with `state = 'dropped'` or `user_state = 'forgotten'` (when those
columns exist) or a blank value are skipped. The rest become
`Learning(Preference)` with text `"<key>: <value>"`, `confidence` from the
column (clamped), `evidence` = `evidence_refs_json`, `observed_at` =
`last_seen_at`, `tags` = `[facet_type, class]` (class when set).

## Ordering and resumption

Sections run in the fixed order above; within a section keys ascend in SQLite
`TEXT` order. Each `ImportedItem` carries the checkpoint covering it and
everything before it. `items_from(&checkpoint)` yields exactly what `items()`
yields after that item, provided the legacy store did not change in between
(the cursor is a key, so a row inserted later below an already-passed key is
not seen). The iterator fetches one page of keys per query, so memory is
bounded by the page size and, for a conversation or chunk source, by that one
thread or source. After an error it yields nothing more; resume from the last
persisted checkpoint.
