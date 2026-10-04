# Tools and context.md

`tinymemory-tools` is the agent-facing side of TinyMemory. It works over any
[`MemoryEngine`](api.md) and has no tool-runtime dependency (no MCP, no
`tinytools`): it produces runtime-neutral tool specs and runs tool calls, and a
host adapts them to whatever its model runtime expects. It also holds the
`context.md` compiler.

| Part | Module | Purpose |
| --- | --- | --- |
| Tools | `tinymemory_tools::tools` | `MemoryTools`, `ToolScope`, `ToolSpec`, the seven tool names |
| Context | `tinymemory_tools::context` | `compile`, `ContextCompiler`, `ContextSpec`, `Brief`, `ContextDoc` |

Reference for the item level lives in rustdoc; the crate README
([`crates/tinymemory-tools/README.md`](../../crates/tinymemory-tools/README.md))
is the quick tour. This page explains how it works and why.

## The design constraint

A model must never choose whose memory it touches. An earlier tool layer let
the model pass a namespace, which let one tenant's agent read another's memory.
Here the host fixes two things in a `ToolScope` when it builds the tools, and
neither is a tool argument:

- where writes land (`place`), and
- how far reads reach (`reach`, see [namespaces.md](namespaces.md)).

Everything below follows from that.

## ToolSpec

```rust,ignore
pub struct ToolSpec {
    pub name: &'static str,        // one of TOOL_NAMES
    pub description: &'static str, // written for the model
    pub parameters: serde_json::Value, // a JSON Schema object
}
```

`ToolSpec` derives `Serialize`. Every object in `parameters` sets
`additionalProperties: false`, and no schema has a `namespace` or `reach`
property. `MemoryTools::specs()` returns the specs to offer, reads first:

- the write tools (`memory_store`, `memory_forget`) appear only when writes are
  enabled;
- `memory_fetch` is left out entirely for an engine whose
  `EngineDescriptor::fetch_modes` is empty, and its `mode` enum lists exactly
  the modes the engine serves.

`TOOL_NAMES` lists all seven in spec order and `WRITE_TOOL_NAMES` the two
write tools. The names are exported as constants (`MEMORY_RECALL`,
`MEMORY_FETCH`, `MEMORY_LIST`, `MEMORY_GET`, `MEMORY_EXPLORE`, `MEMORY_STORE`,
`MEMORY_FORGET`).

## ToolScope

```rust,ignore
pub struct ToolScope {
    pub place: Namespace,       // the node every stored item is written to
    pub reach: Option<Reach>,   // the nodes reads and forgets are confined to
    pub writes: bool,           // whether store and forget are offered
}
```

| Constructor | Result |
| --- | --- |
| `ToolScope::default()` | `place` is the root, `reach` is `None` (every namespace), `writes` is `true`. A single-tenant host's scope. |
| `ToolScope::at(place)` | `place` set, `reach = Some(Reach::of(place))`, writes on. |

`reach: None` reads every namespace, which suits only a host whose engine
serves a single tenant.

`MemoryTools` is built over an `Arc<dyn MemoryEngine>` and a scope:

| Builder | Effect |
| --- | --- |
| `MemoryTools::new(engine)` | `ToolScope::default()` |
| `MemoryTools::with_scope(engine, scope)` | an explicit scope |
| `.placed_at(place)` | writes land at `place`, **and the reach resets to `Reach::of(place)`**: `place` and its ancestors, never a sibling. Writes stay as they were. |
| `.reach(reach)` | replaces the reach; call it after `placed_at` to read differently |
| `.read_only()` | `writes = false` |
| `.scope()` | the current scope |

Because `placed_at` resets the reach, a placed scope can never end up reading
more than its own branch by accident. The order matters: `.reach(r).placed_at(p)`
discards `r`.

```rust,ignore
// One agent: writes to its node, reads it and its ancestors.
let agent = MemoryTools::new(engine.clone()).placed_at(Namespace::agent("writer"));

// A team-wide, read-only auditor.
let team: Namespace = "team:acme".parse()?;
let auditor = MemoryTools::new(engine)
    .placed_at(team.clone())
    .reach(Reach::subtree(team))
    .read_only();
```

Build one `MemoryTools` per agent session from the session's identity, never
from anything the model said. `specs()` is cheap; call it per session so a
read-only or differently scoped agent sees only its own tools.

## Dispatch: MemoryTools::call

`call(name, args: serde_json::Value) -> Result<Value>` runs one tool.

1. An unknown `name` is `Error::InvalidRequest` listing the valid names.
2. A write tool on read-only tools is `Error::Unsupported`. This is checked
   before the arguments are read, so the call never reaches the engine.
