//! Reading a tool call's JSON arguments, strictly.
//!
//! [`Args`] wraps one JSON object and refuses, with
//! [`tinymemory_api::Error::InvalidRequest`] naming the tool and the field:
//!
//! - a `namespace` or `reach` key at any level, with a message saying the host
//!   fixes it (a model that tries to pick a namespace is told so, not quietly
//!   ignored);
//! - any other key the tool's schema does not list;
//! - a value of the wrong type or out of range.
//!
//! An explicit `null` reads as absent, since many models send `null` for an
//! optional argument they mean to leave out.

mod filter;

pub(crate) use filter::{facet, fetch_mode, meta_filter};

use serde_json::{Map, Value};
use tinymemory_api::{Error, Result};

/// Keys a model may never pass, at any depth.
const HOST_FIXED: [&str; 2] = ["namespace", "reach"];

/// One JSON object of arguments, checked against the keys a tool accepts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Args<'a> {
    tool: &'static str,
    path: &'a str,
    map: &'a Map<String, Value>,
}

/// An empty object, what `null` arguments read as.
static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);

impl<'a> Args<'a> {
    /// The top-level arguments of `tool`, accepting only `allowed` keys.
    /// `null` reads as no arguments.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when `value` is not an object, names a
    /// host-fixed key, or names a key outside `allowed`.
    pub(crate) fn parse(tool: &'static str, value: &'a Value, allowed: &[&str]) -> Result<Self> {
        let map = match value {
            Value::Null => &*EMPTY,
            Value::Object(map) => map,
            _ => return Err(invalid(tool, "arguments must be a json object")),
        };
        let args = Self {
            tool,
            path: "",
            map,
        };
        args.check_keys(allowed)?;
        Ok(args)
    }

    /// The nested object at `key`, accepting only `allowed` keys; `None` when
    /// absent. `path` is how errors name it, normally `"{key}."`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is not an object, or its keys
    /// break the rules [`Args::parse`] enforces.
    pub(crate) fn object(
        &self,
        key: &str,
        path: &'a str,
        allowed: &[&str],
    ) -> Result<Option<Args<'a>>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Object(map)) => {
                let nested = Args {
                    tool: self.tool,
                    path,
                    map,
                };
                nested.check_keys(allowed)?;
                Ok(Some(nested))
            }
            Some(_) => Err(self.field_error(key, "must be an object")),
        }
    }

    /// One element of the array at `key`, read as an object accepting only
    /// `allowed` keys. `path` is how errors name its fields.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the element is not an object, or its
    /// keys break the rules [`Args::parse`] enforces.
    pub(crate) fn element<'b>(
        &self,
        key: &str,
        value: &'b Value,
        path: &'b str,
        allowed: &[&str],
    ) -> Result<Args<'b>> {
        let Value::Object(map) = value else {
            return Err(self.field_error(key, "must hold only objects"));
        };
        let element = Args {
            tool: self.tool,
            path,
            map,
        };
        element.check_keys(allowed)?;
        Ok(element)
    }

    /// The tool these arguments belong to.
    pub(crate) fn tool(&self) -> &'static str {
        self.tool
    }

    /// Whether `key` is present and not `null`.
    pub(crate) fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// An optional string.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is not a string.
    pub(crate) fn string(&self, key: &str) -> Result<Option<String>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(self.field_error(key, "must be a string")),
        }
    }

    /// A required, non-blank string.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is missing, blank or not a
    /// string.
    pub(crate) fn required_string(&self, key: &str) -> Result<String> {
        match self.string(key)? {
            Some(value) if !value.trim().is_empty() => Ok(value),
            _ => Err(self.field_error(key, "is required and must not be blank")),
        }
    }

    /// An integer in `1..=max`, `default` when absent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is not an integer in range.
    pub(crate) fn count(&self, key: &str, default: usize, max: usize) -> Result<usize> {
        let Some(value) = self.get(key) else {
            return Ok(default);
        };
        value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| self.field_error(key, &format!("must be an integer from 1 to {max}")))
    }

    /// A number in `0.0..=1.0`, `default` when absent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is not a number in range.
    pub(crate) fn unit(&self, key: &str, default: f32) -> Result<f32> {
        let Some(value) = self.get(key) else {
            return Ok(default);
        };
        value
            .as_f64()
            .filter(|n| (0.0..=1.0).contains(n))
            // In range, so the narrowing loses only precision.
            .map(|n| n as f32)
            .ok_or_else(|| self.field_error(key, "must be a number from 0 to 1"))
    }

    /// An optional list of strings; empty when absent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is not an array of strings.
    pub(crate) fn strings(&self, key: &str) -> Result<Vec<String>> {
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| self.field_error(key, "must be an array of strings"))
                })
                .collect(),
            Some(_) => Err(self.field_error(key, "must be an array of strings")),
        }
    }

    /// An optional array; empty when absent.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] when the value is not an array.
    pub(crate) fn array(&self, key: &str) -> Result<&'a [Value]> {
        match self.get(key) {
            None => Ok(&[]),
            Some(Value::Array(values)) => Ok(values),
            Some(_) => Err(self.field_error(key, "must be an array")),
        }
    }

    /// An [`Error::InvalidRequest`] naming `key` as `{tool}: `{path}{key}` {problem}`.
    pub(crate) fn field_error(&self, key: &str, problem: &str) -> Error {
        invalid(self.tool, &format!("`{}{key}` {problem}", self.path))
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key).filter(|value| !value.is_null())
    }

    fn check_keys(&self, allowed: &[&str]) -> Result<()> {
        if let Some(key) = self.map.keys().find(|key| HOST_FIXED.contains(&key.as_str())) {
            return Err(self.field_error(
                key,
                "is fixed by the host and cannot be passed to a memory tool",
            ));
        }
        if let Some(key) = self
            .map
            .keys()
            .find(|key| !allowed.contains(&key.as_str()))
        {
            return Err(self.field_error(key, "is not an argument of this tool"));
        }
        Ok(())
    }
}

/// An [`Error::InvalidRequest`] prefixed with the tool's name.
pub(crate) fn invalid(tool: &str, message: &str) -> Error {
    Error::InvalidRequest(format!("{tool}: {message}"))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
