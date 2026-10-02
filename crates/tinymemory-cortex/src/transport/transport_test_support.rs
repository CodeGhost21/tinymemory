//! Test-only knobs for [`HttpClient`].

use std::time::Duration;

use super::HttpClient;

impl HttpClient {
    /// Shortens the read-retry backoff.
    pub(crate) fn set_read_backoff(&mut self, backoff: Duration) {
        self.read_backoff = backoff;
    }
}
