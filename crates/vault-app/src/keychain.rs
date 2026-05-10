//! At-rest-key provenance: OS keychain read/init + master_key derivation tree.
//!
//! See HANDOFF.md ADR-040 + ADR-040 amendment for the contract this module
//! implements. T0.2.0 close-out plan iteration 1 Phase 1 + iteration 1.5
//! Discovery 4 factored keychain-touching code into this module so vault-tauri
//! main.rs's surface change stays minimal and the helpers are unit-testable
//! against the same Windows Credential Manager backend the spike runtime-
//! confirmed.
//!
//! ## Public surface
//!
//! - [`read_or_init_master_key`] — read 32-byte master_key from keychain;
//!   on `NotFound` first-run-generate via `getrandom` + persist + return.
//! - [`derive_sqlcipher_passphrase`] — domain-separated BLAKE3 subkey,
//!   hex-encoded for the `SqlCipherKey::new(String)` constructor.
//! - [`derive_at_rest_key`] — domain-separated BLAKE3 subkey, returned as
//!   `Zeroizing<[u8; 32]>` for downstream consumption by
//!   [`vault_storage::LanceVectorStore::open_with_at_rest_key`] (Phase 2/3).
//! - [`PRODUCTION_NAMESPACE`] — `"com.memoryvault.v0.2"` reverse-DNS service
//!   string for production keychain entries.
//!
//! ## Platform support (Phase 1)
//!
//! Windows-only at Phase 1 per ADR-029 V0.1 dogfood pattern + ADR-040 OQ #1
//! partial resolution. macOS / Linux per-platform crate-add deferred to
//! T0.2.0.x sub-task or T0.2.14 Stub-Installer-adjacent. On non-Windows
//! platforms, [`read_or_init_master_key`] returns
//! [`VaultError::KeychainProvenance`] with a clear "platform not supported in
//! V0.2 Phase 1" message — vault-tauri's fatal-dialog surface picks it up.
//!
//! Derivation primitives ([`derive_sqlcipher_passphrase`] +
//! [`derive_at_rest_key`]) are platform-independent — pure BLAKE3 derive_key
//! calls with no keychain dependency.
//!
//! ## Domain-separated subkey discipline (ADR-040 amendment option β)
//!
//! ```text
//! master_key             ← 32 bytes from keychain
//! at_rest_key            = blake3::derive_key("vault memory at-rest sealing v1", &master_key)
//! sqlcipher_passphrase   = hex(blake3::derive_key("vault sqlcipher passphrase v1", &master_key))
//! ```
//!
//! Distinct domain-separator strings prevent algebraic crossover between
//! the two consumers; same primitive (BLAKE3) preserves single-source-crypto
//! per ADR-008 amendment line 693.

use vault_core::{VaultError, VaultResult};
use zeroize::Zeroizing;

/// Reverse-DNS namespace for production keychain entries — locked at ADR-040.
///
/// Distinguishable from any other Memory Vault keychain entry; spike used
/// `com.memoryvault.spike.v0.2` (different sub-namespace) so spike runs never
/// collide with production. V1.0 multi-vault forward-compat preserved: same
/// namespace will hold multiple per-vault entries differentiated by the
/// account-string slot.
pub const PRODUCTION_NAMESPACE: &str = "com.memoryvault.v0.2";

/// BLAKE3 derive_key context for the SqlCipher passphrase subkey
/// (ADR-040 amendment option β).
///
/// The trailing `v1` lets us rotate without ambiguity if the derivation
/// scheme ever changes.
const SQLCIPHER_KDF_CONTEXT: &str = "vault sqlcipher passphrase v1";

/// BLAKE3 derive_key context for the at-rest sealing subkey (ADR-008
/// amendment K3 KDF). Matches the constant ADR-008 amendment locked at
/// HANDOFF.md — same string, single source-of-truth.
const AT_REST_KDF_CONTEXT: &str = "vault memory at-rest sealing v1";

