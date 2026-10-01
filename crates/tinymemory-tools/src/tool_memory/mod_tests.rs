use super::*;
use crate::test_host::NoHost;
use tinytools::Tool;

#[test]
fn exports_memory_tool_wrappers_with_stable_names() {
    assert_eq!(MemoryToolsListTool::new(NoHost).name(), "memory_tools_list");
    assert_eq!(MemoryToolsPutTool::new(NoHost).name(), "memory_tools_put");
}
