//! The CortexDB memory engine.

mod credential;
mod descriptor;
mod envelope;
mod error;
mod log;
mod transport;

pub use credential::{BearerSource, CortexCredential, StaticBearer};
pub use descriptor::{
    CORTEX_API_ENDPOINT, CORTEXDB_ENGINE_ID, CortexWire, TINYHUMANS_API_ENDPOINT,
    TINYHUMANS_ENGINE_ID, cortexdb_descriptor, tinyhumans_descriptor,
};
pub use error::{Error, INSUFFICIENT_CREDITS_CODE, Result, error_code, is_insufficient_credits};
