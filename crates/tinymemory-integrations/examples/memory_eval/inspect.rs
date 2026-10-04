//! What CortexDB synthesised, read straight off its wire.
//!
//! The memory API returns items, not the facts, beliefs and conflicts
//! CortexDB derives from them, so the eval reads those itself: `v1/recall`
//! for what a query would find, the list routes (`v1/facts`, `v1/beliefs`,
//! `v1/conflicts`) for everything captured, and `v1/admin/usage` for what
//! the models cost. That is the only way to tell whether memory captured an
//! event even when a pack does not show it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The derived layers of one scope.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Derived {
    pub(crate) scope: String,
    pub(crate) facts: usize,
    pub(crate) beliefs: usize,
    /// Each belief as "subject predicate object (stance, confidence)".
    pub(crate) claims: Vec<String>,
}

/// Reads CortexDB's derived layers below a layout root.
pub(crate) struct Inspector {
    client: reqwest::Client,
    url: String,
    key: String,
}

impl Inspector {
    /// An inspector of the server at `url`.
    pub(crate) fn new(url: &str, key: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_string(),
            key: key.to_string(),
        }
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, reqwest::Error> {
        self.client
            .post(format!("{}/{path}", self.url))
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    /// Every registered scope whose path holds `node` (a layout root such as
    /// `project:eval-1-main`).
    pub(crate) async fn scopes(&self, node: &str) -> Result<Vec<String>, reqwest::Error> {
        let listed: Value = self
            .client
            .get(format!("{}/v1/scopes/list", self.url))
            .query(&[("prefix", "app:tinymemory")])
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(listed["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["path"].as_str())
            .filter(|path| path.split('/').any(|segment| segment == node))
            .map(str::to_owned)
            .collect())
    }

    /// The facts and beliefs of `scope` relevant to `query`.
    pub(crate) async fn derived(
        &self,
        scope: &str,
        query: &str,
    ) -> Result<Derived, reqwest::Error> {
        let pack = self
            .post(
                "v1/recall",
                &json!({
                    "scope": scope,
                    "query": query,
                    "budgets": { "per_layer_limits": {
                        "events": 1, "facts": 50, "beliefs": 50,
                        "episodes": 0, "understanding": 0,
                    }},
                }),
            )
            .await?;
        let layer = |name: &str| {
            pack.pointer(&format!("/layers/{name}"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let beliefs = layer("beliefs");
        Ok(Derived {
            scope: scope.to_string(),
            facts: layer("facts").len(),
            beliefs: beliefs.len(),
            claims: beliefs.iter().map(claim).collect(),
        })
    }
}

/// A belief as one readable line.
fn claim(belief: &Value) -> String {
    format!(
        "{} {} {} ({}, {:.2})",
        part(belief.pointer("/claim/subject")),
        part(belief.pointer("/claim/predicate")),
        part(belief.pointer("/claim/object")),
        belief["stance"].as_str().unwrap_or("?"),
        belief["confidence"].as_f64().unwrap_or_default(),
    )
}

/// Everything CortexDB derived below a layout root, as readable lines.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Captured {
    pub(crate) facts: Vec<String>,
    pub(crate) beliefs: Vec<String>,
    /// Each open or resolved conflict as "kind status: subject predicate
    /// [values]".
    pub(crate) conflicts: Vec<String>,
    /// Beliefs by stance (`supported`, `contested`, …).
    pub(crate) stances: BTreeMap<String, usize>,
    /// Every belief's confidence.
    pub(crate) confidences: Vec<f64>,
}

impl Captured {
    /// Whether any fact or belief mentions `needle`.
    pub(crate) fn mentions(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.facts
            .iter()
            .chain(&self.beliefs)
            .any(|line| line.to_lowercase().contains(&needle))
    }

    /// Adds everything `other` holds.
    pub(crate) fn extend(&mut self, other: Self) {
        self.facts.extend(other.facts);
        self.beliefs.extend(other.beliefs);
        self.conflicts.extend(other.conflicts);
        for (stance, n) in other.stances {
            *self.stances.entry(stance).or_default() += n;
        }
        self.confidences.extend(other.confidences);
    }
}

/// One line of the models' usage.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub(crate) struct Spend {
    pub(crate) calls: u64,
    pub(crate) tokens: u64,
    pub(crate) cost_usd: f64,
}

impl Spend {
    /// A usage line as `v1/admin/usage` reports it.
    fn of(line: &Value) -> Self {
        let count = |name: &str| line[name].as_u64().unwrap_or_default();
        Self {
            calls: count("calls"),
            tokens: count("tokens_total") + count("tokens_unsplit"),
            cost_usd: line["cost_usd"].as_f64().unwrap_or_default(),
        }
    }

    /// What was spent between `before` and `self`.
    fn since(self, before: Self) -> Self {
        Self {
            calls: self.calls.saturating_sub(before.calls),
            tokens: self.tokens.saturating_sub(before.tokens),
            cost_usd: self.cost_usd - before.cost_usd,
        }
    }
}

/// The models' usage CortexDB accounts for, as its routers price it: in
/// total, and by the role a model plays (extraction, enrichment, answer,
/// embedding, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Usage {
    pub(crate) calls: u64,
    pub(crate) tokens: u64,
    pub(crate) cost_usd: f64,
    pub(crate) by_role: BTreeMap<String, Spend>,
}

impl Usage {
    /// What was spent between `before` and `self`.
    pub(crate) fn since(&self, before: &Self) -> Self {
        let total = self.total().since(before.total());
        Self {
            calls: total.calls,
            tokens: total.tokens,
            cost_usd: total.cost_usd,
            by_role: self
                .by_role
                .iter()
                .map(|(role, spend)| {
                    let earlier = before.by_role.get(role).copied().unwrap_or_default();
                    (role.clone(), spend.since(earlier))
                })
                .filter(|(_, spend)| spend.calls > 0 || spend.cost_usd > 0.0)
                .collect(),
        }
    }

    fn total(&self) -> Spend {
        Spend {
            calls: self.calls,
            tokens: self.tokens,
            cost_usd: self.cost_usd,
        }
    }
}

/// A claim's part (`subject`, `predicate` or `object`) as text.
fn part(value: Option<&Value>) -> String {
    value
        .and_then(|v| v.get("name").or_else(|| v.get("value")).or(Some(v)))
        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
        .unwrap_or_default()
}

/// The array of a list answer, whatever it is called.
fn listed(answer: &Value, field: &str) -> Vec<Value> {
    answer
        .get(field)
        .or_else(|| answer.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

impl Inspector {
    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, reqwest::Error> {
        self.client
            .get(format!("{}/{path}", self.url))
            .query(query)
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    /// Every fact, belief and conflict CortexDB holds in `scopes`.
    pub(crate) async fn captured(&self, scopes: &[String]) -> Result<Captured, reqwest::Error> {
        let mut captured = Captured::default();
        for scope in scopes {
            let page = [("scope", scope.as_str()), ("limit", "200")];
            for fact in listed(&self.get("v1/facts", &page).await?, "facts") {
                captured.facts.push(format!(
                    "{} {} {}",
                    part(fact.pointer("/subject")),
                    part(fact.pointer("/predicate")),
                    part(fact.pointer("/object")),
                ));
            }
            for belief in listed(&self.get("v1/beliefs", &page).await?, "beliefs") {
                captured.beliefs.push(claim(&belief));
                let stance = belief["stance"].as_str().unwrap_or("unknown");
                *captured.stances.entry(stance.to_string()).or_default() += 1;
                if let Some(confidence) = belief["confidence"].as_f64() {
                    captured.confidences.push(confidence);
                }
            }
            for conflict in listed(&self.get("v1/conflicts", &page).await?, "conflicts") {
                let values: Vec<String> = conflict["records"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|record| record["object"].as_str().unwrap_or("?").to_string())
                    .collect();
                captured.conflicts.push(format!(
                    "{} {}: {} {} [{}]",
                    conflict["kind"].as_str().unwrap_or("?"),
                    conflict["status"].as_str().unwrap_or("?"),
                    conflict["subject"].as_str().unwrap_or("?"),
                    conflict["predicate"].as_str().unwrap_or("?"),
                    values.join(" | "),
                ));
            }
        }
        Ok(captured)
    }

    /// The enrichment queue (fact extraction): jobs not yet done, and jobs
    /// ever queued.
    pub(crate) async fn enrichment(&self) -> Result<(u64, u64), reqwest::Error> {
        let report = self.get("v1/admin/usage", &[]).await?;
        let field = |name: &str| {
            report
                .pointer(&format!("/enrichment_backlog/{name}"))
                .and_then(Value::as_u64)
                .unwrap_or_default()
        };
        Ok((field("jobs_pending"), field("jobs_queued_total")))
    }

    /// The models' usage so far.
    pub(crate) async fn usage(&self) -> Result<Usage, reqwest::Error> {
        let report = self.get("v1/admin/usage", &[]).await?;
        let total = Spend::of(&report["total"]);
        // A map keyed by role, or a list of lines that each name theirs.
        let by_role = match &report["by_role"] {
            Value::Object(roles) => roles
                .iter()
                .map(|(role, line)| (role.clone(), Spend::of(line)))
                .collect(),
            Value::Array(lines) => lines
                .iter()
                .map(|line| {
                    let role = line["role"].as_str().unwrap_or("unknown");
                    (role.to_string(), Spend::of(line))
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        Ok(Usage {
            calls: total.calls,
            tokens: total.tokens,
            cost_usd: total.cost_usd,
            by_role,
        })
    }

    /// The server's version and the capabilities it advertises.
    pub(crate) async fn version(&self) -> Result<Value, reqwest::Error> {
        self.get("v1/admin/version", &[]).await
    }
}
