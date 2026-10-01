use super::*;
use serde_json::json;

use crate::pii::{redact_pii, PII_CC};
// `pii`'s internals (checksum validators, the normalization pass) are test-only
// re-exports at the `pii` module level; pull them in here so the nested test
// submodules below can reach them through their own `use super::*;`.
use crate::pii::{
    digits, scan_candidates, valid_cnpj, valid_cpf, valid_cuit, valid_dni_es, valid_iban,
    valid_luhn, valid_nie_es, valid_nino, valid_ssn, valid_verhoeff, NormalizedView,
};
use crate::{MAX_JSON_SANITIZE_DEPTH, REDACTED_PRIVATE_KEY, REDACTED_SECRET};

/// Assembled rather than written out so a repository secret scanner does
/// not read the fixture as a real key block.
fn private_key_fixture(kind: &str, body: &str) -> String {
    format!("-----BEGIN {kind}-----\n{body}\n-----END {kind}-----")
}

fn redacts(input: &str, token: &str) {
    let out = redact_pii(input);
    assert!(
        out.value.contains(token),
        "expected {token} in output. input={input:?} output={out:?}"
    );
}

fn unchanged(input: &str) {
    let out = redact_pii(input);
    assert_eq!(
        out.value, input,
        "expected no change; report={:?}",
        out.report
    );
    assert_eq!(out.report.pii_redactions, 0);
}

#[path = "default_policy_prefilter_tests.rs"]
mod default_policy_prefilter_tests;
#[path = "default_policy_sanitize_tests.rs"]
mod default_policy_sanitize_tests;

/// The one place the two historical copies differed: a bare Luhn-valid run that
/// is neither a real network IIN nor near a card keyword (here a 13-digit
/// epoch-millisecond timestamp). The default policy is the strictest and
/// redacts it; the TinyCortex policy leaves it alone.
#[test]
fn bare_card_gate_is_the_only_policy_difference() {
    let ts = "1700000000004";
    let json = format!("{{\"ts\": {ts}}}");

    let strict = redact_pii(&json);
    assert!(
        strict.value.contains(PII_CC),
        "default policy must redact: {strict:?}"
    );
    assert_eq!(
        crate::pii::redact_pii_with(&json, Policy::default()).value,
        strict.value
    );
    assert_eq!(Policy::default().bare_card, BareCardGate::LuhnOnly);

    let corroborated = crate::pii::redact_pii_with(&json, Policy::corroborated());
    assert_eq!(
        corroborated.value, json,
        "corroborated policy keeps timestamps"
    );

    // Real card, bare, real IIN: both policies redact.
    let visa = "4111111111111111";
    assert!(redact_pii(visa).value.contains(PII_CC));
    assert!(crate::pii::redact_pii_with(visa, Policy::corroborated())
        .value
        .contains(PII_CC));

    // The JSON and text entry points thread the policy through.
    let value = json!({ "ts": ts });
    assert_ne!(
        sanitize_json(&value).value,
        sanitize_json_with(&value, Policy::corroborated()).value
    );
    assert_ne!(
        sanitize_text(ts).value,
        sanitize_text_with(ts, Policy::corroborated()).value
    );
}
