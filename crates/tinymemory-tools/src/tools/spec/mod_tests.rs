//! Tool specs: names, the read-only subset, closed schemas, and the fetch
//! mode enum following the engine.

use super::*;
use serde_json::Value;

fn names(specs: &[ToolSpec]) -> Vec<&'static str> {
    specs.iter().map(|spec| spec.name).collect()
}

fn find<'a>(specs: &'a [ToolSpec], name: &str) -> &'a ToolSpec {
    specs
        .iter()
        .find(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("no spec named {name}"))
}

/// Every object schema reachable from `schema`, with the path to it.
fn objects<'a>(schema: &'a Value, path: String, out: &mut Vec<(String, &'a Value)>) {
    if schema.get("type") == Some(&Value::from("object")) {
        out.push((path.clone(), schema));
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            objects(property, format!("{path}.{name}"), out);
        }
    }
    if let Some(items) = schema.get("items") {
        objects(items, format!("{path}[]"), out);
    }
}

#[test]
fn with_writes_every_tool_is_listed_in_order() {
    let specs = specs(&FetchMode::ALL, true);
    assert_eq!(names(&specs), TOOL_NAMES);
}

#[test]
fn without_writes_the_write_tools_are_left_out() {
    let specs = specs(&FetchMode::ALL, false);
    let listed = names(&specs);
    assert_eq!(listed.len(), TOOL_NAMES.len() - WRITE_TOOL_NAMES.len());
    assert!(WRITE_TOOL_NAMES.iter().all(|name| !listed.contains(name)));
    assert!(is_write_tool(MEMORY_STORE) && is_write_tool(MEMORY_FORGET));
    assert!(!is_write_tool(MEMORY_RECALL));
}

#[test]
fn every_object_is_closed_and_none_names_a_namespace_or_reach() {
    for spec in specs(&FetchMode::ALL, true) {
        let mut found = Vec::new();
        objects(&spec.parameters, spec.name.to_string(), &mut found);
        assert!(!found.is_empty(), "{} has no object schema", spec.name);
        for (path, object) in found {
            assert_eq!(
                object.get("additionalProperties"),
                Some(&Value::Bool(false)),
                "{path} is not closed"
            );
            let properties = object["properties"].as_object().expect("properties");
            for forbidden in ["namespace", "reach"] {
                assert!(
                    !properties.contains_key(forbidden),
                    "{path} names {forbidden}"
                );
            }
        }
    }
}

#[test]
fn the_fetch_mode_enum_lists_exactly_the_engine_modes() {
    let keyword_only = specs(&[FetchMode::Keyword], true);
    let mode = &find(&keyword_only, MEMORY_FETCH).parameters["properties"]["mode"];
    assert_eq!(mode["enum"], serde_json::json!(["keyword"]));
    assert_eq!(mode["default"], serde_json::json!("keyword"));

    let all = specs(&FetchMode::ALL, true);
    let mode = &find(&all, MEMORY_FETCH).parameters["properties"]["mode"];
    assert_eq!(
        mode["enum"],
        serde_json::json!(["keyword", "vector", "hybrid"])
    );
    assert_eq!(mode["default"], serde_json::json!("hybrid"));
}

#[test]
fn an_engine_without_fetch_modes_gets_no_fetch_tool() {
    assert!(!names(&specs(&[], true)).contains(&MEMORY_FETCH));
}

#[test]
fn the_explore_facets_leave_out_the_namespace() {
    let all = specs(&FetchMode::ALL, true);
    let facets = &find(&all, MEMORY_EXPLORE).parameters["properties"]["facet"]["enum"];
    assert!(
        !facets
            .as_array()
            .expect("enum")
            .contains(&Value::from("namespace"))
    );
}