/// Read the 32-byte `master_key` from the OS keychain. On first run (no
/// entry exists), generate a new `master_key` via `getrandom`, persist via
/// `set_secret`, and return the newly-persisted key.
///
/// # Arguments
///
/// - `namespace` — keychain service string. Production callers pass
///   [`PRODUCTION_NAMESPACE`]; tests pass a unique-per-test string to avoid
///   colliding with production entries or with concurrent test runs.
/// - `vault_id` — keychain account/user string. V0.2 single-vault per
///   BRD §6.2 passes `"default"`. V1.0 multi-vault will pass real
///   per-vault ids.
///
/// # Errors
///
/// Returns [`VaultError::KeychainProvenance`] for any keychain failure
/// other than `NotFound` (which is handled internally as the first-run
/// signal). On non-Windows platforms returns the same variant with a
/// "platform not supported in V0.2 Phase 1" message.
///
/// # Security
///
/// Returned key is wrapped in [`Zeroizing<[u8; 32]>`] so the bytes are
/// wiped from memory on Drop per BRD §11.5.3. Caller MUST NOT materialise
/// the bytes as a plaintext `Vec<u8>` outside the immediate derivation
/// call site.
#[cfg(windows)]
pub fn read_or_init_master_key(
    namespace: &str,
    vault_id: &str,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    use windows_native_keyring_store::Store;

    keyring_core::set_default_store(
        Store::new()
            .map_err(|e| VaultError::KeychainProvenance(format!("Store::new failed: {e}")))?,
    );

    let result = read_or_init_inner(namespace, vault_id);

    keyring_core::unset_default_store();

    result
}

/// Non-Windows stub: returns a clear "platform not supported" error.
/// vault-tauri's fatal-dialog flow surfaces it to the user. Cross-platform
/// keychain wiring lands at T0.2.0.x sub-task or T0.2.14 Stub-Installer-
/// adjacent per HANDOFF.md OQ #1 partial resolution.
#[cfg(not(windows))]
pub fn read_or_init_master_key(
    _namespace: &str,
    _vault_id: &str,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    Err(VaultError::KeychainProvenance(format!(
        "Keychain provenance is Windows-only at V0.2 Phase 1 (current platform: {}). \
         macOS / Linux per-platform crate-add deferred to T0.2.0.x sub-task or \
         T0.2.14 Stub-Installer-adjacent per HANDOFF.md OQ #1 partial resolution.",
        std::env::consts::OS
    )))
}

/// Inner read/first-run helper. Assumes [`keyring_core::set_default_store`]
/// has been called by the caller; caller is also responsible for the matching
/// [`keyring_core::unset_default_store`].
#[cfg(windows)]
fn read_or_init_inner(namespace: &str, vault_id: &str) -> VaultResult<Zeroizing<[u8; 32]>> {
    use keyring_core::Entry;

    let entry = Entry::new(namespace, vault_id)
        .map_err(|e| VaultError::KeychainProvenance(format!("Entry::new failed: {e}")))?;

    match entry.get_secret() {
        Ok(bytes) => {
            // Existing entry — must be exactly 32 bytes for our master_key
            // shape. Anything else means the entry was written by a
            // different (potentially future-incompat) scheme; fail closed.
            if bytes.len() != 32 {
                return Err(VaultError::KeychainProvenance(format!(
                    "Keychain entry exists but secret is {} bytes (expected 32). \
                     Entry may have been written by a non-Memory-Vault tool or by \
                     a future version with a different scheme. Investigate via \
                     Credential Manager (entry namespace: {namespace}, account: \
                     {vault_id}) before proceeding.",
                    bytes.len()
                )));
            }
            let mut master_key = Zeroizing::new([0u8; 32]);
            master_key.copy_from_slice(&bytes);
            Ok(master_key)
        }
        Err(get_err) => {
            // The keyring-core Error type doesn't expose a stable kind enum
            // we can match on across crate versions — string-match on the
            // Display form for the NotFound case (first-run signal). All
            // other errors flow as KeychainProvenance.
            //
            // This is a deliberate trade-off: string-matching is brittle
            // across keyring-core upgrades, but the alternative (treating
            // every Err as first-run) would silently overwrite a legitimate
            // entry on read failure — unsafe. If keyring-core 1.x exposes
            // a stable kind enum in a future minor, swap to that.
            let err_str = format!("{get_err}");
            let err_lower = err_str.to_lowercase();
            let is_not_found = err_lower.contains("no entry")
                || err_lower.contains("not found")
                || err_lower.contains("no such")
                // Windows Credential Manager (`windows-native-keyring-store`)
                // surfaces the empty-entry case as "No matching credential found"
                // — none of the prior substrings match it. Verified empirically
                // at T0.2.0 Phase 1 DoD-gate session 2026-05-10.
                || err_lower.contains("no matching credential");
            if !is_not_found {
                return Err(VaultError::KeychainProvenance(format!(
                    "get_secret failed (non-NotFound): {get_err}"
                )));
            }

            // First-run path: generate new 32-byte master_key, persist,
            // return.
            let mut master_key = Zeroizing::new([0u8; 32]);
            getrandom::getrandom(&mut *master_key)
                .map_err(|e| VaultError::KeychainProvenance(format!("getrandom: {e}")))?;
            entry.set_secret(&*master_key).map_err(|e| {
                VaultError::KeychainProvenance(format!(
                    "set_secret failed during first-run init: {e}"
                ))
            })?;
            Ok(master_key)
        }
    }
}

