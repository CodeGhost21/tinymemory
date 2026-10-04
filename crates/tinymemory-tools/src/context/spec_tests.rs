//! Spec defaults and validation.

use super::*;

#[test]
fn the_default_spec_has_the_four_briefs_in_order() {
    let spec = ContextSpec::default();
    let headings: Vec<&str> = spec.briefs.iter().map(|b| b.heading.as_str()).collect();
    assert_eq!(
        headings,
        [
            "About the user",
            "Active work",
            "Preferences and standing instructions",
            "Recent important events"
        ]
    );
    assert!(spec.validate().is_ok());
}

#[test]
fn a_zero_budget_or_blank_brief_is_invalid() {
    let zero = ContextSpec {
        budget_tokens: 0,
        ..ContextSpec::default()
    };
    assert!(matches!(zero.validate(), Err(Error::InvalidSpec(_))));
    let blank = ContextSpec {
        briefs: vec![Brief::new("Heading", "  ")],
        ..ContextSpec::default()
    };
    assert!(matches!(blank.validate(), Err(Error::InvalidSpec(_))));
}
