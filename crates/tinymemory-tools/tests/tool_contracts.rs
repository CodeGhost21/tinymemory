//! Freezes the tool names and argument schemas a model sees.
//!
//! `fixtures/tool_contracts.json` is the serialised `specs()` of writable
//! tools over the reference engine (every fetch mode). A schema change is a
//! change to what every host's model is told, so it must be deliberate.
//! To regenerate after an intended change:
//!
//! ```sh
//! BLESS_TOOL_CONTRACTS=1 cargo test -p tinymemory-tools --test tool_contracts
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_tools::MemoryTools;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tool_contracts.json")
}

#[test]
fn specs_match_the_frozen_tool_contracts() {
    let specs = MemoryTools::new(Arc::new(ReferenceEngine::new())).specs();
    let actual = serde_json::to_value(&specs).unwrap();
    if std::env::var_os("BLESS_TOOL_CONTRACTS").is_some() {
        let mut text = serde_json::to_string_pretty(&actual).unwrap();
        text.push('\n');
        std::fs::write(fixture(), text).unwrap();
        return;
    }
    let frozen: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture()).unwrap()).unwrap();
    assert!(
        actual == frozen,
        "the memory tool specs no longer match tests/fixtures/tool_contracts.json.\n\
         If the change is deliberate, regenerate the fixture with\n\
         `BLESS_TOOL_CONTRACTS=1 cargo test -p tinymemory-tools --test tool_contracts`\n\
         and review the diff: every host's model sees these schemas.\n\
         actual:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}
