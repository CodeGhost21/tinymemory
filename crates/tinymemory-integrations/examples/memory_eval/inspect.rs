//! What CortexDB synthesised, read straight off its wire.
//!
//! The memory API returns items, not the facts, beliefs and conflicts
//! CortexDB derives from them, so the eval reads those itself: `v1/recall`
//! for what a query would find, the list routes (`v1/facts`, `v1/beliefs`,
//! `v1/conflicts`) for everything captured, and `v1/admin/usage` for what
//! the models cost. That is the only way to tell whether memory captured an
//! event even when a pack does not show it.

use serde::Serialize;
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
}

/// The models' usage CortexDB accounts for, as its routers price it.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct Usage {
    pub(crate) calls: u64,
    pub(crate) tokens: u64,
    pub(crate) cost_usd: f64,
}

impl Usage {
    /// What was spent between `before` and `self`.
    pub(crate) fn since(self, before: Self) -> Self {
        Self {
            calls: self.calls.saturating_sub(before.calls),
            tokens: self.tokens.saturating_sub(before.tokens),
            cost_usd: self.cost_usd - before.cost_usd,
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

    /// Enrichment jobs (fact extraction) not yet done.
    pub(crate) async fn enrichment_pending(&self) -> Result<u64, reqwest::Error> {
        let report = self.get("v1/admin/usage", &[]).await?;
        Ok(report
            .pointer("/enrichment_backlog/jobs_pending")
            .and_then(Value::as_u64)
            .unwrap_or_default())
    }

    /// The models' usage so far.
    pub(crate) async fn usage(&self) -> Result<Usage, reqwest::Error> {
        let report = self.get("v1/admin/usage", &[]).await?;
        let total = &report["total"];
        Ok(Usage {
            calls: total["calls"].as_u64().unwrap_or_default(),
            tokens: total["tokens_total"].as_u64().unwrap_or_default()
                + total["tokens_unsplit"].as_u64().unwrap_or_default(),
            cost_usd: total["cost_usd"].as_f64().unwrap_or_default(),
        })
    }
}
