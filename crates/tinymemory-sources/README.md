# tinymemory-sources

Readers that turn a source into `StoreItem`s: a folder, a single file, a web
page, a GitHub repository, an RSS feed, a Composio toolkit payload, or the
host's local conversation threads. Conversion to markdown and language
detection come from `tinymemory-documents`.

## Layers

| Module | Owns |
| --- | --- |
| `types`, `validation`, `registry`, `reconcile` | the configuration a host persists: `MemorySourceEntry` keyed by `SourceKind`, `MemorySourcePatch`, field rules, the `[[memory_sources]]` TOML registry, Composio reconciliation |
| `readers` | `SourceReader` (list, read, read as a `StoreItem`) and one reader per kind; the SSRF guard (`readers::ssrf`) |
| `fetch` | one URL into a `RawDocument` or a link item, behind the SSRF guard (`network`) |
| `items` | reader output to `StoreItem`s with `MemoryMeta` filled per kind; `collect_items` drives a reader end to end |
| `composio` | toolkit normalisers (Gmail, Slack, GitHub, Linear, Notion, ClickUp) and `payload_items` |
| `error` | the crate `Error`, mapped onto `tinymemory_api::Error` |

## Kinds and metadata

Every item's `meta.source` is `SourceRef { kind, id: Some(entry.id) }`.

| Config kind | `SourceKind` | Item | Metadata |
| --- | --- | --- | --- |
| `folder` | `Folder` | document | `workspace`, `folder` (containing directory), `file_path`, `language`, `observed_at` (mtime), `mime` |
| `file` | `File` | document | as `folder`; `file_item` reads a path with no configured source |
| `web_page` | `Link` | document | `url` |
| `github_repo` | `Github` | document | `repo` (`owner/name`), `commit` (commits), `url` (issues, PRs), `observed_at` |
| `rss_feed` | `Rss` | document | `url` (the entry's link), `observed_at` (published) |
| `composio` | `Composio` | document | `tags = [toolkit]`; payloads add `url`, `observed_at`, `thread_id`, `repo` |
| `conversation` | `Conversation` | conversation | `workspace`, `thread_id`, `turns`, `observed_at` (last turn) |

## Folder selection

With a glob, a folder source takes exactly the matching files. Without one it
takes markdown, plain text and source code (`is_default_candidate`). Either way
it skips hidden files and directories and `target`, `node_modules`,
`__pycache__` and `venv`, never follows symlinks while walking, refuses files
over `FOLDER_FILE_SIZE_CAP_BYTES` (10 MiB), and confines reads to the folder
root (`ensure_within_base`). A relative path is anchored on the workspace, not
the process working directory.

## Who decides when

`readers::reader_for` hands out only the local readers (folder, file,
conversation), which are safe to drive on a timer. Network readers are
constructed explicitly, or through `reader_for_request` for an explicit user
request. Scheduling, credentials, OAuth and egress budgets stay with the host.

## Features

- `network` — the GitHub, RSS and web-page readers, `fetch`, and the SSRF
  guard. Off by default, so a host that only reads local sources links no HTTP
  stack.
