# CortexDB flags eval

Do CortexDB's server flags change what memory is worth to an agent? This
eval runs the [agent memory eval](README.md) once per **flag profile**, each
against its own fresh server, and compares the runs' KPIs against the
baseline.

TinyMemory's `CortexEngine` exposes no tuning: nearly every knob CortexDB
has is a server environment variable or a `cortex.toml` key. So the sweep
varies the server, not the client. Per-request options such as recall
`order`, `query_variants` and write `directives` would need new engine API
and are not covered.

## Running it

```sh
# Every profile on the mock models: plumbing and latency only (see Caveats).
./scripts/memory-flag-sweep.sh

# Real models through OpenRouter, a model answering every probe. Costs money
# per profile: run the baseline alone first and read its cost.
MODELS=openrouter ./scripts/memory-flag-sweep.sh baseline -- --llm
MODELS=openrouter PARALLEL=4 ./scripts/memory-flag-sweep.sh -- --llm

# Some profiles, some scenarios, two repeats each to measure the noise.
REPEAT=2 ./scripts/memory-flag-sweep.sh baseline no-graph -- --only conflicts,surprise

# Compare reports by hand.
cargo run -p tinymemory-integrations --features full --example memory_eval -- \
  compare target/memory-eval/flags/*.json
```

Each run writes `flags-<profile>-r<n>.{md,json,log}` to `OUT_DIR`
(`target/memory-eval/flags`), and the comparison writes `summary.md`.
`PARALLEL` servers run at once, on free ports from `BASE_PORT` (3150) up.
A profile whose `# requires:` credential is unset is skipped.

## Profiles

Profiles live in `integration/cortexdb/flags/`. Each is applied on top of
`baseline.env` and names only what it changes. All but the two composites
change one thing, so a moved KPI can be pinned on it.

| Profile | Change from the baseline | Targets |
| --- | --- | --- |
| `baseline` | graph retrieval, auto-built belief layers, per-question salience routing, no minimum age for consolidation | all |
| `no-graph` | `CORTEX_ENTITY_GRAPH=0` | accuracy, cost |
| `no-auto-route` | `CORTEX_AUTO_ROUTE=0` | accuracy |
| `no-hyde-multihop` | `CORTEX_HYDE_PASSAGES_MS=0`, `CORTEX_MULTIHOP_QUERY_PLANNER_DISABLE=1` | accuracy, cost |
| `salience-high` | `CORTEX_SALIENCE_WEIGHT=0.3` (default 0.10) | surprise, accuracy |
| `surprise-strict` | `CORTEX_CONSOLIDATION_MAX_SURPRISE=0.2` (default 0.5) | surprise, learning |
| `surprise-loose` | `CORTEX_CONSOLIDATION_MAX_SURPRISE=0.9` | surprise, learning |
| `bitemporal-enforce` | `CORTEX_BITEMPORAL_MODE=enforce` (default `shadow`) | conflicts |
| `bitemporal-off` | `CORTEX_BITEMPORAL_MODE=off` | conflicts |
| `no-polarity-recheck` | `CORTEX_POLARITY_RECHECK=0` | conflicts, cost |
| `layers-incremental` | `CORTEX_V1_LAYERS_INCREMENTAL=1` | learning, cost |
| `no-verifier` | verifier lane off (see below) | cost, accuracy |
| `enrich-batched` | `CORTEX_ENRICHMENT_SCOPE_QUIET_MS=5000` | cost, learning |
| `no-learning-loop` | feedback-weight and cognitive-persist jobs pushed to once a year (`cortex.no-learning.toml`) | learning |
| `rerank-cohere` | Cohere `rerank-v3.5` cross-encoder; needs `COHERE_API_KEY` | accuracy, cost |
| `max-recall` | 3 HyDE passages, entity-vector seeding, assistant excerpts, enforced bi-temporal validity | all |
| `cost-optimized` | graph, HyDE, multihop, polarity recheck and verifier off; enrichment batched | all |

Every flag a profile sets was checked against the `v0.10.4` server binary.
Each profile's header links the CortexDB docs page for its flags.

