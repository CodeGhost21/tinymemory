//! Unit tests for the contract version rule in [`super`].
//!
//! The rule these pin is the one from the kernel design: a **minor** bump means
//! a capability was added and stays compatible; a **major** mismatch refuses
//! the bind.

use super::*;

#[test]
fn contract_version_is_four_two() {
    // (4, 0): the six runtime-tree members and `flavour_profile` were added to
    // `Tree` — a family a driver may ALREADY advertise. The rule makes that a
    // major bump and not a minor one, and the reason is the whole point of the
    // rule: negotiation is family-granular, so a driver advertising `Tree` at
    // (3, 0) would be bound and then asked for a method it has never heard of.
    // The major half is what refuses that bind instead of discovering it at
    // the call.
    //
    // (3, 0) was the same shape one round earlier: `count_chunks`, the three
    // entity-occurrence members and the two tree-forest members, all onto
    // already-advertised families.
    //
    // Note for anyone reading the history: #85/#86/#89/#90 also added methods
    // to advertised families and stayed on the minor half, and so did #122 —
    // the first round of the openhuman engine shed, which put `Summarise`,
    // `RootSummaries`, `ChunkScore`, `DegradedState` and `SourceIngestStatus`
    // onto existing families and shipped as v1.13.5 without touching this
    // constant. All of that was wrong by this rule; those releases and their
    // hosts moved in lockstep so nothing was bound across the gap, but it is
    // drift, not precedent. This round declines to extend it.
    //
    // (4, 1): the five granular operation capabilities are new families, so
    // capability negotiation makes their addition minor-safe.
    //
    // (4, 2): `TreeSummary::preview`, a new optional field. An older kernel
    // skips it when decoding, and a driver that leaves it out sends nothing,
    // so the rule counts it minor-safe.
    assert_eq!(CONTRACT_VERSION, (4, 2));
}

#[test]
fn own_version_is_compatible_with_itself() {
    assert!(is_compatible(CONTRACT_VERSION));
}

#[test]
fn a_minor_bump_stays_compatible_in_both_directions() {
    let (major, minor) = CONTRACT_VERSION;

    // Remote ahead: it advertises families this build does not know. Unknown
    // family strings are skipped during handshake parsing.
    assert!(is_compatible((major, minor + 1)));
    assert!(is_compatible((major, minor + 25)));
    assert!(is_compatible((major, u16::MAX)));

    // Remote behind: it lacks families this build knows. Those simply are not
    // advertised, so the surface degrades — the ordinary path, not an error.
    assert!(is_compatible((major, minor.saturating_sub(1))));
    assert!(is_compatible((major, 0)));
}

#[test]
fn a_major_mismatch_refuses_the_bind() {
    let (major, minor) = CONTRACT_VERSION;

    // Remote ahead by a major: an existing signature changed under us.
    assert!(!is_compatible((major + 1, 0)));
    assert!(!is_compatible((major + 1, minor)));
    assert!(!is_compatible((major + 1, u16::MAX)));

    // Remote behind by a major: same reasoning, other direction. A newer minor
    // does not rescue an older major.
    assert!(!is_compatible((major - 1, u16::MAX)));
    assert!(!is_compatible((0, 0)));
}

#[test]
fn adding_a_method_to_an_already_advertised_family_requires_a_major_bump() {
    // Capability negotiation has family granularity, not method granularity:
    // there is no way to advertise "Core, but without the new method". So a
    // method added to a family a driver may already advertise (e.g. Core,
    // Recall) cannot be made minor-safe by negotiation the way a brand-new
    // capability family can — an older driver still advertising that family
    // would be called into a method it never implemented. This is why the
    // module docs classify that addition as a MAJOR bump, not minor, even
    // though it looks additive. This test exists so the rule cannot be
    // re-derived from `is_compatible`'s code alone, which only encodes "major
    // halves must match" and says nothing about *why* a same-family method
    // addition belongs on the major side of that line.
    assert!(
        !is_compatible((CONTRACT_VERSION.0 + 1, 0)),
        "a method added to an existing family must ship as a major bump, \
         which this asserts refuses the bind against an old build"
    );
}

#[test]
fn compatibility_depends_only_on_the_major_half() {
    let (major, _) = CONTRACT_VERSION;
    for minor in [0u16, 1, 2, 7, 999, u16::MAX] {
        assert!(
            is_compatible((major, minor)),
            "minor {minor} should not affect compatibility"
        );
        assert!(
            !is_compatible((major + 1, minor)),
            "minor {minor} must not rescue a major mismatch"
        );
    }
}
