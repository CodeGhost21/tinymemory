# sources

`tinymemory_integrations::sources` (features `sources` and `sources-network`):
readers that turn a source into `StoreItem`s — a folder, a single file, a web
page, a GitHub repository, an RSS feed, a Composio toolkit payload, or the
host's local conversation threads. Conversion to markdown and language
detection come from the sibling `documents` module.

Where a host stores its configured sources, and how it edits them, is the
host's business: this module reads a `MemorySourceEntry` it is handed and
checks it with `MemorySourceEntry::validate`, nothing more.

## Layers

| Module | Owns |
| --- | --- |
| `types` | the configuration a host persists: `MemorySourceEntry` keyed by `SourceKind`, its field rules, and the reader output types (`SourceItem`, `SourceContent`, `ContentType`) |
| `readers` | `SourceReader` (list, read, read as a `StoreItem`) and one reader per kind, each in its own module directory; `local_file` holds the shared size-capped read and the path-containment guard |
| `fetch` | one URL into a `RawDocument` or a link item (`sources-network`); the RSS and web-page readers fetch through it with their own body caps |
| `fetch::ssrf` | the SSRF guard: scheme and host policy, one address classifier for literal and resolved addresses, a public-only DNS resolver, per-hop redirect checks, and a capped body reader |
| `items` | reader output to `StoreItem`s with `MemoryMeta` filled per kind; `collect_items` drives a reader end to end |
| `composio` | toolkit normalisers (Gmail, Slack, GitHub, Linear, Notion, ClickUp), the `fields::pick_str` lookup they share, and `payload_items` |
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
root (`readers::local_file::ensure_within_base`). A relative path is anchored on the workspace, not
the process working directory.

## Who decides when

`readers::reader_for` hands out only the local readers (folder, file,
conversation), which are safe to drive on a timer. Network readers are
constructed explicitly, or through `reader_for_request` for an explicit user
request. Scheduling, credentials, OAuth and egress budgets stay with the host.

## Fetching

Every network fetch of a user-configured URL goes through `fetch` and its
SSRF guard. A hostname is checked as text (private and reserved IP literals,
`localhost`, `.local`/`.internal`, single-label names), its resolved addresses
are checked again by the client's resolver, which pins the connection to an
address it has vetted, and every redirect hop is re-checked. Bodies are read
against a cap while streaming: 32 MiB for `fetch_url`, 10 MiB for a web page,
5 MiB for a feed. Failures are typed — `Invalid` for a refused or malformed
URL, `Unreachable`, `Upstream` for a failure status, `TooLarge`.

Page titles and feed text are decoded with the `documents::html` helpers, so
named and numeric entities decode the same way everywhere.

## Features

- `sources` — the local readers, `items`, `composio` and `types`. Links no
  HTTP stack.
- `sources-network` — adds the GitHub, RSS and web-page readers, `fetch`, and
  the SSRF guard.
