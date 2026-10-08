# sources

`tinymemory_integrations::sources` (feature `sources`):
readers that turn a source into `StoreItem`s — a folder, a single file, or the
host's local conversation threads. Conversion to markdown and language
detection come from the sibling [`documents`](../documents/README.md) module.
Architecture overview:
[`docs/architecture/integrations.md`](../../../../docs/architecture/integrations.md).

Where a host stores its configured sources, and how it edits them, is the
host's business: this module reads a `MemorySourceEntry` it is handed and
checks it with `MemorySourceEntry::validate`, nothing more.

## Layers

| Module | Owns |
| --- | --- |
| `types` | the configuration a host persists: `MemorySourceEntry` keyed by `SourceKind`, its field rules, and the reader output types (`SourceItem`, `SourceContent`, `ContentType`) |
| `readers` | `SourceReader` (list, read, read as a `StoreItem`) and one reader per kind, each in its own module directory; `local_file` holds the shared size-capped read and the path-containment guard |
| `items` | reader output to `StoreItem`s with `MemoryMeta` filled per kind; `collect_items` drives a reader end to end |
| `error` | the module `Error`, mapped onto `tinymemory_api::Error` |

## Kinds and metadata

Every item's `meta.source` is `SourceRef { kind, id: Some(entry.id) }`.

| Config kind | `SourceKind` | Item | Metadata |
| --- | --- | --- | --- |
| `folder` | `Folder` | document | `workspace`, `folder` (containing directory), `file_path`, `language`, `observed_at` (mtime), `mime` |
| `file` | `File` | document | as `folder`; `file_item` reads a path with no configured source |
| `conversation` | `Conversation` | conversation | `workspace`, `thread_id`, `turns`, `observed_at` (last turn) |

## Folder selection

With a glob, a folder source takes exactly the matching files. Without one it
takes markdown, plain text and source code (`is_default_candidate`). Either way
it skips hidden files and directories and `target`, `node_modules`,
`__pycache__` and `venv`, never follows symlinks while walking, refuses files
over `FOLDER_FILE_SIZE_CAP_BYTES` (10 MiB), and confines reads to the folder
root (`readers::local_file::ensure_within_base`). A relative path is anchored on the workspace, not
the process working directory.

## Who decides when

`readers::reader_for` hands out the folder, file and conversation readers, all of
which read local state only. Scheduling stays with the host.

## Features

- `sources` — the readers, `items` and `types`; implies `documents`. Links no
  HTTP stack.
