//! The value types the capability families exchange.
//!
//! These sit under `provider` because that is where they live in
//! `tinymemory-api`, which re-exports every one of them at its historical path.
//! Keeping the two trees the same shape is what makes the split auditable: a
//! type is either here, as data, or there, as a trait — and which one it is can
//! be read off the path.
//!
//! The traits themselves are **not** here and will not be. A trait is a driver
//! obligation; this crate describes a frame. See [`crate`] for the rest of that
//! argument.

pub mod chunks;
pub mod diagnosis;
pub mod episodic;
pub mod episodic_portability;
pub mod people;
pub mod profile;
pub mod retrieval;
pub mod sessions;
pub mod sync;
pub mod types;
