//! An accuracy and latency eval of the agent memory lifecycle.
//!
//! A scripted agent (`agent`) plays nine scenarios (`scenarios`) through the
//! real lifecycle calls: brain lookups, a restart, contradicting facts, a
//! tool-heavy incident, a team handoff, compaction, tenant isolation, a
//! needle in noise, and explicit learnings. After each scenario's writes
//! settle, its probes are scored (`score`). Every scenario then runs a
//! belief build over its whole tree and is probed again, so the effect of
//! synthesis shows up as a second phase.
//!
//! ```sh
//! # Offline, against the reference engine:
//! cargo run -p tinymemory-integrations --features full --example memory_eval
//!
//! # Against a CortexDB (see integration/cortexdb/ and docs/evals/):
//! CORTEX_DB_URL=http://127.0.0.1:3142 CORTEX_DB_KEY=tinymemory-cortex-test \
//!   cargo run -p tinymemory-integrations --features full --example memory_eval -- \
//!   --label mock --json target/memory-eval/mock.json
//! ```
//!
//! Flags:
//!
//! - `--engine reference|cortex`: the default is `cortex` when
//!   `CORTEX_DB_URL` is set, and `reference` otherwise.
//! - `--only <scenario>`: run one scenario.
//! - `--enrich-wait <secs>`: how long to let CortexDB extract facts before
//!   the belief build (default 20 against CortexDB, 0 otherwise).
//! - `--json <path>`: write every probe, pack included, as JSON.
//! - `--label <name>`: name the run in the report.
//!
//! Everything is written below roots unique to the run and forgotten at the
//! end, unless `CORTEX_DB_KEEP` is set.

mod agent;
mod inspect;
mod scenarios;
mod score;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use serde::Serialize;
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    ConsolidateRequest, ForgetTarget, ListRequest, MemoryEngine, MemoryMeta, Reach, Role,
    StoreItem, Turn,
};
use tinymemory_integrations::cortex::{CortexCredential, CortexEngine};
use tinymemory_tools::{
    AgentMemory, BackgroundJob, Brain, BrainDocument, Compaction, ContextPack, JobOutcome,
    MemoryLayout, PreTurn, RecallPolicy, SessionStart,
};

use agent::{ScriptedAgent, ms};
use inspect::{Derived, Inspector};
use scenarios::{MAIN, Probe, Scenario, Step, Via};
use score::{Latency, ProbeResult, Totals, score};

type Error = Box<dyn std::error::Error>;

/// Turns of a thread the scripted agent keeps in its prompt.
const WINDOW: u32 = 8;

/// The longest a scenario's writes may take to become visible.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(90);

/// The command line.
struct Args {
    engine: String,
    only: Option<String>,
    enrich_wait: Option<u64>,
    json: Option<String>,
    label: String,
}

fn args() -> Result<Args, Error> {
    let mut parsed = Args {
        engine: if std::env::var("CORTEX_DB_URL").is_ok() {
            "cortex".into()
        } else {
            "reference".into()
        },
        only: None,
        enrich_wait: None,
        json: None,
        label: String::new(),
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--engine" => parsed.engine = value()?,
            "--only" => parsed.only = Some(value()?),
            "--enrich-wait" => parsed.enrich_wait = Some(value()?.parse()?),
            "--json" => parsed.json = Some(value()?),
            "--label" => parsed.label = value()?,
            other => return Err(format!("unknown flag {other}").into()),
        }
    }
    if parsed.label.is_empty() {
        parsed.label = parsed.engine.clone();
    }
    Ok(parsed)
}

/// Latency samples by step.
#[derive(Default, Serialize)]
struct Timings(BTreeMap<String, Vec<f64>>);

impl Timings {
    fn add(&mut self, step: &str, ms: f64) {
        self.0.entry(step.to_string()).or_default().push(ms);
    }
}

/// What the synthesis step did for a scenario.
#[derive(Debug, Default, Serialize)]
struct Synthesis {
    jobs: usize,
    outcomes: BTreeMap<String, usize>,
    scopes: usize,
    ms: f64,
    derived: Vec<Derived>,
}

