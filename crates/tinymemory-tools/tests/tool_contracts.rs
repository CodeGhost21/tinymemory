//! Every tool's name, description, parameter schema, exposure and permission
//! level, pinned against a literal fixture.
//!
//! These are wire contracts: the model reads the schema on every provider call
//! and saved transcripts and skills name the tools. The fixture was captured
//! from the tools as they stood in OpenHuman's `memory::tools` and
//! `memory::query` before they moved here; a diff in it is a change to what the
//! model is told, not a refactor.

mod common;

use common::TestHost;
use serde_json::{json, Value};
use tinymemory_tools::query::{
    MemoryTreeCoverWindowTool, MemoryTreeDrillDownTool, MemoryTreeFetchLeavesTool,
    MemoryTreeQuerySourceTool, MemoryTreeSearchEntitiesTool, MemoryTreeTool,
};
use tinymemory_tools::raw_store::{
    MemoryStoreKindsTool, MemoryStoreRawChunksTool, MemoryStoreRawSearchTool,
};
use tinymemory_tools::search::{
    MemoryChunkContextTool, MemoryHybridSearchTool, MemoryVectorSearchTool,
};
use tinymemory_tools::tool_memory::{MemoryToolsListTool, MemoryToolsPutTool};
use tinytools::Tool;

fn contract(tool: &dyn Tool) -> Value {
    json!({
        "name": tool.name(),
        "description": tool.description(),
        "parameters_schema": tool.parameters_schema(),
        "exposure": format!("{:?}", tool.exposure()),
        "permission_level": format!("{:?}", tool.permission_level()),
    })
}

#[test]
fn tool_contracts_match_the_recorded_fixture() {
    let host = TestHost::bound();
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(MemoryChunkContextTool::new(host.clone())),
        Box::new(MemoryHybridSearchTool::new(host.clone())),
        Box::new(MemoryVectorSearchTool::new(host.clone())),
        Box::new(MemoryStoreKindsTool::new(host.clone())),
        Box::new(MemoryStoreRawChunksTool::new(host.clone())),
        Box::new(MemoryStoreRawSearchTool::new(host.clone())),
        Box::new(MemoryToolsListTool::new(host.clone())),
        Box::new(MemoryToolsPutTool::new(host.clone())),
        Box::new(MemoryTreeDrillDownTool::new(host.clone())),
        Box::new(MemoryTreeFetchLeavesTool::new(host.clone())),
        Box::new(MemoryTreeQuerySourceTool::new(host.clone())),
        Box::new(MemoryTreeSearchEntitiesTool::new(host.clone())),
        Box::new(MemoryTreeCoverWindowTool::new(host.clone())),
        Box::new(MemoryTreeTool::new(host.clone())),
    ];
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/tool_contracts.json")).expect("fixture parses");
    let fixture = fixture.as_object().expect("fixture is an object");
    assert_eq!(tools.len(), fixture.len(), "every fixture entry has a tool");
    for tool in &tools {
        let expected = fixture
            .get(tool.name())
            .unwrap_or_else(|| panic!("no fixture entry for {}", tool.name()));
        assert_eq!(&contract(tool.as_ref()), expected, "{}", tool.name());
    }
}
