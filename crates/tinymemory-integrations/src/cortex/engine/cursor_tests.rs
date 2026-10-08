//! Tests for the opaque cursor codec.

use super::*;

#[test]
fn a_list_cursor_round_trips() {
    let state = ListCursor {
        scope: Some("app:tinymemory/agent:a/app:learnings".into()),
        engine: Some("a+b&c".into()),
        offset: 7,
        last: Some("evt_3".into()),
    };
    let encoded = encode('l', &state).unwrap();
    assert!(encoded.starts_with('l'));
    assert_eq!(decode::<ListCursor>('l', &encoded).unwrap(), state);
}

#[test]
fn a_cursor_from_the_other_operation_or_garbage_is_invalid() {
    let fetch = encode(
        'f',
        &FetchCursor {
            offset: 3,
            scopes: None,
        },
    )
    .unwrap();
    for bad in [fetch.as_str(), "lzz", "l123", "", "l"] {
        assert!(
            matches!(
                decode::<ListCursor>('l', bad),
                Err(Error::InvalidRequest(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn a_cursor_at_a_scope_names_its_path() {
    let at = ListCursor::at("app:tinymemory/app:learnings");
    assert_eq!(at.scope.as_deref(), Some("app:tinymemory/app:learnings"));
    assert_eq!((at.engine, at.offset, at.last), (None, 0, None));
}

#[test]
fn a_capped_fetch_cursor_carries_its_scopes() {
    let state = FetchCursor {
        offset: 10,
        scopes: Some(vec!["app:tinymemory/source:gmail/app:documents".into()]),
    };
    let encoded = encode('f', &state).unwrap();
    assert_eq!(decode::<FetchCursor>('f', &encoded).unwrap(), state);
    // An uncapped cursor keeps its old shape, and an old one still reads.
    let old = encode('f', &serde_json::json!({ "offset": 4 })).unwrap();
    assert_eq!(
        decode::<FetchCursor>('f', &old).unwrap(),
        FetchCursor {
            offset: 4,
            scopes: None
        }
    );
}
