//! Capability-accurate CortexDB provider composition.

mod operations;
mod portability;
mod types;

pub use operations::CortexProvider;

#[cfg(test)]
mod test;
