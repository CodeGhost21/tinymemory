//! Strict argument reading: host-fixed keys, unknown keys, types and ranges.

use super::*;
use serde_json::json;

fn message(error: Error) -> String {
    match error {
        Error::InvalidRequest(message) => message,
        other => panic!("expected an invalid request, got {other:?}"),
    }
}

#[test]
fn null_reads_as_no_arguments() {
    let value = Value::Null;
    let args = Args::parse("memory_list", &value, &["limit"]).unwrap();
    assert!(!args.has("limit"));
    assert_eq!(args.count("limit", 7, 50).unwrap(), 7);
}

#[test]
fn arguments_that_are_not_an_object_are_refused() {
    let value = json!([1, 2]);
    let error = Args::parse("memory_list", &value, &[]).unwrap_err();
    assert_eq!(
        message(error),
        "memory_list: arguments must be a json object"
    );
}

#[test]
fn a_namespace_or_reach_key_is_refused_as_host_fixed() {
    for key in ["namespace", "reach"] {
        let value = json!({ key: "agent:other" });
        let error = Args::parse("memory_list", &value, &["limit"]).unwrap_err();
        let text = message(error);
        assert!(text.contains(&format!("`{key}`")), "{text}");
        assert!(text.contains("fixed by the host"), "{text}");
    }
}

#[test]
fn a_host_fixed_key_is_refused_even_when_listed_as_allowed() {
    let value = json!({ "namespace": "root" });
    assert!(Args::parse("memory_list", &value, &["namespace"]).is_err());
}

#[test]
fn a_nested_host_fixed_key_is_refused_with_its_path() {
    let value = json!({ "filter": { "reach": { "at": "root" } } });
    let error = Args::parse("memory_list", &value, &["filter"]).unwrap_err();
    assert!(message(error).contains("`filter.reach` is fixed by the host"));
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let value = json!({ "limt": 3 });
    let error = Args::parse("memory_list", &value, &["limit"]).unwrap_err();
    assert_eq!(
        message(error),
        "memory_list: `limt` is not an argument of this tool"
    );
}

#[test]
fn counts_default_and_are_range_checked() {
    let value = json!({ "a": 5, "b": 0, "c": 51, "d": "5", "e": -1 });
    let args = Args::parse("t", &value, &["a", "b", "c", "d", "e"]).unwrap();
    assert_eq!(args.count("a", 1, 50).unwrap(), 5);
    assert_eq!(args.count("missing", 9, 50).unwrap(), 9);
    for key in ["b", "c", "d", "e"] {
        let text = message(args.count(key, 1, 50).unwrap_err());
        assert!(
            text.contains(&format!("`{key}` must be an integer from 1 to 50")),
            "{text}"
        );
    }
}

#[test]
fn units_default_and_are_range_checked() {
    let value = json!({ "ok": 0.25, "high": 1.5, "text": "high" });
    let args = Args::parse("t", &value, &["ok", "high", "text"]).unwrap();
    assert!((args.unit("ok", 0.8).unwrap() - 0.25).abs() < f32::EPSILON);
    assert!((args.unit("missing", 0.8).unwrap() - 0.8).abs() < f32::EPSILON);
    assert!(args.unit("high", 0.8).is_err());
    assert!(args.unit("text", 0.8).is_err());
}

#[test]
fn strings_are_type_checked() {
    let value = json!({ "s": 1, "blank": "  ", "list": ["a", 2], "notlist": "a", "null": null });
    let args = Args::parse("t", &value, &["s", "blank", "list", "notlist", "null"]).unwrap();
    assert!(message(args.string("s").unwrap_err()).contains("`s` must be a string"));
    assert!(message(args.required_string("blank").unwrap_err()).contains("must not be blank"));
    assert!(args.required_string("null").is_err());
    assert!(args.strings("list").is_err());
    assert!(args.strings("notlist").is_err());
    assert!(args.strings("null").unwrap().is_empty());
    assert!(args.array("notlist").is_err());
}

#[test]
fn a_nested_value_that_is_not_an_object_is_refused() {
    let value = json!({ "filter": "kinds=learning" });
    let args = Args::parse("t", &value, &["filter"]).unwrap();
    assert!(
        message(args.object("filter", "filter.", &[]).unwrap_err()).contains("must be an object")
    );
}

#[test]
fn an_array_element_that_is_not_an_object_is_refused() {
    let value = json!({ "turns": ["hi"] });
    let args = Args::parse("t", &value, &["turns"]).unwrap();
    let element = &args.array("turns").unwrap()[0];
    let error = args
        .element("turns", element, "turns[].", &["text"])
        .unwrap_err();
    assert!(message(error).contains("`turns` must hold only objects"));
}

#[test]
fn a_host_fixed_key_is_found_at_any_depth() {
    let value = json!({ "conversation": { "turns": [{ "role": "user", "reach": "x" }] } });
    let error = Args::parse("memory_store", &value, &["conversation"]).unwrap_err();
    let text = message(error);
    assert!(
        text.contains("`conversation.turns[].reach` is fixed by the host"),
        "{text}"
    );

    let value = json!({ "unknown": { "namespace": "root" } });
    let text = message(Args::parse("memory_get", &value, &["ids"]).unwrap_err());
    assert!(
        text.contains("`unknown.namespace` is fixed by the host"),
        "{text}"
    );
}