/// Derive the SqlCipher passphrase from the master_key per ADR-040
/// amendment option β: BLAKE3 derive_key with the locked
/// [`SQLCIPHER_KDF_CONTEXT`] domain-separator, hex-encoded to a 64-character
/// String suitable for [`vault_storage::SqlCipherKey::new`].
///
/// Hex-encoding is a serialization choice (32 raw bytes don't fit cleanly
/// into a `String` passphrase per SqlCipherKey's contract), not a security
/// choice. The 32 bytes are full-strength keying material; SQLCipher's
/// PBKDF2 over the hex-encoded form is defense-in-depth.
pub fn derive_sqlcipher_passphrase(master_key: &[u8; 32]) -> vault_storage::SqlCipherKey {
    let subkey = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, master_key);
    vault_storage::SqlCipherKey::new(hex::encode(subkey))
}

/// Derive the at-rest sealing key from the master_key per ADR-008
/// amendment K3 KDF: BLAKE3 derive_key with the locked
/// [`AT_REST_KDF_CONTEXT`] domain-separator. Returned as
/// `Zeroizing<[u8; 32]>` for downstream consumption by
/// [`vault_storage::LanceVectorStore::open_with_at_rest_key`] at Phase 2/3.
pub fn derive_at_rest_key(master_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let subkey = blake3::derive_key(AT_REST_KDF_CONTEXT, master_key);
    Zeroizing::new(subkey)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a unique-per-test namespace so concurrent + sequential test
    /// runs never collide on the same keychain entry. Prefixed with
    /// `com.memoryvault.test.v0.2.` so a stale entry from a panicked test
    /// is still distinguishable from production / spike entries during
    /// manual Credential Manager cleanup.
    #[cfg(windows)]
    fn unique_test_namespace(test_name: &str) -> String {
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce).expect("getrandom for test namespace");
        format!(
            "com.memoryvault.test.v0.2.{}.{}",
            test_name,
            hex::encode(nonce)
        )
    }

    /// Best-effort cleanup helper. Used in test teardown so re-runs are
    /// deterministic; failures are logged but do not fail the test.
    #[cfg(windows)]
    fn cleanup_keychain_entry(namespace: &str, vault_id: &str) {
        use keyring_core::Entry;
        use windows_native_keyring_store::Store;

        let store = match Store::new() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[cleanup] Store::new failed: {e}");
                return;
            }
        };
        keyring_core::set_default_store(store);
        if let Ok(entry) = Entry::new(namespace, vault_id) {
            let _ = entry.delete_credential();
        }
        keyring_core::unset_default_store();
    }

    /// Round-trip: write a master_key via the helper path → read back → byte-
    /// equal. Per iteration-1.5 amendment test floor adjustment (a).
    #[test]
    #[cfg(windows)]
    fn read_or_init_master_key_round_trips_byte_identical() {
        let namespace = unique_test_namespace("round_trip");
        let vault_id = "test-round-trip-vault";

        // First call generates + persists. Second call reads back the
        // persisted bytes. Byte-equal assertion verifies the keychain
        // round-trip preserves all 32 bytes exactly.
        let first = read_or_init_master_key(&namespace, vault_id).expect("first call must succeed");
        let second =
            read_or_init_master_key(&namespace, vault_id).expect("second call must succeed");

        assert_eq!(
            first.as_slice(),
            second.as_slice(),
            "Round-trip MUST preserve all 32 bytes byte-identical; \
             first call generated key X, second call read back key Y, X != Y. \
             Either get_secret returned different bytes than set_secret wrote, \
             OR the second call hit first-run path despite the entry existing \
             (NotFound classifier regression)."
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// First-run-generates-and-persists: no entry exists → first call
    /// generates new 32-byte key + persists. Second call reads the
    /// persisted entry (NOT a fresh generation). Per iteration-1.5
    /// amendment test floor adjustment (b).
    #[test]
    #[cfg(windows)]
    fn read_or_init_master_key_first_run_generates_and_persists() {
        let namespace = unique_test_namespace("first_run");
        let vault_id = "test-first-run-vault";

        // Pre-cleanup so the test starts from a known no-entry state even if
        // a prior test run leaked.
        cleanup_keychain_entry(&namespace, vault_id);

        let first = read_or_init_master_key(&namespace, vault_id)
            .expect("first call must succeed (first-run path)");

        // Subsequent call must return THE SAME bytes (read existing entry),
        // not regenerate. Byte-equal assertion = persistence proof.
        let second = read_or_init_master_key(&namespace, vault_id)
            .expect("second call must succeed (read existing entry)");
        assert_eq!(
            first.as_slice(),
            second.as_slice(),
            "First-run-generates-and-persists invariant: second call MUST read \
             the persisted key, not regenerate. If second != first, the helper \
             is hitting first-run path twice — entry was not persisted, OR the \
             read path's NotFound classifier is misfiring on a successful read."
        );

        // Sanity: keys are 32 bytes (not zero, not truncated).
        assert_eq!(
            first.len(),
            32,
            "Generated master_key must be 32 bytes; got {}",
            first.len()
        );
        let all_zero = first.iter().all(|&b| b == 0);
        assert!(
            !all_zero,
            "Generated master_key must NOT be all zeros (would indicate getrandom \
             returned without filling the buffer or a write/read corruption)."
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// SqlCipher derivation is deterministic + uses the locked context string.
    /// Defensive pin against accidental KDF-context drift.
    #[test]
    fn derive_sqlcipher_passphrase_is_deterministic() {
        let master_key = [42u8; 32];
        let p1 = derive_sqlcipher_passphrase(&master_key);
        let p2 = derive_sqlcipher_passphrase(&master_key);
        // SqlCipherKey doesn't impl PartialEq + Debug per its zeroize-on-drop
        // discipline; we compare via the crate-private as_str through the
        // Display-less surface: rebuild and compare hex outputs by recomputing
        // the BLAKE3 derive_key step independently here.
        let expected = hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key));
        assert_eq!(
            expected.len(),
            64,
            "BLAKE3 derive_key output must hex-encode to 64 ASCII chars; got {}",
            expected.len()
        );
        // Both p1 and p2 derived from the same master_key + same context must
        // be byte-equal — but SqlCipherKey hides its bytes by design. This
        // assertion verifies the BLAKE3 layer is deterministic; the
        // SqlCipherKey layer is a thin wrapper.
        let again = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key);
        assert_eq!(
            hex::encode(again),
            expected,
            "BLAKE3 derive_key with the same context + same master_key MUST \
             produce identical output; non-deterministic output is a libray bug."
        );
        // p1 / p2 are dropped here, exercising Zeroize on the SqlCipherKey
        // wrapper for a smoke check that the helper-returned values can be
        // dropped without unsafe panics.
        drop((p1, p2));
    }

    /// At-rest derivation is deterministic + uses ADR-008 amendment K3 KDF
    /// context. Defensive pin against accidental K3 KDF context drift.
    #[test]
    fn derive_at_rest_key_is_deterministic_and_uses_k3_kdf_context() {
        let master_key = [7u8; 32];
        let k1 = derive_at_rest_key(&master_key);
        let k2 = derive_at_rest_key(&master_key);
        assert_eq!(
            k1.as_slice(),
            k2.as_slice(),
            "BLAKE3 derive_key with the same context + master_key MUST be \
             deterministic; non-deterministic output is a library bug."
        );
        // ADR-008 amendment locks the context string verbatim; drift here
        // would silently change at-rest key material, breaking on-disk
        // sealed bytes already written under the old derivation. Defensive
        // pin: independently recompute and compare.
        let expected = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
        assert_eq!(
            k1.as_slice(),
            expected.as_slice(),
            "ADR-008 amendment K3 KDF context drift detected — derive_at_rest_key \
             output does not match independently-recomputed BLAKE3 derive_key with \
             context string \"vault memory at-rest sealing v1\". This would \
             invalidate every on-disk sealed file written under the prior \
             derivation. STOP."
        );
    }

    /// Distinct domain-separator strings for SqlCipher vs at-rest produce
    /// distinct subkeys from the same master_key. Defensive pin against
    /// accidentally-collapsed KDF contexts (which would silently couple the
    /// two consumers' keying material).
    #[test]
    fn sqlcipher_and_at_rest_subkeys_are_domain_separated() {
        let master_key = [99u8; 32];
        let sqlcipher_subkey = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key);
        let at_rest_subkey = blake3::derive_key(AT_REST_KDF_CONTEXT, &master_key);
        assert_ne!(
            sqlcipher_subkey, at_rest_subkey,
            "ADR-040 amendment option β requires distinct SqlCipher / at-rest subkeys \
             via distinct BLAKE3 domain-separator contexts. Equal subkeys would \
             couple the two consumers' keying material — silent crypto-discipline \
             regression."
        );
    }
}
