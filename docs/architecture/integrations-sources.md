# Integrations: sources

The `sources` module of `tinymemory-integrations` (feature `sources`) turns a configured source into `StoreItem`s. Overview of the
crate: [integrations.md](integrations.md). Module README:
[`src/sources/README.md`](../../crates/tinymemory-integrations/src/sources/README.md).

The module reads what it is handed. Where a host stores its sources, when it
syncs them, credentials, OAuth and egress budgets all stay with the host.

## Configuration: MemorySourceEntry and SourceKind

`MemorySourceEntry` is the configuration a host persists (it derives serde and
`JsonSchema`). Its `kind` (`SourceKind`, snake_case on the wire) selects which
optional fields are required; `validate()` checks them.

| `SourceKind` | Required | Other fields | Item's `SourceKind` |
| --- | --- | --- | --- |
| `folder` | `path` | `glob` | `Folder` |
| `file` | `path` | | `File` |
| `conversation` | | | `Conversation` |

Every entry also needs a non-blank `id` (no `:` or control characters) and a
non-empty `label`, and carries `enabled` and optional sync-budget fields
(`max_tokens_per_sync`, `max_cost_per_sync_usd`, `sync_depth_days`) that the
host's sync runner interprets. An empty string counts as missing. Failure is
`Error::Invalid` naming the first failing rule.

## Readers

`SourceReader` is the narrow trait: `kind`, `list_items(source, workspace)`,
`read_item(source, item_id, workspace)` and `read_store_item(source, item,
workspace, converter)`. The default `read_store_item` reads the content and maps
it through `items::content_item`; local readers override it to work from raw
bytes so a bound converter can handle PDF or DOCX.

`readers::reader_for(kind)` returns the folder, file or conversation reader; every
reader is local.

| Reader | Items | Notes |
| --- | --- | --- |
| `FolderReader` | one per selected file; id is the folder-relative slash path | With a `glob`, exactly the matches (compiled to a regex over the relative path). Without one, markdown, plain text and source code (`is_default_candidate`). Skips hidden files and directories and `.git`, `.hg`, `.svn`, `target`, `node_modules`, `__pycache__`, `venv`; never follows symlinks; refuses files over `FOLDER_FILE_SIZE_CAP_BYTES` (10 MiB). A relative `path` is anchored on the workspace. |
| `FileReader` | exactly one, id is the file name | Same size cap. `FileReader::read_path` reads a path with no configured source. |
| `ConversationReader` | one per `<workspace>/threads/<id>.json` | Threads are `{title, messages: [{role, content, created_at?}]}`. Messages with blank text or an unknown role are dropped. Timestamps: RFC 3339, or epoch seconds or milliseconds. The id may not contain separators or `..`. |

### local_file and ensure_within_base

`readers::local_file` is shared by the folder and file readers: a size-capped
whole-file read into `LocalFile { path, id, bytes, modified }`, and the
path-traversal guard `ensure_within_base(base, target)`. The guard
canonicalises both paths (resolving symlinks and `..`) and returns
`Error::PathEscape("path traversal denied")` when the target is outside the
base, or `Error::Io` when either cannot be canonicalised. The folder reader
applies it on every read; the conversation reader applies it within the
threads directory.

## Items mapping

`items` maps reader output to `StoreItem`s. Every item's `meta.source` is
`SourceRef { kind, id: Some(entry.id) }`, and every document body is markdown
(local files through the host's converter, reader bodies through
`markdown_from_text`).

| Kind | Item | Metadata filled |
| --- | --- | --- |
| folder, file | document | `workspace`, `folder`, `file_path` (canonical), `language`, `observed_at` (mtime), `mime` |
| conversation | conversation | `workspace`, `thread_id`, `turns` (`0..=n-1`), `observed_at` (last turn, else mtime) |

`collect_items(reader, entry, workspace, converter)` lists and reads every
item, returning `Collected { items, skipped }`. One bad item lands in `skipped`
with its error and the pass continues; only a listing failure is an `Err`.
Other entry points: `file_item` (a path with no source), `conversation_item`,
`content_item` and `items::local_file_item`.
