# CortexDB via the TinyHumans backend

## Status and owner

Implemented. Owner: TinyMemory maintainers.

## Problem

The TinyHumans backend hosts CortexDB behind `/memory/*`. A host that is signed
in to TinyHumans wants that memory as an ordinary `MemoryProvider`, using its
session as the credential, without the user pasting an engine key. The existing
`cortex` adapter speaks CortexDB's own `/v1/*` API and cannot reach it.

## Goals and non-goals

Goals:

- Reuse the `cortex` adapter's append-and-fold storage, ingestion and answer
  code over the hosted routes, with the same capabilities as `CortexProvider`.
- Resolve the credential per request, so a refreshing session works.
- Map the backend's typed failures onto the existing `MemoryError` taxonomy.
- Let a host list and build engines from configuration (`tinymemory::factory`)
  and copy memories between two providers (`tinymemory::migrate`).

Non-goals: changing the backend, exposing hosted-only routes that have no
`MemoryProvider` counterpart (`facts`, `beliefs`, `understanding`,
`derivation-status`, `blobs`), or a wire-level `MemoryError` change.

## Proposed behavior

### Wire

Base is the backend origin (default `https://api.tinyhumans.ai`), no `/v1`.

| Operation | Direct (`cortex`) | Hosted (`tinyhumans`) |
| --- | --- | --- |
| append | `POST v1/experience` | `POST memory/experience` |
| append and wait | `POST v1/experience?wait=indexed` | `POST memory/experience`, then poll |
| batch | `POST v1/experience/bulk?wait=indexed` | one `POST memory/experience` per item, in order, then poll |
| list | `GET v1/events` | `GET memory/events` |
| recall | `POST v1/recall` | `POST memory/recall` |
| delete | `POST v1/forget` | `POST memory/forget` |
| answer | `POST v1/answer` | `POST memory/answer` |
| scopes | `GET v1/scopes/list?limit=N` | `GET memory/scopes` |
| health | `GET v1/admin/health` | `GET memory/scopes?prefix=__health__` |

Every body carries `scope`. Responses are `{"success":true,"data":<body>}`; the
transport unwraps `data`. A 2xx body without the envelope is a `Backend` error.

### Auth

`Authorization: Bearer <token>`, where the token comes from a `BearerSource`
(`tinymemory_remote::BearerSource`, re-exported as
`tinymemory::factory::BearerSource`). It is called on **every request attempt**,
including retries. An error or blank token is `MemoryError::Unauthorized` and no
request is sent. The token is never logged; `Debug` output shows only that a
client is authenticated. A credentialed endpoint must be HTTPS unless it is
loopback.

### Errors

Failures are `{"success":false,"error":"...","errorCode":"CODE"}`. They map to
`MemoryError` and carry the backend code as a `[CODE] ` message prefix, read back
with `tinymemory_remote::error_code`.

| HTTP | Typical code | `MemoryError` | Retried (reads) |
| --- | --- | --- | --- |
| 401, 403 | `UNAUTHORIZED` | `Unauthorized` | no |
| 402 | `USER_INSUFFICIENT_CREDITS` | `BudgetExceeded` | no |
| 429, 502, 503, 504 | `RATE_LIMITED` | `Unavailable` | yes, 3 attempts |
| 400, 409, 413, 422 | `VALIDATION_ERROR`, `CONFLICT` | `Invalid` | no |
| 404 | | `NotFound` | no |
| other | | `Backend` | no |

`is_insufficient_credits(&MemoryError)` is the check for a "top up" prompt.
`MemoryError` itself is unchanged, so the bus wire names and contract version
are unchanged; the code survives in the message, not as a wire field.

### Idempotency

Every write body already carries an `idempotency_key` (a fresh unique key for
keyed writes, a content hash for ingestion). In hosted mode the same value is
also sent as `Idempotency-Key` when it matches `^[A-Za-z0-9_.:-]{1,128}$`.

### Read-your-writes

The backend drops `?wait=indexed`. Hosted mode reproduces it client side with
the wait keyed writes already use: poll `memory/events` until the accepted event
id is listed (fatal after 30 s), then poll `memory/recall` until it is ranked
(best effort, 10 s). A replayed write (`replayed_from_idempotency`) is not
waited on.

### Answer

The hosted `answer` schema is strict. The body holds only `scope`, `question`,
`use_pack_id`, `cite_sources`, `include_context` and, when set,
`answer_instructions`. A missing instruction is omitted, never `null`.

### Constructors

```rust
CortexMemory::tinyhumans(backend_base_url: &str, bearer: Arc<dyn BearerSource>) -> Result<CortexMemory>
tinymemory_remote::tinyhumans_provider(backend_base_url: &str, bearer: Arc<dyn BearerSource>) -> Result<CortexProvider>
```

The provider reports driver id `tinyhumans` (`TINYHUMANS_DRIVER_ID`), registered
`External` in `DriverRegistry::builtin()`. The Cargo feature `tinyhumans`
implies `cortex`.

## Factory and migration

`tinymemory::factory` (feature `factory`) offers `list_engines()`, which lists
only compiled-in engines, and `build_provider(id, &EngineConfig, EngineCredential)`.
Each engine arm is gated on its own feature, so every feature combination
compiles. The `tinyhumans` engine is labelled `CortexDB (via TinyHumans)`, is
`hosted`, and takes its credential from the host (`EngineCredential::Dynamic`,
or `Static` for a token a user pasted).

`tinymemory::migrate::copy(from, to, progress)` walks `export_page` to the end
and feeds each page to `import_records`. It never deletes from the source and
reports `pages`, `records`, `imported`, `skipped`, `failed`.

## Invariants and constraints

- Direct-mode behavior is unchanged.
- No token appears in any `Debug` output or error message.
- Hosted mode never sends a `/v1` path, `wait=`, a bulk route or an unknown
  `answer` key.

## Acceptance criteria

- The conformance suite passes against a `/memory/*` double
  (`hosted_test.rs`), and against a real backend when
  `TINYMEMORY_TEST_TINYHUMANS_URL` and `TINYMEMORY_TEST_TINYHUMANS_TOKEN` are set
  (`tests/live_remote_engines.rs`).
- Tests cover path mapping for every operation, envelope unwrap, 401/402/429/400
  mapping, per-request bearer resolution, the bulk fallback and the poll.

## Open questions

- `memory/scopes` documents only `prefix`. Hosted mode sends no `limit`, so a
  backend default page size would truncate `namespaces()` without an error.
  Confirm the default with the backend, or add a cursor.
- Hosted rate limit is 300 requests per minute per user. Per-item bulk writes
  and the visibility poll spend it faster than the direct path; batching support
  in the backend would remove the per-item cost.
