//! Event type mapping tests.

use super::*;

#[test]
fn event_types_map_onto_learning_kinds() {
    assert_eq!(kind("fact"), LearningKind::Fact);
    assert_eq!(kind(" Decision "), LearningKind::Fact);
    assert_eq!(kind("preference"), LearningKind::Preference);
    for other in ["commitment", "question", "foresight", "", "mystery"] {
        assert_eq!(kind(other), LearningKind::Other, "{other}");
    }
}