/// One scenario's results.
#[derive(Serialize)]
struct ScenarioReport {
    name: &'static str,
    about: &'static str,
    writes: usize,
    tool_calls: usize,
    settle_ms: f64,
    synthesis: Synthesis,
    probes: Vec<ProbeResult>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = args()?;
    let url = std::env::var("CORTEX_DB_URL").unwrap_or_default();
    let key = std::env::var("CORTEX_DB_KEY").unwrap_or_else(|_| "tinymemory-cortex-test".into());
    let (engine, inspector): (Arc<dyn MemoryEngine>, Option<Inspector>) = match args.engine.as_str()
    {
        "reference" => (Arc::new(ReferenceEngine::new()), None),
        "cortex" if !url.is_empty() => (
            Arc::new(CortexEngine::direct(&url, CortexCredential::api_key(&key))?),
            Some(Inspector::new(&url, &key)),
        ),
        "cortex" => return Err("--engine cortex needs CORTEX_DB_URL".into()),
        other => return Err(format!("unknown engine {other}").into()),
    };
    let enrich_wait = args
        .enrich_wait
        .unwrap_or(if inspector.is_some() { 20 } else { 0 });
    let run = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    println!(
        "memory eval `{}`: engine {} ({:?}), run {run}\n",
        args.label,
        engine.descriptor().id,
        engine.health().await
    );

    let mut timings = Timings::default();
    let mut reports = Vec::new();
    for scenario in scenarios::all() {
        if args
            .only
            .as_deref()
            .is_some_and(|only| only != scenario.name)
        {
            continue;
        }
        println!("== {}: {}", scenario.name, scenario.about);
        let report = run_scenario(
            &engine,
            inspector.as_ref(),
            run,
            &scenario,
            enrich_wait,
            &mut timings,
        )
        .await?;
        for phase in ["recall", "synthesis"] {
            let totals = Totals::of(report.probes.iter().filter(|p| p.phase == phase));
            println!(
                "   {phase:<9} hits {:<12} answers {:<12} MRR {:.2}",
                Totals::pct(totals.hits, totals.scored),
                Totals::pct(totals.answers_ok, totals.scored),
                totals.mrr,
            );
        }
        reports.push(report);
    }

    print_summary(&args.label, &reports, &timings);
    if let Some(path) = &args.json {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir)?;
        }
        let out = serde_json::json!({
            "label": args.label,
            "engine": engine.descriptor().id,
            "run": run,
            "scenarios": reports,
            "timings": timings,
        });
        std::fs::write(path, serde_json::to_string_pretty(&out)?)?;
        println!("\nwrote {path}");
    }
    Ok(())
}

/// The layout of `tenant` in `scenario` for this run.
fn layout(run: u64, scenario: &str, tenant: &str) -> Result<MemoryLayout, Error> {
    let root = format!("project:eval-{run}-{}-{tenant}", scenario.replace('_', "-"));
    Ok(MemoryLayout::new(root.parse()?)?)
}

/// The tenants a scenario touches.
fn tenants(scenario: &Scenario) -> Vec<&'static str> {
    let mut tenants: Vec<&'static str> = scenario
        .steps
        .iter()
        .map(|step| match step {
            Step::Doc { tenant, .. } | Step::Chat { tenant, .. } => *tenant,
            Step::Learning { .. } => MAIN,
        })
        .chain(scenario.probes.iter().map(|probe| probe.tenant))
        .collect();
    tenants.sort_unstable();
    tenants.dedup();
    tenants
}

