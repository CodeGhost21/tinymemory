# Agent memory eval: results

How well the agent memory lifecycle serves an agent across nine scenarios.
Method, metrics and how to rerun it: [README.md](README.md).

**Run:** 2026-10-04 · CortexDB `v0.10.4` (`integration/cortexdb/`) ·
TinyMemory branch `agent-memory` · answer model `openai/gpt-4.1-mini`
(temperature 0) · 38 scored probes per phase.

| Label | Engine | Models behind CortexDB |
| --- | --- | --- |
| `reference` | `ReferenceEngine` (in memory) | none: keyword and toy-vector ranking |
| `cortex-mock` | CortexDB, Direct wire | the harness's mock: hashed embeddings, no extraction |
| `cortex-openrouter` | CortexDB, Direct wire | OpenRouter: `openai/text-embedding-3-large`, `openai/gpt-4.1-mini` for extraction and answers |

## Headline

| | reference | cortex-mock | cortex-openrouter |
| --- | --- | --- | --- |
| Pack holds the answer | 92% | 87% | **95%** (97% after synthesis) |
| Model answers correctly from the pack | 92% | 84% | **92%** (95% after synthesis) |
| Paraphrased questions: pack hit | 85% | 62% | **92%** |
| Contradictions answered with the current value | 6/6 | 5/6 | **6/6** |
| Cross-tenant leaks | 0/3 | 0/3 | **0/3** |
| `pre_turn` p50 / p95 | 1 / 4 ms | 13 / 260 ms | 466 / 1168 ms |
| `post_turn` p50 | 0.2 ms | 2.2 ms | 2.4 ms |
| Beliefs built by synthesis | 1 per item (toy) | 0 | 281 over 9 scenarios |

- **Retrieval is good.** With real embeddings, 36 of 38 packs hold the
  answer, paraphrases included. A model reading the pack answers 35 of 38.
- **Contradictions are resolved by the reader, not the ranking.** Retrieval
  puts the superseded value first in 4 of 5 contradiction probes, since
  "budget is 5000" matches "what is the budget" as well as "budget cut to
  6500". Now that turns carry their dates, the model picks the current
  value every time.
- **Synthesis runs but does not reach the pre-turn pack.** CortexDB builds
  281 beliefs, but fetched sections only show stored items. The 2-probe
  gain after synthesis comes from the answer route (compaction) and from
  enrichment having had more time.
- **The hot path is fast where it can be.** `post_turn` is a few
  milliseconds on every engine. `pre_turn` costs one query embedding per
  scope, so its latency is the embedding provider's round trip.

## Accuracy by scenario (`cortex-openrouter`)

| Scenario | Phase | Pack hit | Extractive answer | Model answer | MRR | Fresh first | Leaks |
| --- | --- | --- | --- | --- | --- | --- | --- |
| brain_lookup | recall | 100% (8/8) | 75% (6/8) | 100% (8/8) | 0.53 | – | – |
| brain_lookup | synthesis | 100% (8/8) | 75% (6/8) | 100% (8/8) | 0.53 | – | – |
| restart_recall | recall | 86% (6/7) | 43% (3/7) | 86% (6/7) | 0.67 | – | – |
| restart_recall | synthesis | 100% (7/7) | 57% (4/7) | 100% (7/7) | 0.67 | – | – |
| contradictions | recall | 100% (6/6) | 33% (2/6) | 100% (6/6) | 0.50 | 20% (1/5) | – |
| contradictions | synthesis | 100% (6/6) | 50% (3/6) | 100% (6/6) | 0.58 | 40% (2/5) | – |
| tool_heavy | recall | 83% (5/6) | 67% (4/6) | 83% (5/6) | 0.75 | – | – |
| tool_heavy | synthesis | 83% (5/6) | 67% (4/6) | 83% (5/6) | 0.75 | – | – |
| team_handoff | recall | 100% (3/3) | 67% (2/3) | 100% (3/3) | 0.61 | – | – |
| team_handoff | synthesis | 100% (3/3) | 67% (2/3) | 100% (3/3) | 0.39 | – | – |
| compaction | recall | 100% (2/2) | 50% (1/2) | 100% (2/2) | 0.62 | – | 0/1 |
| compaction | synthesis | 100% (2/2) | 100% (2/2) | 100% (2/2) | 1.00 | – | 0/1 |
| isolation | recall | 100% (2/2) | 100% (2/2) | 50% (1/2) | 0.75 | – | 0/2 |
| isolation | synthesis | 100% (2/2) | 100% (2/2) | 50% (1/2) | 0.75 | – | 0/2 |
| needle_in_noise | recall | 100% (2/2) | 50% (1/2) | 100% (2/2) | 0.62 | – | – |
| needle_in_noise | synthesis | 100% (2/2) | 50% (1/2) | 100% (2/2) | 0.58 | – | – |
| learnings | recall | 100% (2/2) | 50% (1/2) | 100% (2/2) | 1.00 | – | – |
| learnings | synthesis | 100% (2/2) | 50% (1/2) | 100% (2/2) | 1.00 | – | – |
| **all** | recall | 95% (36/38) | 58% (22/38) | 92% (35/38) | 0.64 | 20% (1/5) | 0/3 |
| **all** | synthesis | 97% (37/38) | 66% (25/38) | 95% (36/38) | 0.65 | 40% (2/5) | 0/3 |

