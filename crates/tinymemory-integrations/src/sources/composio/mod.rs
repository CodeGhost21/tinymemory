//! Composio toolkit payloads: normalisers and the mapping to `StoreItem`s.
//!
//! A host runs Composio actions with its own credentials and hands the raw
//! responses here. Two layers turn them into memory:
//!
//! 1. **Normalisers**, one module per toolkit, are pure
//!    `serde_json::Value` transforms: they walk Composio's envelope variants
//!    and pull out the tasks, issues, pages or messages
//!    ([`clickup`], [`github`], [`linear`], [`notion`]), or rewrite a verbose
//!    response into a slim one in place ([`gmail_post_process`],
//!    [`slack_post_process`]). [`fields`] holds the path lookup they share.
//! 2. [`normalise_payload`] turns one (post-processed) response into
//!    [`ComposioDocument`]s, and [`payload_items`] turns those into
//!    [`StoreItem::Document`](tinymemory_api::StoreItem::Document)s with
//!    `source.kind = Composio`, `source.id` = the connection or source id,
//!    `url` and `observed_at` where the payload has them, and
//!    `tags = [toolkit]`.
//!
//! Nothing here holds a credential, opens a socket or decides when to sync.
//!
//! One caveat on "pure": [`gmail_post_process::format_email_local_time`]
//! renders in `chrono::Local`, so it reads the host's timezone. The raw UTC
//! fields are preserved alongside, so ordering and identity stay UTC-based.

pub mod clickup;
pub mod github;
pub mod gmail_post_process;
pub mod fields;
pub mod linear;
pub mod notion;
pub mod slack_post_process;

mod documents;

pub use documents::{ComposioDocument, normalise_payload, payload_items};
