# Integrations: sources

The `sources` module of `tinymemory-integrations` (features `sources` and
`sources-network`) turns a configured source into `StoreItem`s. Overview of the
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
| `web_page` | `url` | `selector` | `Link` |
| `github_repo` | `url` | `branch`, `paths`, `max_commits`, `max_issues`, `max_prs` (default 1000 each) | `Github` |
| `rss_feed` | `url` | `max_items` (default 50) | `Rss` |
| `composio` | `toolkit`, `connection_id` | | `Composio` |

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

`readers::reader_for(kind)` returns only the local readers (folder, file,
conversation), which are safe to drive on a timer. The network kinds return
`None`, meaning "route through the host's sync runner". With
`sources-network`, `reader_for_request(kind)` returns a reader for every kind,
for a host servicing an explicit user request, never a polling loop.

| Reader | Items | Notes |
| --- | --- | --- |
| `FolderReader` | one per selected file; id is the folder-relative slash path | With a `glob`, exactly the matches (compiled to a regex over the relative path). Without one, markdown, plain text and source code (`is_default_candidate`). Skips hidden files and directories and `.git`, `.hg`, `.svn`, `target`, `node_modules`, `__pycache__`, `venv`; never follows symlinks; refuses files over `FOLDER_FILE_SIZE_CAP_BYTES` (10 MiB). A relative `path` is anchored on the workspace. |
| `FileReader` | exactly one, id is the file name | Same size cap. `FileReader::read_path` reads a path with no configured source. |
| `ConversationReader` | one per `<workspace>/threads/<id>.json` | Threads are `{title, messages: [{role, content, created_at?}]}`. Messages with blank text or an unknown role are dropped. Timestamps: RFC 3339, or epoch seconds or milliseconds. The id may not contain separators or `..`. |
| `WebPageReader` | one: the page URL | With a CSS `selector`, only the text of matching elements (plain text; only the last compound of a descendant chain is honoured). Otherwise the whole page as markdown. 10 MiB body cap. |
| `RssReader` | one per feed entry (RSS or Atom), up to `max_items` | The parsed feed is cached for 60 seconds so a list-then-read pass downloads it once. 5 MiB cap; non-UTF-8 bodies are refused. |
| `GithubReader` | `commit:<sha>`, `issue:<n>`, `pr:<n>` | See below. |
| `ComposioReader` | one: the connection | A placeholder; see Composio. |

### local_file and ensure_within_base

`readers::local_file` is shared by the folder and file readers: a size-capped
whole-file read into `LocalFile { path, id, bytes, modified }`, and the
path-traversal guard `ensure_within_base(base, target)`. The guard
canonicalises both paths (resolving symlinks and `..`) and returns
`Error::PathEscape("path traversal denied")` when the target is outside the
base, or `Error::Io` when either cannot be canonicalised. The folder reader
applies it on every read; the conversation reader applies it within the
threads directory.

### GitHub transports

The reader pulls **project activity**, not source code, from
`https://github.com/<owner>/<repo>` (extra path segments such as `/tree/main`
are rejected). It combines three transports:

- **Commits:** a bare clone under `<workspace>/git_cache/<owner>/<repo>.git`,
  fetched with an explicit refspec and listed with `git log`, honouring
  `branch` and `paths`. `git` must be on `PATH`. If the clone fails, it falls
  back to the commits API.
- **Issues and pull requests:** `gh api` when the `gh` CLI is available
  (authenticated, higher rate limit; probed once per process), otherwise the
  unauthenticated REST API at `api.github.com`. The list pass caches full rows
  so reads do not refetch.
- Limits: `max_commits`, `max_issues`, `max_prs` per sync, default 1000 each.
  Listing fails only when every call failed; otherwise the partial list is
  returned. Failures surface as `Error::Reader`.

## Items mapping

`items` maps reader output to `StoreItem`s. Every item's `meta.source` is
`SourceRef { kind, id: Some(entry.id) }`, and every document body is markdown
(local files through the host's converter, reader bodies through
`markdown_from_text`).

| Kind | Item | Metadata filled |
| --- | --- | --- |
| folder, file | document | `workspace`, `folder`, `file_path` (canonical), `language`, `observed_at` (mtime), `mime` |
| github | document | `repo` (`owner/name`), `commit` (commits), `url` (issues and PRs), `observed_at` |
| web page | document | `url` |
| rss | document | `url` (entry link), `observed_at` (published) |
| composio | document | `tags = [toolkit]`; payloads add `url`, `observed_at`, `thread_id`, `repo` |
| conversation | conversation | `workspace`, `thread_id`, `turns` (`0..=n-1`), `observed_at` (last turn, else mtime) |

`collect_items(reader, entry, workspace, converter)` lists and reads every
item, returning `Collected { items, skipped }`. One bad item lands in `skipped`
with its error and the pass continues; only a listing failure is an `Err`.
Other entry points: `file_item` (a path with no source), `conversation_item`,
`content_item` and `items::local_file_item`.

