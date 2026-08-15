//! Security contract for the public whole-artifact sealing API
//! ([`vault_storage::seal_vault_blob`] / [`vault_storage::unseal_vault_blob`]),
//! added at ADR-SEC-007.
//!
//! # Why this file exists
//!
//! ADR-SEC-007 found the consolidator REPORT
//! (`<vault_root>/reports/<boundary>.report.json`) being written as
//! **plaintext JSON containing verbatim memory text**, in violation of
//! BRD §11.5.1 ("All data on disk is encrypted. No exceptions."). The fix
//! routes that artifact through the same at-rest envelope the vector store
//! and graph snapshot already use.
//!
//! These are the tests that make the fix meaningful rather than decorative.
//! Per BRD §11.13 they are adversarial: wrong key, tampered bytes, and the
//! cross-boundary swap that the AAD binding exists to defeat.
//!
//! **The non-vacuity test matters most.** A sealing wrapper that silently
//! did nothing would pass every round-trip test ever written — encrypt with
//! the identity function and `seal(x)` still unseals to `x`. So
//! `sealed_bytes_do_not_contain_the_plaintext` asserts the property the
//! feature actually exists for: the secret is not readable on disk. Same
//! discipline as ADR-SEC-006's comment-stripping guard, which passed on
//! prose until it was made to run against the real markup.

use vault_storage::{seal_vault_blob, unseal_vault_blob};

/// Stand-in for real REPORT content: the fact text is the thing that must
/// never appear on disk.
const SECRET_FACT: &str = "The user's mother's maiden name is Kowalczyk.";

fn report_json() -> Vec<u8> {
    format!(
        r#"{{"schema_version":1,"boundary":"personal","facts_by_topic":{{"family":[{{"fact":"{SECRET_FACT}"}}]}}}}"#
    )
    .into_bytes()
}

fn key_a() -> [u8; 32] {
    [7u8; 32]
}

fn key_b() -> [u8; 32] {
    [9u8; 32]
}

const PERSONAL_PATH: &str = "reports/personal.report.sealed";
const WORK_PATH: &str = "reports/work.report.sealed";

#[test]
fn round_trips_through_the_at_rest_envelope() {
    let plaintext = report_json();
    let sealed = seal_vault_blob(&plaintext, &key_a(), PERSONAL_PATH);
    let recovered = unseal_vault_blob(&sealed, &key_a(), PERSONAL_PATH)
        .expect("correct key + path must unseal");
    assert_eq!(
        recovered, plaintext,
        "seal->unseal MUST be lossless for the REPORT write/read path"
    );
}

/// The test that proves the feature does its job. Without this, a no-op
/// "sealing" wrapper passes the whole suite.
#[test]
fn sealed_bytes_do_not_contain_the_plaintext() {
    let plaintext = report_json();
    let sealed = seal_vault_blob(&plaintext, &key_a(), PERSONAL_PATH);

    assert_ne!(
        sealed, plaintext,
        "sealed output MUST differ from plaintext (non-vacuity)"
    );

    // The exact bug ADR-SEC-007 fixes: reading the artifact off disk with
    // no key and finding a memory in it.
    let haystack = String::from_utf8_lossy(&sealed);
    assert!(
        !haystack.contains(SECRET_FACT),
        "SECURITY: the sealed blob leaks the fact text verbatim. This is the \
         ADR-SEC-007 bug reappearing (BRD §11.5.1: all data on disk encrypted, \
         no exceptions)."
    );
    assert!(
        !haystack.contains("facts_by_topic"),
        "SECURITY: the sealed blob leaks REPORT structure, so it was not \
         actually encrypted."
    );
}

#[test]
fn wrong_key_fails_closed() {
    let sealed = seal_vault_blob(&report_json(), &key_a(), PERSONAL_PATH);
    let result = unseal_vault_blob(&sealed, &key_b(), PERSONAL_PATH);
    assert!(
        result.is_err(),
        "SP-4 fail-securely: a wrong at-rest key MUST NOT yield plaintext"
    );
}

#[test]
fn tampered_ciphertext_fails_closed() {
    let mut sealed = seal_vault_blob(&report_json(), &key_a(), PERSONAL_PATH);
    // Flip a bit deep in the ciphertext, past the 26-byte framing prefix.
    let last = sealed.len() - 1;
    sealed[last] ^= 0b0000_0001;

    let result = unseal_vault_blob(&sealed, &key_a(), PERSONAL_PATH);
    assert!(
        result.is_err(),
        "AEAD authentication MUST reject tampered bytes rather than \
         returning attacker-influenced plaintext"
    );
}

#[test]
fn truncated_envelope_fails_closed() {
    let sealed = seal_vault_blob(&report_json(), &key_a(), PERSONAL_PATH);
    let truncated = &sealed[..10];
    assert!(
        unseal_vault_blob(truncated, &key_a(), PERSONAL_PATH).is_err(),
        "a truncated envelope MUST be rejected, not partially decoded"
    );
}

/// BRD §11.3.2: "AAD includes memory ID and boundary — binds ciphertext to
/// context, prevents swap attacks."
///
/// The attack: copy `work.report.sealed` over `personal.report.sealed`. The
/// key is identical (same vault), so only the AAD stands between the
/// attacker and one boundary's facts being served as another's — which
/// would defeat the mandatory access control in BRD §11.4.3.
#[test]
fn cross_boundary_swap_is_rejected() {
    let sealed_work = seal_vault_blob(&report_json(), &key_a(), WORK_PATH);

    let result = unseal_vault_blob(&sealed_work, &key_a(), PERSONAL_PATH);
    assert!(
        result.is_err(),
        "SECURITY: a REPORT sealed for the `work` boundary unsealed under \
         `personal`. Boundary isolation (BRD §11.4.3) is defeated by a file \
         rename. The AAD must bind the boundary."
    );

    // Control: the same bytes at their own path still work, proving the
    // rejection above is the AAD binding and not a broken seal.
    assert!(
        unseal_vault_blob(&sealed_work, &key_a(), WORK_PATH).is_ok(),
        "control: the work REPORT MUST still unseal at its own path"
    );
}

/// Distinct boundaries must not collide even with identical content — the
/// AAD differs, so the envelopes differ.
#[test]
fn same_content_different_boundary_produces_different_envelopes() {
    let plaintext = report_json();
    let a = seal_vault_blob(&plaintext, &key_a(), PERSONAL_PATH);
    let b = seal_vault_blob(&plaintext, &key_a(), WORK_PATH);
    assert_ne!(
        a, b,
        "identical content at different boundaries MUST NOT produce identical \
         envelopes"
    );
}

#[test]
fn empty_plaintext_round_trips() {
    // A boundary with no facts yet still writes a REPORT skeleton; the
    // degenerate case must not panic or produce an unreadable envelope.
    let sealed = seal_vault_blob(&[], &key_a(), PERSONAL_PATH);
    let recovered =
        unseal_vault_blob(&sealed, &key_a(), PERSONAL_PATH).expect("empty payload must round-trip");
    assert!(recovered.is_empty());
}
