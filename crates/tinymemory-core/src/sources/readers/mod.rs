//! Source readers: the engine-neutral trait and implementations live in
//! `tinymemory-sources`; this module names them under the path this crate's
//! callers already use and adds the one dispatch decision that is the engine's.
//!
//! The adapters that used to live here (one per kind, each a `&Config` /
//! `Result<_, String>` shell over the `tinymemory-sources` reader) are gone:
//! call sites hand the reader `config.workspace_dir()` themselves.

pub use tinymemory_sources::readers::{
    conversation, folder, github, rss, twitter, web_page, SourceReader,
};

use crate::sources::types::SourceKind;

/// Get the reader for a given source kind, if this crate has one.
///
/// `None` for [`SourceKind::Composio`]. The kind itself stays — records
/// synced from a connected account are still stored, still queried, and still
/// forgotten under it, and removing it would orphan every row already written.
/// What left is the *reading*: an OAuth connector is reached with a credential
/// this crate does not hold and must not, so the host fetches through
/// `tinyconnectors` and hands the records to the memory provider.
///
/// Returning `Option` rather than a stub reader that always errors is
/// deliberate: a caller has to decide what to do about a kind it cannot read,
/// and a stub would let it call and discover the same thing at runtime, once
/// per item.
pub fn reader_for(kind: &SourceKind) -> Option<Box<dyn SourceReader>> {
    match kind {
        SourceKind::Composio => None,
        SourceKind::Conversation => Some(Box::new(conversation::ConversationReader)),
        SourceKind::Folder => Some(Box::new(folder::FolderReader)),
        SourceKind::GithubRepo => Some(Box::new(github::GithubReader)),
        SourceKind::TwitterQuery => Some(Box::new(twitter::TwitterReader)),
        SourceKind::RssFeed => Some(Box::new(rss::RssReader::new())),
        SourceKind::WebPage => Some(Box::new(web_page::WebPageReader)),
    }
}
