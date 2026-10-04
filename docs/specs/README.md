# Specifications

- [Memory v2: Recall, Fetch, Store](memory-v2.md) — the contract, the CortexDB
  engine, `context.md`, legacy import and the conformance suite. Accepted; it
  supersedes every earlier specification.
- [Agent memory lifecycle](agent-memory.md) — the standard layout (brain by
  source, per-agent conversations, learnings), holistic recall, pre- and
  post-turn calls, compaction and background belief builds. Accepted; builds
  on memory v2.
- [Core scopes](core-scopes.md) — shared memory above a layout (a hive-wide
  core, a company brain) recalled as its own sections, set per agent or per
  call, written by the host. Accepted; builds on the agent memory lifecycle.

Specifications define what the system must do before implementation details
take over. Create one for behavior that changes a public API, crosses module
boundaries, introduces a durable data format, or has meaningful operational
constraints.

Use a short kebab-case filename such as `retry-policy.md`. Each specification
should contain:

1. **Status and owner** — Draft, Accepted, Implemented, or Superseded.
2. **Problem** — the user or system need, without prescribing a solution.
3. **Goals and non-goals** — the exact boundary of the work.
4. **Proposed behavior** — public API, inputs, outputs, errors, and examples.
5. **Invariants and constraints** — properties every implementation must keep.
6. **Acceptance criteria** — externally observable pass/fail conditions.
7. **Open questions** — unresolved decisions that block acceptance.

After the specification is accepted, create a linked implementation plan in
[`../plans/`](../plans/README.md). Keep code snippets small enough to clarify
the contract; production code still belongs under `src/`.