async fn run_scenario(
    engine: &Arc<dyn MemoryEngine>,
    inspector: Option<&Inspector>,
    run: u64,
    scenario: &Scenario,
    enrich_wait: u64,
    timings: &mut Timings,
) -> Result<ScenarioReport, Error> {
    let policy = RecallPolicy {
        build_beliefs_every: Some(4),
        ..RecallPolicy::default()
    };
    let memory = |tenant: &str, agent: &str| -> Result<AgentMemory, Error> {
        Ok(
            AgentMemory::new(engine.clone(), layout(run, scenario.name, tenant)?, agent)?
                .with_policy(policy.clone()),
        )
    };
    let epoch = Utc
        .with_ymd_and_hms(2026, 9, 1, 9, 0, 0)
        .single()
        .ok_or("a valid epoch")?;

    // Writes.
    let mut jobs: Vec<BackgroundJob> = Vec::new();
    let mut writes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut tool_calls = 0;
    for step in &scenario.steps {
        match step {
            Step::Doc {
                tenant,
                source,
                title,
                text,
            } => {
                let brain = Brain::new(engine.clone(), layout(run, scenario.name, tenant)?);
                let started = Instant::now();
                let ingested = brain
                    .ingest(BrainDocument::new(source.clone(), *text).titled(*title))
                    .await?;
                timings.add("brain ingest (visible)", ms(started));
                jobs.push(ingested.job);
                *writes.entry(tenant).or_default() += 1;
            }
            Step::Learning {
                kind,
                text,
                confidence,
            } => {
                let layout = layout(run, scenario.name, MAIN)?;
                let meta = MemoryMeta {
                    namespace: layout.learnings().clone(),
                    ..MemoryMeta::default()
                };
                engine
                    .store(StoreItem::learning(*text, *kind, *confidence, meta))
                    .await?;
                *writes.entry(MAIN).or_default() += 1;
            }
            Step::Chat {
                tenant,
                agent,
                thread,
                day,
                turns,
            } => {
                let mut scripted = ScriptedAgent::new(memory(tenant, agent)?, thread, WINDOW)
                    .at(epoch + chrono::Duration::days(*day));
                for (text, tools) in turns {
                    let record = scripted.user(text, tools).await?;
                    timings.add("pre_turn (log + recall)", record.pre_ms);
                    timings.add("post_turn (log)", record.post_ms);
                    if !record.logged {
                        println!("   ! a turn of {thread} was not logged");
                    }
                    tool_calls += record.tool_calls;
                    jobs.extend(record.jobs);
                    *writes.entry(tenant).or_default() += 2;
                }
            }
        }
    }

    // Settle: wait until every write is listed.
    let started = Instant::now();
    for (tenant, expected) in &writes {
        settle(engine, &layout(run, scenario.name, tenant)?, *expected).await?;
    }
    let settle_ms = ms(started);
    timings.add("settle (all writes listed)", settle_ms);

    let mut probes = Vec::new();
    for probe in &scenario.probes {
        probes.push(run_probe(engine, run, scenario, probe, "recall", &policy, timings).await?);
    }

    // Synthesis: the jobs the writes handed back, then one build per tenant
    // over its whole tree.
    if enrich_wait > 0 {
        tokio::time::sleep(Duration::from_secs(enrich_wait)).await;
    }
    for tenant in tenants(scenario) {
        let root = layout(run, scenario.name, tenant)?.root().clone();
        jobs.push(BackgroundJob::BuildBeliefs {
            request: ConsolidateRequest::new(Reach::subtree(root)),
        });
    }
    let runner = memory(MAIN, "eval")?.background();
    let mut synthesis = Synthesis {
        jobs: jobs.len(),
        ..Synthesis::default()
    };
    let started = Instant::now();
    for job in jobs {
        let report = runner.run(job).await?;
        let outcome = match &report.outcome {
            JobOutcome::Done => "done",
            JobOutcome::Started => "started",
            JobOutcome::Scheduled => "scheduled",
            JobOutcome::Skipped { .. } => "skipped",
        };
        *synthesis.outcomes.entry(outcome.to_string()).or_default() += 1;
        synthesis.scopes += report.consolidation.map_or(0, |receipt| receipt.scopes);
    }
    synthesis.ms = ms(started);
    timings.add("synthesis (all builds)", synthesis.ms);
    if let Some(inspector) = inspector {
        for tenant in tenants(scenario) {
            let layout = layout(run, scenario.name, tenant)?;
            let node = layout.root().to_string();
            for scope in inspector.scopes(&node).await? {
                synthesis
                    .derived
                    .push(inspector.derived(&scope, scenario.about).await?);
            }
        }
    }
    let beliefs: usize = synthesis.derived.iter().map(|d| d.beliefs).sum();
    let facts: usize = synthesis.derived.iter().map(|d| d.facts).sum();
    println!(
        "   synthesis {:?} over {} scopes in {:.0} ms; derived {facts} facts, {beliefs} beliefs",
        synthesis.outcomes, synthesis.scopes, synthesis.ms
    );

    for probe in &scenario.probes {
        probes.push(run_probe(engine, run, scenario, probe, "synthesis", &policy, timings).await?);
    }

    if std::env::var("CORTEX_DB_KEEP").is_err() {
        for tenant in tenants(scenario) {
            engine
                .forget(ForgetTarget::Filter(
                    layout(run, scenario.name, tenant)?.holistic_filter(),
                ))
                .await?;
        }
    }
    Ok(ScenarioReport {
        name: scenario.name,
        about: scenario.about,
        writes: writes.values().sum(),
        tool_calls,
        settle_ms,
        synthesis,
        probes,
    })
}

