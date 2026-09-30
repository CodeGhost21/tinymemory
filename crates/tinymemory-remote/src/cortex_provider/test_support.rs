//! Test-only knobs for [`CortexProvider`].

use super::CortexProvider;

impl CortexProvider {
    /// Replaces the pauses a hosted import takes while the backend is
    /// unavailable, so a test need not wait out a real rate-limit window.
    pub(crate) fn with_import_patience(mut self, pauses: Vec<std::time::Duration>) -> Self {
        self.import_patience = pauses;
        self
    }
}