By question style (recall phase):

| Style | reference pack / model | cortex-mock pack / model | cortex-openrouter pack / model |
| --- | --- | --- | --- |
| lexical (25) | 96% / 96% | 100% / 96% | 96% / 92% |
| paraphrase (13) | 85% / 85% | 62% / 62% | 92% / 92% |

The mock models rank by keyword only, so paraphrases fall to 62%. Real
embeddings bring them level with lexical questions. The extractive agent
answers only 23% of paraphrases on every engine, because it needs a shared
word. That is a limit of the stand-in agent, not of the pack.

### The misses, read

- **`tool_heavy/who-to-ask`** ("Who should I talk to about the
  regression?"). The answer, `jmiller`, sits in the fourth tool result of
  a long assistant turn, and that turn never ranks. Memory keeps only a
  tool call's name and id. A result survives only inside the reply's text,
  where it is diluted by the rest of the turn.
- **`restart_recall/resume-focused`**. `start_session` with focus "the
  user's timezone and location" ranked other turns first, and the turn with
  the answer fell outside `history_limit` (6). After synthesis it was found.
- **`isolation/other-tenant-turns`**. The pack held "Our Q3 revenue was
  strong this year", and the model answered "Unknown". It wanted a number.
  The leak check, the point of the probe, passed.

## Synthesis

Each scenario's belief builds ran after a 20 s wait for enrichment. With
real models CortexDB built beliefs in every scenario:

| Scenario | Builds | Scopes | Beliefs built | Wall time |
| --- | --- | --- | --- | --- |
| brain_lookup | 7 | 12 | 31 | 17.5 s |
| restart_recall | 4 | 4 | 32 | 10.1 s |
| contradictions | 5 | 5 | 39 | 10.9 s |
| tool_heavy | 4 | 4 | 24 | 9.2 s |
| team_handoff | 2 | 3 | 8 | 4.7 s |
| compaction | 11 | 11 | 121 | 18.0 s |
| isolation | 4 | 6 | 7 | 9.7 s |
| needle_in_noise | 1 | 1 | 14 | 1.9 s |
| learnings | 1 | 2 | 5 | 7.8 s |

Against the mock models every build answered `built: 0`, because the mock
extracts no facts to build from. A build is seconds per scope, so it must
never run on a turn. The library returns it as a `BackgroundJob`.

What changed after synthesis: pack hits went from 36 to 37 and model
answers from 35 to 37. The gains are in `restart_recall` and `compaction`,
both helped by more enrichment time and by the answer route, which reads
the derived layers. The fetched sections of a pre-turn pack (Learnings,
Brain, history, team) never show a belief. `CortexEngine::fetch` decodes
only `layers.events`, so the beliefs CortexDB builds are invisible to the
hot path. This is the largest gap the eval found; see
[follow-ups](#follow-ups).

## Latency

`cortex-openrouter` (ms):

| Step | n | p50 | p95 | max |
| --- | --- | --- | --- | --- |
| `post_turn` (log) | 71 | 2.4 | 3.3 | 3.8 |
| `pre_turn` (log + recall) | 71 | 466 | 1168 | 1191 |
| probe `pre_turn` (new thread) | 68 | 13 | 2081 | 5102 |
| `start_session` | 6 | 7.4 | 10.6 | 10.6 |
| `recall_for_compaction` (runs a model) | 2 | 1998 | 1998 | 1998 |
| brain ingest, waiting until visible | 8 | 1868 | 2948 | 2948 |
| settle (every write listed) | 9 | 858 | 2145 | 2145 |

- `post_turn` and the log half of `pre_turn` never wait for indexing:
  about 2 ms.
- The read half of `pre_turn` asks CortexDB for one recall pack per scope.
  Each pack embeds the query, which with real models is an OpenRouter round
  trip (about 300–1000 ms). Repeated questions hit CortexDB's cache, so
  the probe p50 is 13 ms.
- On `cortex-mock` (no network models) `pre_turn` is 13 ms p50 and 260 ms
  p95. The p95 is the first read under a new root, which discovers its
  scopes.

## What the eval changed

The eval was built before these fixes and run again after them. Baseline
runs used the same scenarios without the model answerer.

| Finding | Fix | Before → after (`cortex-openrouter` unless noted) |
| --- | --- | --- |
| Fetch read a pack's scopes one at a time, so a turn's latency grew with its scope count | `cortex/engine/fetch.rs` reads up to 4 scopes concurrently, in order | `pre_turn` p50 916 → 466 ms, p95 1432 → 1168 ms. Mock: 38 → 13 ms p50. Slowest probe 11.8 → 5.1 s |
| CortexDB v0.10 builds beliefs within the request and returns `built`, but the client reported `Started` and dropped the count. The test double modelled a queued job the server never returns | `cortex/engine/consolidate.rs` reports `Completed` with `ConsolidateReceipt::built`, or `Started` when an answer names a job. The double answers like v0.10 | builds now report `done` with 281 beliefs counted; before, `started` and no count |
| A superseded value and its correction looked alike in a pack | Timed conversation bullets are led by `[YYYY-MM-DD HH:MM]` (`recall/gather.rs`) | stale value ranked first in 4/5 probes either way; the model now answers all 6 contradiction probes with the current value |
| Compaction's query was the last 600 characters of the dropped turns, so early facts never steered it, and the summary read only 6 turns | The gist samples the start of every dropped turn; the summary reads as many turns as were dropped (up to 24) | compaction pack hits 1/2 → 2/2 in the recall phase |
| The docs said fetched sections show built beliefs | `lifecycle.md`, `cortex-wire.md` and `consolidate` docs now say where beliefs do and do not surface | — |

## Follow-ups

1. **Surface beliefs in the pre-turn pack.** This has the most upside.
   CortexDB already returns `beliefs` and `facts` layers in the same recall
   pack fetch requests. Mapping them to `Learning` hits for a Learnings
   section would bring synthesis onto the hot path at no extra request.
   That needs a decision on contract semantics: these hits are not stored
   items, so they cannot be listed or forgotten by id. It belongs in a spec
   change rather than a quiet engine tweak.
2. **Keep tool results.** `ToolCallRef` keeps a name and an id, so a tool
   result survives only as prose in the reply (`tool_heavy/who-to-ask`). A
   `post_turn` that also stores results as `Role::Tool` turns would make
   them retrievable on their own.
3. **"Team conversations" includes the agent's own turns.** The section
   reads every agent's conversations and dedupes against the agent's
   history, so the agent's own overflow shows up under "Team". An
   agent-exclusion filter would fix the label and free the slots for other
   agents.
4. **Rank by recency for history.** Fetch ranks by relevance only. The
   dates fix the reader's side, but `fresh first` is still 1–2 of 5. A
   recency boost in the history section, or beliefs (1), would put current
   values first.

## Reproduce

```sh
cargo run -p tinymemory-integrations --features full --example memory_eval -- --llm
./scripts/memory-eval.sh --llm
MODELS=openrouter ./scripts/memory-eval.sh --llm
```

The packs behind every number are in `target/memory-eval/<label>.json`
after a run. Model answers vary slightly between runs, even at temperature
0. Read a one-probe change as noise.