/// Waits until `layout` lists at least `expected` items.
async fn settle(
    engine: &Arc<dyn MemoryEngine>,
    layout: &MemoryLayout,
    expected: usize,
) -> Result<(), Error> {
    let started = Instant::now();
    loop {
        let mut listed = 0;
        let mut cursor = None;
        loop {
            let mut req = ListRequest::new(layout.holistic_filter(), 100);
            req.cursor = cursor;
            let page = engine.list(req).await?;
            listed += page.items.len();
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        if listed >= expected {
            return Ok(());
        }
        if started.elapsed() > SETTLE_TIMEOUT {
            return Err(format!(
                "only {listed} of {expected} writes visible under {}",
                layout.root()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn run_probe(
    engine: &Arc<dyn MemoryEngine>,
    run: u64,
    scenario: &Scenario,
    probe: &Probe,
    phase: &'static str,
    policy: &RecallPolicy,
    timings: &mut Timings,
) -> Result<ProbeResult, Error> {
    let memory = AgentMemory::new(
        engine.clone(),
        layout(run, scenario.name, probe.tenant)?,
        probe.agent,
    )?
    .with_policy(policy.clone());
    let started = Instant::now();
    let pack: ContextPack = match &probe.via {
        Via::Ask => {
            let thread = format!("probe-{}", probe.id);
            memory
                .pre_turn(PreTurn::new(thread, 0, probe.question))
                .await?
                .pack
        }
        Via::Resume { thread, focus } => {
            memory
                .start_session(SessionStart {
                    thread_id: thread.map(str::to_owned),
                    focus: focus.map(str::to_owned),
                })
                .await?
        }
        Via::Compact { thread, dropped } => {
            memory
                .recall_for_compaction(Compaction {
                    thread_id: (*thread).to_string(),
                    dropped: dropped
                        .iter()
                        .map(|text| Turn::new(Role::User, text.as_str()))
                        .collect(),
                    focus: None,
                })
                .await?
        }
        Via::Continue {
            thread,
            turn_index,
            in_prompt_from,
        } => {
            let mut pre = PreTurn::new(*thread, *turn_index, probe.question);
            pre.in_prompt_from = *in_prompt_from;
            memory.pre_turn(pre).await?.pack
        }
    };
    let elapsed = ms(started);
    let result = score(
        scenario.name,
        phase,
        probe,
        &pack.markdown,
        pack.tokens,
        elapsed,
    );
    timings.add(&format!("probe {}", result.via), elapsed);
    Ok(result)
}

fn print_summary(label: &str, reports: &[ScenarioReport], timings: &Timings) {
    println!("\n## Accuracy (`{label}`)\n");
    println!("| Scenario | Phase | Pack hit | Answer | MRR | Fresh first | Leaks |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    let all: Vec<&ProbeResult> = reports.iter().flat_map(|r| &r.probes).collect();
    for report in reports {
        for phase in ["recall", "synthesis"] {
            let t = Totals::of(report.probes.iter().filter(|p| p.phase == phase));
            println!(
                "| {} | {phase} | {} | {} | {:.2} | {} | {} |",
                report.name,
                Totals::pct(t.hits, t.scored),
                Totals::pct(t.answers_ok, t.scored),
                t.mrr,
                Totals::pct(t.fresh_first, t.contradictions),
                if t.leak_checks == 0 {
                    "–".to_string()
                } else {
                    format!("{}/{}", t.leaks, t.leak_checks)
                },
            );
        }
    }
    for phase in ["recall", "synthesis"] {
        let t = Totals::of(all.iter().copied().filter(|p| p.phase == phase));
        println!(
            "| **all** | {phase} | {} | {} | {:.2} | {} | {}/{} |",
            Totals::pct(t.hits, t.scored),
            Totals::pct(t.answers_ok, t.scored),
            t.mrr,
            Totals::pct(t.fresh_first, t.contradictions),
            t.leaks,
            t.leak_checks,
        );
    }

    println!("\n## By question style (recall phase)\n");
    println!("| Style | Pack hit | Answer | MRR |");
    println!("| --- | --- | --- | --- |");
    for style in ["lexical", "paraphrase"] {
        let t = Totals::of(
            all.iter()
                .copied()
                .filter(|p| p.phase == "recall" && p.style == style),
        );
        println!(
            "| {style} | {} | {} | {:.2} |",
            Totals::pct(t.hits, t.scored),
            Totals::pct(t.answers_ok, t.scored),
            t.mrr
        );
    }

    println!("\n## Latency (ms)\n");
    println!("| Step | n | p50 | p95 | max |");
    println!("| --- | --- | --- | --- | --- |");
    for (step, samples) in &timings.0 {
        let l = Latency::of(samples);
        println!(
            "| {step} | {} | {:.1} | {:.1} | {:.1} |",
            l.n, l.p50, l.p95, l.max
        );
    }

    println!("\n## Misses (recall phase)\n");
    for result in all.iter().filter(|p| {
        p.phase == "recall"
            && (p.hit == Some(false) || p.answer_ok == Some(false) || p.leak || p.stale_first)
    }) {
        println!(
            "- {}/{} ({}, {}): hit {:?}, rank {:?}, stale first {}, leak {}; answered {:?}",
            result.scenario,
            result.id,
            result.via,
            result.style,
            result.hit,
            result.rank,
            result.stale_first,
            result.leak,
            result
                .answer
                .as_deref()
                .map(|a| a.chars().take(100).collect::<String>()),
        );
    }
}
