//! Capability-accurate CortexDB provider composition.

mod families;
mod operations;
mod portability;
mod types;

pub use operations::CortexProvider;

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_support;
