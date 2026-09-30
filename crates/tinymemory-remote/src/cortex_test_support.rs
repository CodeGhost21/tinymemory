//! Test-only knobs for [`CortexMemory`].

use super::CortexMemory;

impl CortexMemory {
    /// Shortens how long a write waits to see its own event, so a test can
    /// reach an outcome-unknown write without the full 30s.
    pub(crate) fn with_visibility_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.inner.dialect_mut().visibility_timeout = timeout;
        self
    }
}
