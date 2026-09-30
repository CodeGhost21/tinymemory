use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn exports_memory_store_tools_with_stable_names() {
    assert_eq!(MemoryStoreKindsTool::new(NoHost).name(), "memory_store_kinds");
    assert_eq!(MemoryStoreRawChunksTool::new(NoHost).name(), "memory_store_raw_chunks");
    assert_eq!(MemoryStoreRawSearchTool::new(NoHost).name(), "memory_store_raw_search");
}
