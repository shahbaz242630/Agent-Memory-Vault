//! [`SqlCipherKey`] — a zeroize-on-drop wrapper for the SQLCipher passphrase.
//!
//! In production the key fed to SQLCipher is derived from the user's master
//! key (BRD §11.3.1). For T0.1.3 the store accepts the key as an opaque
//! string; key derivation lives in `vault-sync` (T0.2.9). What this type
//! guarantees is that:
//!
//! - The key bytes are wiped from memory on drop ([`zeroize::ZeroizeOnDrop`])
//! - The key can never be accidentally logged: there is no [`Debug`] /
//!   [`std::fmt::Display`] impl
//! - The key is single-use within `vault-storage`: callers pass it once at
//!   `MetadataStore::open` and the store never re-exposes it

use zeroize::{Zeroize, ZeroizeOnDrop};

/// Opaque passphrase for SQLCipher. See module docs.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SqlCipherKey(String);

impl SqlCipherKey {
    /// Construct a key from any string-like input.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// Borrow the key as `&str`. Crate-private — only the storage layer
    /// uses this to issue the `PRAGMA key = '...'` statement.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_constructible_from_string_and_str() {
        let _from_owned = SqlCipherKey::new(String::from("secret"));
        let _from_borrowed = SqlCipherKey::new("secret");
    }

    #[test]
    fn key_is_clonable() {
        // Cloning is intentional — `Connection` open paths may need to
        // re-issue the key on schema changes. The clone is also wiped on drop.
        let k = SqlCipherKey::new("secret");
        let _clone = k.clone();
    }
}
