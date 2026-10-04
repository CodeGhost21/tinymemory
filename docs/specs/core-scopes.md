# Core scopes: shared memory above a layout

**Status:** Accepted. **Builds on:** [agent-memory.md](agent-memory.md).
**Plan:** [../plans/core-scopes.md](../plans/core-scopes.md).

## Problem

An `AgentMemory` recalls only the subtree of its layout root. A multi-agent
host such as tinyhivemind needs memory shared more widely than one layout:

- a hive-wide **core** that every agent recalls;
- a **company brain** that every team below one company node recalls.

The host must be able to set that shared memory once, change it for a single
call, and write into it. Before this change none of that was possible.

## Goals and non-goals

- **Goal:** recall from shared nodes *above* the layout root, as their own
  pack sections, set once or per call.
- **Goal:** a host-only way to write learnings and documents into a shared node.
- **Non-goal:** sharing between unrelated nodes. A shared node must be an
  ancestor of the layout root, so a company that shares memory nests its
  tenants below one node (`ws:acme/team:hive`). The `Reach` contract and the
  engines are unchanged.
- **Non-goal:** letting the model choose a scope. Tool arguments still may not
  name a namespace or reach.

## Behavior (`tinymemory-tools`)

- **`CoreScope { at, heading, kinds, limit }`**
  - `CoreScope::new(at, heading)` reads learnings and documents, up to
    `DEFAULT_CORE_LIMIT` (6). It has `kinds(..)` and `limit(..)` builders.
  - `filter()` reads `at` **exactly** (`Reach::exact`).
  - `brief(question)` gives a `context.md` section.
- **`MemoryLayout::admits_core(at)`** accepts only a strict ancestor of the
  root. **`MemoryLayout::ancestors()`** lists those nodes, root first.
- **`AgentMemory::with_core(Vec<CoreScope>) -> Result<Self>`** replaces the
  core set; an empty list drops it. It refuses a node that is not a strict
  ancestor, a blank heading, or a node named twice. `AgentMemory` is cheap to
  clone, so `memory.clone().with_core(..)?` overrides the set for one call.
  `core()` returns the current set.
- **Standard sections** are Learnings, then one section per core scope in
  order, then Brain, this agent's history, and Team conversations. A zero
  limit leaves a core section out.
- **`AgentMemory::promote(&scope, item)`** stores a learning or document at a
  configured core node and overwrites the item's namespace. It refuses a
  conversation or an unconfigured node.
- **`AgentMemory::core_build(&scope)`** returns the `BuildBeliefs` job for
  exactly that node.
- **`AgentMemory::tools()`** is unchanged. `Reach::of(node)` already reads
  every ancestor, core nodes included.

## Invariants

- A core read never admits a sibling tenant. It reads its node exactly, never
  the subtree below it.
- With no core scopes configured, packs are unchanged byte for byte.
- Turns are always written at the agent's node.

## Acceptance criteria

- On the reference engine, a core section shows the company node's
  learnings and never another team's.
- Over both CortexDB doubles, a core section recalls
  `app:tinymemory/ws:acme/app:learnings`, and no scope below a sibling team.
- A `context.md` brief built from a core scope reads only that node.