**The verifier.** The docs say an empty `CORTEX_VERIFIER_URL` turns the
verifier off. In `v0.10.4` it does not: the server falls back to
`api.openai.com`, and it turns the verifier on whenever `OPENAI_API_KEY` or
`CORTEX_VERIFIER_API_KEY` is set. `no-verifier` empties both. A boot-log
diff shows the verifier is the only lane that changes.

## KPIs

Every run ends with these (`kpi.rs`). Unless noted, each is read in the
synthesis phase, after the belief build. A dash means the run could not
measure it (no CortexDB, or no `--llm`).

| Group | KPI | Meaning |
| --- | --- | --- |
| accuracy | pack hit (recall), pack hit | Probes whose pack holds the answer, before and after synthesis |
| accuracy | MRR | Mean reciprocal rank of the answer in the pack |
| accuracy | extractive / model answer | Probes answered correctly by the scripted agent / the `--llm` model |
| accuracy | captured | Probes whose answer CortexDB derived as a fact or belief |
| accuracy | leaks | Packs holding another tenant's data or in-prompt turns |
| learning | lesson in pack / answered / captured | The same three, over the `learnings` and `learning_from_feedback` scenarios: do corrections and standing instructions stick? |
| learning | synthesis gain | Pack hit after synthesis minus before, in points: what belief building adds |
| learning | beliefs built / held / not supported, belief confidence, facts held | What CortexDB learned: beliefs its builds reported, beliefs and facts it lists, beliefs whose stance is not `supported`, and mean confidence |
| surprise | surprise in pack / MRR / answered | Over the `surprise` scenario: does a break from routine (a backup failing after 412 days, a 47x error spike) surface, and how high? |
| conflicts | planted conflicts flagged | Conflicts the `conflicts` scenario plants (refund timelines that disagree) that CortexDB raises |
| conflicts | spurious conflicts | Conflicts raised in scenarios that plant none (`contradictions`, where a superseded value may fairly be flagged, is excluded) |
| conflicts | conflicts raised | Every conflict raised |
| conflicts | disagreement in pack | `conflicts` probes whose pack shows both sides |
| conflicts | fresh first | Over superseded values: the current one comes first |
| cost | CortexDB models, calls, tokens | What CortexDB's models spent over the run (`v1/admin/usage`), and by role |
| cost | answerer, answerer tokens | What the `--llm` model spent (OpenRouter prices each call) |
| cost | per correct answer | All of the above over correct model answers (extractive without `--llm`) |
| cost | pack tokens (mean) | Prompt tokens a pack costs the host |
| latency | pre_turn / probe p50, p95; enrichment drain; belief builds | What an agent waits for, and how long synthesis takes |

**Reading a comparison.** `summary.md` shows each profile's value with its
delta from the baseline. ▲ marks an improvement and ▼ a regression, but
only when the delta is larger than the noise: the spread across a
profile's repeats, and never less than 3 points for a rate, 0.03 for a
score, 25% (at least 5 ms) for a latency, or 10% for a cost or count. `~`
means within noise, and `Δ` means a KPI with no better direction changed.
"What moved" lists every ▲ and ▼ by profile.

## Results

RESULTS_PENDING

## Caveats

- **Mock models measure plumbing, not intelligence.** They embed by hashing
  and extract nothing, so no facts, beliefs or conflicts form, and most
  flags have nothing to act on. Only a real-model run says whether a flag
  matters.
- **The sample is small.** Each phase has about 50 scored probes, so one
  probe is about 2 points, and the learning, surprise and conflict KPIs
  rest on a handful of probes each. A single run cannot separate a 1-probe
  change from model nondeterminism. Use `REPEAT` before acting on a small
  delta.
- **Some flags target scale this eval does not reach.** Graph retrieval,
  multihop and HyDE are tuned for multi-session questions over thousands
  of events. Each scenario here holds a few dozen, so "no effect" means no
  effect at this size.
- **Time-based jobs barely run.** The learning loop ticks every 60–120 s,
  and a scenario lasts a few minutes. `no-learning-loop` can only show
  what a few ticks do.