3. The call is dispatched to the tool's implementation (reads in
   `tools/read`, writes in `tools/write`), which parses the arguments strictly,
   confines the request to the scope, calls the engine and renders the result.

`null` arguments read as no arguments. Arguments that are not a JSON object are
`InvalidRequest`. An explicit `null` for an optional field reads as absent, since
many models send `null` for an argument they mean to leave out.

### Argument validation

Argument reading (`tools/args`) is deliberately strict. It refuses:

- a `namespace` or `reach` key **anywhere** in the arguments, at any depth,
  nested objects and arrays included. The message says the host fixes it, so
  the model is told rather than quietly ignored. The check runs before the
  unknown-key check, so a `namespace` under a key the tool would otherwise
  reject is still reported as host-fixed;
- any other key the tool's schema does not list;
- a value of the wrong type or out of range.

Messages are lowercase and name the tool and the field, for example
``memory_list: `filter.kinds` `memo` is not an item kind``, so they can be shown
to the model as the tool's error output.

Limits: `limit` is `1..=50`, default `10`. `ids` lists `1..=200` non-blank ids
(`MAX_GET_IDS`), with duplicates dropped. Timestamps are RFC 3339.

## The seven tools

The exact names and schemas are frozen in
[`crates/tinymemory-tools/tests/fixtures/tool_contracts.json`](../../crates/tinymemory-tools/tests/fixtures/tool_contracts.json).
That file is the source of truth for parameter schemas; the summaries below
are for orientation.

| Tool | Kind | Arguments (`?` optional) | Result |
| --- | --- | --- | --- |
| `memory_recall` | read | `question`, `filter?`, `limit?`, `instructions?` | `{answer, citations: [...]}` |
| `memory_fetch` | read | `query`, `mode?`, `filter?`, `limit?`, `cursor?` | `{hits: [...], next_cursor?}` |
| `memory_list` | read | `filter?`, `limit?`, `cursor?` | `{items: [...], next_cursor?}` |
| `memory_get` | read | `ids` | `{items: [...], missing: [ids]}` |
| `memory_explore` | read | `facet`, `filter?`, `limit?` | `{facet, buckets: [{value, count}], total, missing, more_buckets, truncated}` |
| `memory_store` | write | exactly one of `learning` / `document` / `conversation`, `tags?` | `{id, replayed}` |
| `memory_forget` | write | `ids` **or** a non-empty `filter` | `{forgotten, skipped: [ids]}` |

A change to a name or schema changes what every host's model is told, so the
`tool_contracts` test fails until the fixture is regenerated on purpose:

```sh
BLESS_TOOL_CONTRACTS=1 cargo test -p tinymemory-tools --test tool_contracts
```

### The model-facing filter

`filter` is a subset of `MetaFilter`: `kinds`, `sources` (enum lists of item
and source kinds), `tags_any`, `workspace`, `folder`, `file_path`, `repo`,
`url`, `thread_id`, `agent_id`, `observed_after` and `observed_before`. It
never has a namespace or a reach; the tool sets `reach` itself from the scope,
overwriting anything else.

### memory_recall

Asks the engine a question and returns its synthesised answer with citations.
`question` is required and non-blank. `limit` bounds how many memories the
answer may cite; `instructions` steers length or format. A citation renders as
`{id, kind, snippet, score?, meta}`.

### memory_fetch

Raw retrieval. `query` is required. `mode` is one of the modes the engine
serves; when the model names none, `hybrid` is used if served, otherwise the
engine's first mode. A mode the engine does not serve is `InvalidRequest`; an
engine serving no mode gets `Error::Unsupported` (and no spec). Pass back
`next_cursor` as `cursor` for the next page.

### memory_list

Pages through stored memories with no query, optionally narrowed by a filter,
with `cursor` paging as above.

### memory_get

Reads memories whole by id. The request carries the scope's reach, so an id
outside it comes back in `missing`, as does an id that names nothing. A model
therefore cannot tell "does not exist" from "not yours".

### memory_explore

