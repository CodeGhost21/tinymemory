use super::*;
use crate::test_host::NoHost;
use serde_json::{json, Value};
use tinytools::Tool;

#[test]
fn parameters_schema_is_empty_object() {
    let tool = MemoryStoreKindsTool::new(NoHost);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"], json!({}));
}