## Composio

Composio data does not arrive item by item. A host runs toolkit actions with
its own credentials and hands the raw responses to `sources::composio`, which
holds no credential, opens no socket and decides nothing about when to sync.

1. **Normalisers**, pure `serde_json::Value` transforms, one module per
   toolkit. They walk Composio's envelope variants (top level, under `data`,
   under `data.data`) and return the first array found: `clickup`
   (`extract_tasks`), `github` (`extract_issues`), `linear`
   (`extract_issues`), `notion` (`extract_results`, `extract_page_markdown`),
   each with title, id and updated-time helpers. `fields::pick_str` is the
   shared lookup: it tries dotted paths, descends only through objects, and
   rejects non-string leaves.
2. **Post-processors the host must call** for two toolkits, because their raw
   responses are too verbose:
   - `gmail_post_process::post_process(slug, arguments, &mut data)` rewrites a
     `GMAIL_FETCH_EMAILS` response into slim `messages[]` (other Gmail slugs
     pass through; `raw_html: true` in the arguments skips the reshape). If the
     response carries a response-level `markdownFormatted` string, call
     `apply_response_level_markdown(&mut data, markdown)` **before**
     `post_process`; it is a no-op unless the split count matches the message
     count. `format_email_local_time` renders in the host's local timezone;
     the raw UTC fields are preserved.
   - `slack_post_process::post_process(slug, arguments, &mut data)` reshapes
     `SLACK_FETCH_CONVERSATION_HISTORY`, `SLACK_LIST_CONVERSATIONS` and
     `SLACK_SEARCH_MESSAGES`; unknown slugs are no-ops. `channel_id` for history
     is injected by the host (it is in the request, not the response), and user
     ids are resolved by the host.
3. `normalise_payload(toolkit, &data)` returns `ComposioDocument`s (id, title,
   markdown body, url, `observed_at`, `thread_id`, `repo`), dispatching on the
   case-insensitive toolkit slug: `gmail`, `slack`, `github`, `linear`,
   `notion`, `clickup`; any other toolkit falls back to each record as fenced
   JSON, so a new toolkit is ingested verbosely rather than dropped. Records
   with no text are skipped. `payload_items(toolkit, source_id, &data)` wraps
   them as `StoreItem::Document` with `source.kind = Composio`,
   `source.id = source_id` and `tags = [toolkit]`.

`readers::composio::ComposioReader` is only a placeholder so
`reader_for_request` can serve every kind: `list_items` returns the connection
as one sync target.

## Fetching and the SSRF guard (sources-network)

`fetch::fetch_url(url)` fetches one URL into a `RawDocument`: the `Content-Type`
becomes the declared MIME, the URL the origin, and the last path segment (if
it has an extension) the filename. `fetch::link_item(url, source_id,
converter)` converts it into a document with `source.kind = Link` and `url`
set. The cap is `MAX_DOCUMENT_BYTES` (32 MiB), applied while streaming. No
retries, no robots.txt, no scheduling. Errors: `Invalid` (malformed or refused
URL, empty body), `Unreachable` (never completed, interrupted read, may
succeed later), `Upstream` (non-success status), `TooLarge`.

The web-page and RSS readers fetch through the same path with tighter caps
(10 MiB and 5 MiB). `fetch::ssrf` is public so a host fetching a user-supplied
URL by other means applies the same policy.

Guard rules:

- **Scheme:** `http` and `https` only.
- **Host text:** refused are empty hosts, `localhost`, `.local` and
  `.internal` names, single-label names (internal service names such as
  `redis`), and IP literals that are not globally routable.
- **One address classifier** serves literals and resolved addresses. Not
  fetchable: loopback, private, link-local (including `169.254.169.254`),
  unspecified, CGNAT (`100.64.0.0/10`), `192.0.0.0/16`, multicast, broadcast,
  documentation and benchmarking ranges, reserved `240.0.0.0/4`, IPv6
  unique-local and link-local; IPv4-mapped and IPv4-compatible IPv6 addresses
  are judged by their IPv4 part.
- **IPv6 literals fail closed.** A URL such as `http://[2606:4700::1111]/` is
  refused whatever the address: the host text keeps its brackets, does not parse
  as an IP, and has no dot, so the single-label rule blocks it. A hostname with
  an AAAA record is still vetted when resolved.
- **Resolver:** `PublicOnlyResolver` keeps only public addresses and fails the
  request when none remain, pinning the connection to a vetted address with no
  re-resolution between check and connect.
- **Redirects** are re-checked per hop; a refused hop is not followed, and the
  read fails on the resulting non-success status.
- **Client:** 20-second timeout and the user agent `openhuman`.
- **Body caps** are enforced while streaming (`read_body_capped`), with an early
  rejection on a truthful `Content-Length`.
