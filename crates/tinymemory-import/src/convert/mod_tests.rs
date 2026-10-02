//! Tests for the legacy value conversions.

use super::*;

#[test]
fn converts_fractional_unix_seconds() {
    let at = from_unix_seconds(1_700_000_000.25).expect("in range");
    assert_eq!(at.timestamp(), 1_700_000_000);
    assert_eq!(at.timestamp_subsec_millis(), 250);
    assert_eq!(from_unix_seconds(f64::NAN), None);
    assert_eq!(from_unix_seconds(f64::INFINITY), None);
    assert_eq!(from_unix_seconds(1e300), None);
}

#[test]
fn converts_unix_millis() {
    let at = from_unix_millis(1_700_000_000_500).expect("in range");
    assert_eq!(at.timestamp_subsec_millis(), 500);
}

#[test]
fn clamps_confidence_and_defaults_unusable_values() {
    assert_eq!(confidence(Some(0.8)), 0.8);
    assert_eq!(confidence(Some(1.7)), 1.0);
    assert_eq!(confidence(Some(-0.2)), 0.0);
    assert_eq!(confidence(Some(f64::NAN)), DEFAULT_CONFIDENCE);
    assert_eq!(confidence(None), DEFAULT_CONFIDENCE);
}

#[test]
fn reads_string_arrays_leniently() {
    assert_eq!(string_array(r#"["a", 1, "", "b"]"#), vec!["a", "b"]);
    assert!(string_array("not json").is_empty());
    assert!(string_array(r#"{"a": 1}"#).is_empty());
}

#[test]
fn reads_objects_and_their_string_fields() {
    let map = object(r#"{"url": "https://x.test", "mime": " ", "n": 1}"#).expect("object");
    assert_eq!(string_field(&map, "url").as_deref(), Some("https://x.test"));
    assert_eq!(string_field(&map, "mime"), None);
    assert_eq!(string_field(&map, "n"), None);
    assert!(object("[]").is_none());
}

#[test]
fn maps_roles_with_unknown_as_user() {
    assert_eq!(role("user"), Role::User);
    assert_eq!(role(" Assistant "), Role::Assistant);
    assert_eq!(role("system"), Role::System);
    assert_eq!(role("tool"), Role::Tool);
    assert_eq!(role("function"), Role::Tool);
    assert_eq!(role("telegram-member"), Role::User);
}

#[test]
fn reads_tool_calls_in_several_shapes() {
    let calls = tool_calls(
        r#"[{"name": "search", "id": "c1"}, {"function": {"name": "fetch"}}, {"x": 1}]"#,
    );
    assert_eq!(
        calls,
        vec![
            ToolCallRef {
                name: "search".into(),
                id: Some("c1".into()),
            },
            ToolCallRef {
                name: "fetch".into(),
                id: None,
            },
        ]
    );
    let wrapped = tool_calls(r#"{"tool_calls": [{"tool": "shell", "call_id": "z"}]}"#);
    assert_eq!(wrapped[0].name, "shell");
    assert_eq!(wrapped[0].id.as_deref(), Some("z"));
    assert_eq!(tool_calls(r#"{"tool_name": "ls"}"#)[0].name, "ls");
    assert!(tool_calls("{oops").is_empty());
    assert!(tool_calls("42").is_empty());
}

#[test]
fn maps_learning_classes() {
    assert_eq!(learning_kind("style"), LearningKind::Preference);
    assert_eq!(learning_kind("channel"), LearningKind::Preference);
    assert_eq!(learning_kind("identity"), LearningKind::Fact);
    assert_eq!(learning_kind("tooling"), LearningKind::Procedure);
    assert_eq!(learning_kind("veto"), LearningKind::Correction);
    assert_eq!(learning_kind("goal"), LearningKind::Other);
    assert_eq!(learning_kind("mystery"), LearningKind::Other);
}

#[test]
fn renders_values_as_text() {
    assert_eq!(value_text(&Value::String("terse".into())), "terse");
    assert_eq!(value_text(&serde_json::json!({"a": 1})), r#"{"a":1}"#);
}
