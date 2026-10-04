//! What CortexDB synthesised, read straight off its wire.
//!
//! The memory API returns items, not the facts and beliefs CortexDB derives
//! from them, so the eval asks CortexDB's `v1/recall` for those layers
//! itself. That is the only way to tell whether a build produced anything
//! and whether a pack could have used it.

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
    let part = |pointer: &str| {
        let value = belief.pointer(pointer);
        value
            .and_then(|v| v.get("name").or_else(|| v.get("value")).or(Some(v)))
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .unwrap_or_default()
    };
    format!(
        "{} {} {} ({}, {:.2})",
        part("/claim/subject"),
        part("/claim/predicate"),
        part("/claim/object"),
        belief["stance"].as_str().unwrap_or("?"),
        belief["confidence"].as_f64().unwrap_or_default(),
    )
}
