//! Tests for the ClickUp normaliser.

use super::*;
use serde_json::json;

#[test]
fn extract_tasks_from_data_tasks() {
    let data = json!({ "data": { "tasks": [{"id": "t1"}] } });
    assert_eq!(extract_tasks(&data).len(), 1);
}

#[test]
fn extract_tasks_from_top_level_tasks() {
    let data = json!({ "tasks": [{"id": "a"}, {"id": "b"}] });
    assert_eq!(extract_tasks(&data).len(), 2);
}

#[test]
fn extract_tasks_empty_when_missing() {
    let data = json!({ "foo": "bar" });
    assert!(extract_tasks(&data).is_empty());
}

#[test]
fn extract_task_name_from_top_level() {
    let task = json!({ "id": "t1", "name": "Build feature X" });
    assert_eq!(extract_task_name(&task), Some("Build feature X".into()));
}

#[test]
fn extract_task_name_falls_back_to_data_name() {
    let task = json!({ "data": { "name": "Wrapped" } });
    assert_eq!(extract_task_name(&task), Some("Wrapped".into()));
}

#[test]
fn extract_task_name_none_when_missing() {
    let task = json!({ "id": "t1" });
    assert!(extract_task_name(&task).is_none());
}

#[test]
fn extract_task_updated_handles_string_form() {
    let task = json!({ "date_updated": "1733412345678" });
    assert_eq!(
        extract_task_updated(&task),
        Some("1733412345678".to_string())
    );
}

#[test]
fn extract_task_updated_handles_nested_data() {
    let task = json!({ "data": { "dateUpdated": "1700000000000" } });
    assert_eq!(
        extract_task_updated(&task),
        Some("1700000000000".to_string())
    );
}