Counts memories per value of one `facet`, largest first. Facets: `kind`,
`source`, `source_id`, `workspace`, `folder`, `file_path`, `language`, `repo`,
`url`, `thread`, `agent`, `tool_call` and `tag`. The `namespace` facet is
deliberately absent. The result is the engine's explore page: `buckets` of
`{value, count}`, `total` (items the filter admitted), `missing` (those with no
value for the facet), `more_buckets` (values beyond `limit`) and `truncated`
(the engine's scan stopped early, so counts are a lower bound).

### memory_store

Stores exactly one memory; none or several of the three shapes is
`InvalidRequest`.

- `learning: {text, learning_kind?, confidence?, evidence?}`: kind is one of
  `preference`, `fact`, `procedure`, `correction`, `other` (default `fact`);
  confidence is `0..=1` (default `0.8`).
- `document: {title?, text}`: stored as a text document.
- `conversation: {turns: [{role, text}]}`: at least one turn; roles are
  `user`, `assistant`, `system`, `tool`.

The item's metadata is built by the tool and never read from arguments:
namespace is the scope's `place`, source is `agent`, `tags` are the model's,
and `observed_at` is the time of the call. Storing the same memory twice is a
replay (`replayed: true`), not a duplicate.

### memory_forget

Exactly one of `ids` or `filter`, else `InvalidRequest`.

- **By ids:** the ids are first read back with `get` under the scope's reach,
  and only those found are forgotten. The rest are returned as `skipped` and
  survive. If none were found, the engine is not called at all.
- **By filter:** the filter must set at least one field (a reach alone would
  mean "everything in reach"); an empty filter is `InvalidRequest`. It is then
  confined to the reach and forgotten as a filter target. `skipped` is empty.

### Result shape

Results carry what a model can act on and nothing else. A hit renders as
`{id, kind, text, score, confidence?, meta}`, where `meta` is a subset:
`source {kind, id?}`, `file_path`, `url`, `thread_id`, `tags`, `observed_at`.
Scores and confidences are rounded to four places so `f32` noise does not reach
the model. Absent optional fields are omitted rather than written as `null`.
The namespace is never rendered.

## Errors

`call` returns `tinymemory_api::Result<serde_json::Value>`:

| Error | When |
| --- | --- |
| `InvalidRequest` | unknown tool; arguments not an object; a host-fixed key; an unknown key; a missing, mistyped or out-of-range value; a bad store shape; a forget with neither or both selectors or an empty filter |
| `Unsupported` | a write tool on read-only tools; `memory_fetch` on an engine serving no fetch mode |
| anything else | whatever the engine returns for the request |

`Unsupported` means "the call is well formed but these tools do not offer the
operation", which is what the variant means across the contract.

## Scoping and security invariants

1. **Namespace and reach are refused at any depth.** A `namespace` or `reach`
   key anywhere in the arguments is `InvalidRequest`, never ignored.
2. **Stores are forced to `place`.** The tool builds the metadata; the model
   supplies only content and tags.
3. **Reads are confined to the reach.** Every recall, fetch, list and explore
   filter has its `reach` overwritten with the scope's; `get` passes it as
   `GetRequest::reach`.
4. **Forget cannot reach out.** By id it goes through `get` under the reach
   first; by filter the reach is applied and an empty filter is refused.
5. **Schemas are closed.** `additionalProperties: false` everywhere.
6. **Read-only means read-only.** The write tools are neither listed nor run.

### The ancestor-forget caveat

A reach built with `Reach::of(place)` (the default from `placed_at`) includes
the node's **ancestors**. Reads there are the point: an agent sees what its team
and the root share. But forget uses the same reach, so an agent may forget
memory it can see that lives at an ancestor, which includes memory shared with
its team or the root. A host that wants an agent's forgets confined to its own
node sets `Reach::exact(place)` (which also narrows reads to that node), or
offers read-only tools. There is no separate "forget reach".

## Adapting ToolSpec to a tool runtime

`ToolSpec` is three fields, and most runtimes want exactly those:

| Runtime | Mapping |
| --- | --- |
| MCP | `name` to `name`, `description` to `description`, `parameters` to `inputSchema`. On `tools/call`, pass `arguments` to `MemoryTools::call` and return the result serialised as text content, or an error result with the error's message on `Err`. |
| OpenAI-style function calling | `{"type": "function", "function": {"name", "description", "parameters"}}`. The schemas are closed objects, so they suit strict mode where the runtime accepts optional properties. |
| Anthropic tool use | `name`, `description`, `input_schema`. |
| `tinytools` or a custom registry | register `(name, description, parameters)` and route calls to `call`. |

```rust,ignore
for spec in tools.specs() {
    runtime.register(spec.name, spec.description, spec.parameters);
}
// When the model calls a tool:
let output = match tools.call(&call.name, call.arguments).await {
    Ok(value) => value.to_string(),
    Err(error) => format!("error: {error}"),
};
```

## context.md

`context.md` is a token-budgeted brief a host injects at the start of a
session so the agent begins with what memory knows about its user. The
compiler works over any engine and is stateless. It is a preset of the
holistic recall in `recall`: one answered section per brief, then the
latest learnings, rendered with frontmatter. The agent lifecycle's packs are
other presets of the same read (see [lifecycle.md](lifecycle.md)).

### ContextSpec

| Field | Default | Meaning |
| --- | --- | --- |
| `budget_tokens` | `1_500` (`DEFAULT_BUDGET_TOKENS`) | the most tokens the whole document may take |
| `briefs` | `Brief::defaults()` | the sections, in order; each is answered by one recall |
| `learnings_limit` | `20` (`DEFAULT_LEARNINGS_LIMIT`) | the most learnings listed after the briefs |
| `reach` | `None` | whose memory the document is about; `None` reads every namespace |

`ContextSpec` is serde-serialisable, so a host can keep it in its config.
`validate()` checks the spec: a zero budget, or a brief with a blank heading or
question, is `context::Error::InvalidSpec`. It is the only error `compile` can
return.

A `Brief` is a `heading`, a `question` and a `filter` (a `MetaFilter`
restricting what the answer may draw on). `Brief::new(heading, question)` has no
filter. The four default briefs, in order:

1. About the user: identity, role, how they like to work
2. Active work: current projects, workspaces and repositories
3. Preferences and standing instructions
4. Recent important events

### Compilation

`compile(&engine, &spec)` (or `ContextCompiler::new().compile(...)`; use
`ContextCompiler::at(timestamp)` for reproducible `generated_at`) does:

1. **Validate** the spec.
2. **Gather briefs:** one recall per brief, with the brief's filter (and the
   spec's `reach` overwriting the filter's reach when set), up to 8 citations,
   and a fixed instruction to answer briefly as markdown bullets stating only
   what the stored items support. A brief whose recall fails is logged and
   skipped. A brief whose answer is blank or cites nothing is skipped too.
