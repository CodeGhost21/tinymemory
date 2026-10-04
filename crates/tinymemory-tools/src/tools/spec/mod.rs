//! What the model is told about each tool: its name, a description, and a
//! JSON Schema for its arguments.
//!
//! [`ToolSpec`] is deliberately runtime-neutral: a host maps `name`,
//! `description` and `parameters` onto whatever its tool runtime calls them
//! (an MCP tool, an OpenAI function, a `tinytools` tool). The schemas never
//! mention a namespace or a reach; those are fixed by the host on
//! [`super::MemoryTools`] and are not the model's to choose.

pub(crate) mod schema;

use serde::Serialize;
use tinymemory_api::FetchMode;

/// `memory_recall`: a question in, a synthesised answer with citations out.
pub const MEMORY_RECALL: &str = "memory_recall";
/// `memory_fetch`: raw keyword, vector or hybrid retrieval.
pub const MEMORY_FETCH: &str = "memory_fetch";
/// `memory_list`: page through stored memories with no query.
pub const MEMORY_LIST: &str = "memory_list";
/// `memory_get`: read memories whole by id.
pub const MEMORY_GET: &str = "memory_get";
/// `memory_explore`: count stored memories per value of one facet.
pub const MEMORY_EXPLORE: &str = "memory_explore";
/// `memory_store`: store a learning, a document or a conversation.
pub const MEMORY_STORE: &str = "memory_store";
/// `memory_forget`: remove memories by id or by a non-empty filter.
pub const MEMORY_FORGET: &str = "memory_forget";

/// Every tool name, reads first, in the order [`super::MemoryTools::specs`]
/// lists them.
pub const TOOL_NAMES: [&str; 7] = [
    MEMORY_RECALL,
    MEMORY_FETCH,
    MEMORY_LIST,
    MEMORY_GET,
    MEMORY_EXPLORE,
    MEMORY_STORE,
    MEMORY_FORGET,
];

/// The tools that change what memory holds; a read-only
/// [`super::MemoryTools`] neither lists nor runs them.
pub const WRITE_TOOL_NAMES: [&str; 2] = [MEMORY_STORE, MEMORY_FORGET];

/// One tool as a model sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    /// The tool's name, one of [`TOOL_NAMES`].
    pub name: &'static str,
    /// What the tool does and when to use it, written for the model.
    pub description: &'static str,
    /// A JSON Schema object describing the arguments. Every object in it sets
    /// `additionalProperties: false`.
    pub parameters: serde_json::Value,
}

/// Whether `name` is one of the write tools.
pub(crate) fn is_write_tool(name: &str) -> bool {
    WRITE_TOOL_NAMES.contains(&name)
}

/// The specs for an engine serving `fetch_modes`, with the write tools only
/// when `writes`. `memory_fetch` is left out when the engine serves no mode.
pub(crate) fn specs(fetch_modes: &[FetchMode], writes: bool) -> Vec<ToolSpec> {
    let mut specs = vec![ToolSpec {
        name: MEMORY_RECALL,
        description: "Answer a question from long-term memory. Returns a synthesised answer \
                      and the memories it cites. Use this first when you need to know what \
                      is remembered about something.",
        parameters: schema::recall(),
    }];
    if !fetch_modes.is_empty() {
        specs.push(ToolSpec {
            name: MEMORY_FETCH,
            description: "Search long-term memory and return the raw matching memories, best \
                          first. Use it when you need the stored text itself rather than an \
                          answer. Pass `cursor` from a previous result to get the next page.",
            parameters: schema::fetch(fetch_modes),
        });
    }
    specs.extend([
        ToolSpec {
            name: MEMORY_LIST,
            description: "Page through stored memories without a query, optionally narrowed \
                          by a filter. Pass `cursor` from a previous result to get the next \
                          page.",
            parameters: schema::list(),
        },
        ToolSpec {
            name: MEMORY_GET,
            description: "Read memories whole by id (ids come from recall citations, fetch \
                          and list results). Ids that name nothing you can see are reported \
                          as missing.",
            parameters: schema::get(),
        },
        ToolSpec {
            name: MEMORY_EXPLORE,
            description: "Count stored memories per value of one facet (kind, source, \
                          folder, thread, tag, ...), largest first, to see what memory \
                          holds before narrowing a filter.",
            parameters: schema::explore(),
        },
    ]);
    if writes {
        specs.extend([
            ToolSpec {
                name: MEMORY_STORE,
                description: "Store one memory: exactly one of `learning` (a distilled \
                              statement worth remembering: a preference, fact, procedure or \
                              correction), `document` (a titled text) or `conversation` \
                              (ordered turns). Storing the same memory twice is harmless.",
                parameters: schema::store(),
            },
            ToolSpec {
                name: MEMORY_FORGET,
                description: "Remove memories, either by `ids` or by a non-empty `filter` \
                              (never both). Ids you cannot see are skipped and reported.",
                parameters: schema::forget(),
            },
        ]);
    }
    specs
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
