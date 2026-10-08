//! Waiting out a rate limit before the next request.
//!
//! The TinyHumans backend gives each user one memory budget (300 requests a
//! minute) and answers past it with `429`, saying when to come back in
//! `Retry-After` or `RateLimit-Reset` (seconds). Retrying sooner only spends
//! the next attempt on another `429`: a hosted write has three attempts and a
//! visibility poll treats a `429` as "not yet" until its deadline, so a
//! burst of them turned into failed writes and false "not readable" errors.
//!
//! [`RatePause`] remembers the latest such answer, shared by the clones of
//! one client, and every request first waits until it has passed. The wait
//! is capped at [`MAX_PAUSE`], so a server that asks for minutes cannot
//! stall a caller past its own deadlines.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::header::HeaderMap;
use tokio::time::Instant;

/// The longest a rate limit holds requests back.
pub(crate) const MAX_PAUSE: Duration = Duration::from_secs(10);

/// When the last rate limit lifts; clones share it.
#[derive(Clone, Debug, Default)]
pub(crate) struct RatePause(Arc<Mutex<Option<Instant>>>);

impl RatePause {
    /// Sleeps until the latest rate limit has lifted, if it has not. The
    /// pause is read again after each sleep: a `429` another request got
    /// meanwhile may have extended it.
    pub(crate) async fn wait(&self) {
        loop {
            let until = *self.0.lock().unwrap_or_else(PoisonError::into_inner);
            match until.filter(|until| *until > Instant::now()) {
                Some(until) => tokio::time::sleep_until(until).await,
                None => return,
            }
        }
    }

    /// Records a `429` answer's `headers`: later requests wait as long as
    /// they ask, at most [`MAX_PAUSE`]. Nothing is recorded without a wait.
    pub(crate) fn note(&self, headers: &HeaderMap) {
        let Some(wait) = retry_after(headers) else {
            return;
        };
        let until = Instant::now() + wait;
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_none_or(|held| held < until) {
            *held = Some(until);
        }
    }
}

/// How long a `429` asks the caller to wait: `Retry-After`, else
/// `RateLimit-Reset`, in whole seconds, at most [`MAX_PAUSE`]. `None` when
/// neither is a number (an HTTP-date `Retry-After` is not honoured).
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    ["retry-after", "ratelimit-reset"]
        .iter()
        .find_map(|name| {
            headers
                .get(*name)?
                .to_str()
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|seconds| Duration::from_secs(seconds).min(MAX_PAUSE))
        .filter(|wait| !wait.is_zero())
}

#[cfg(test)]
#[path = "pause_tests.rs"]
mod tests;