3. **Gather learnings:** list `Learning` items under the spec's reach, 100 per
   page up to 50 pages, then rank newest first, then most confident first
   (undated learnings after dated ones; ties keep the engine's order) and take
   `learnings_limit`. A limit of `0` skips the listing. A failed listing leaves
   the learnings out.
4. **Render** within the budget.

Engine failures are never errors of `compile`: an engine that holds nothing,
or that fails everything, yields an empty document.

### The document

```markdown
---
generated_at: 2026-10-04T09:30:00Z
engine: tinyhumans
tokens: 412
refs: [id1, id2, id3]
---

# Context

## About the user

- ...

## Learnings

- The user prefers short answers
```

`ContextDoc` carries the `markdown`, its estimated `tokens`, `generated_at`,
the `engine` id and `refs`: every item the document cites, in order of first
citation. A document with no briefs and no learnings is the empty string, with
zero tokens and no frontmatter. Learning lines are collapsed to a single line.

### Budget and trim rules

Tokens are estimated at four characters each, rounded up (`estimate_tokens`).
The frontmatter counts toward the budget, and the `tokens` it reports is
exact: the compiler settles the number to a fixed point. When the document is
over budget, `render` trims in a fixed order until it fits:

1. **Learnings go first**, one line at a time from the end (so the oldest or
   least confident leave first).
2. **Then the last remaining brief shrinks.** Its body is cut back by the
   overflow, to a word boundary when one is near, and marked with an ellipsis.
   Once fewer than 40 characters of it would remain, the brief is dropped
   instead of kept as a stub.
3. Earlier briefs keep their text longest and every surviving brief keeps its
   place. If everything is trimmed away the result is the empty document,
   which always fits.

## Where things live

```text
crates/tinymemory-tools/src/
├── lib.rs              # crate docs and re-exports
├── tools/
│   ├── mod.rs          # MemoryTools, ToolScope, dispatch
│   ├── spec/           # ToolSpec, tool names, JSON Schemas, limits
│   ├── args/           # strict argument reading and the model filter
│   ├── read/           # recall, fetch, list, get, explore
│   ├── write/          # store, forget
│   └── render/         # compact result JSON
├── context/            # ContextSpec, Brief, compile, Error
├── recall/             # HolisticRecall, ContextPack: gather, settle, render
├── layout/             # MemoryLayout, BrainSource
├── brain/              # Brain, BrainDocument
├── lifecycle/          # AgentMemory, RecallPolicy, turn types
└── background/         # BackgroundJob, BackgroundRunner
```

Tests: `tests/tools_roundtrip.rs` runs every tool against the reference
engine, `tests/tools_scoping.rs` pins the invariants above, and
`tests/tool_contracts.rs` guards the frozen schemas. See [testing.md](testing.md).
