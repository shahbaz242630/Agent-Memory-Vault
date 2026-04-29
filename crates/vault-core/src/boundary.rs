//! Boundary — the unit of mandatory access control.
//!
//! Every memory belongs to exactly one boundary at write time. Retrieval
//! requests declare which boundaries they may access. Boundary enforcement
//! happens at the storage layer (BRD §11.4.3) so buggy retrieval logic
//! cannot leak cross-boundary content.
//!
//! Typical boundary names: `"work"`, `"personal"`, `"health"`, `"default"`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{VaultError, VaultResult};

/// Maximum length of a boundary name in bytes (BRD §11.7.1).
pub const MAX_BOUNDARY_LEN: usize = 64;

/// A boundary scope. Validated at construction.
///
/// The inner string is private to enforce length and charset invariants;
/// use [`Boundary::new`] / [`Boundary::try_from`] to construct, and
/// [`Boundary::as_str`] to read.
///
/// This deviates from BRD §5.1's literal `pub struct Boundary(pub String)`
/// in favour of the BRD §11.7.1 invariant "boundary names ≤ 64 chars,
/// no control characters." The validated constructor pattern enforces it
/// at the type boundary so storage and retrieval never see an invalid value.
///
/// # Example
///
/// ```
/// use vault_core::Boundary;
/// let b = Boundary::new("work").unwrap();
/// assert_eq!(b.as_str(), "work");
/// assert!(Boundary::new("").is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Boundary(String);

impl Boundary {
    /// Construct a new boundary, validating against the rules in BRD §11.7.1.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::InvalidInput`] if the name is empty, exceeds
    /// [`MAX_BOUNDARY_LEN`] bytes, or contains control characters.
    pub fn new(name: impl Into<String>) -> VaultResult<Self> {
        let name = name.into();
        Self::validate(&name)?;
        Ok(Self(name))
    }

    /// Borrow the boundary name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the boundary, returning the underlying string.
    pub fn into_inner(self) -> String {
        self.0
    }

    /// The conventional default boundary name. Used when an MCP client does
    /// not declare a scope (BRD §5.7).
    pub fn default_name() -> Self {
        Self("default".to_string())
    }

    fn validate(name: &str) -> VaultResult<()> {
        if name.is_empty() {
            return Err(VaultError::InvalidInput(
                "boundary name must not be empty".into(),
            ));
        }
        if name.len() > MAX_BOUNDARY_LEN {
            return Err(VaultError::InvalidInput(format!(
                "boundary name exceeds {MAX_BOUNDARY_LEN} bytes",
            )));
        }
        // Boundary names are identifier-like: ASCII letters, digits, dash, and
        // underscore only. This is stricter than BRD §11.7.1's "no control
        // characters" floor, and is required because boundary names are
        // interpolated into the LanceDB `only_if` SQL filter at the query layer
        // (T0.1.4) — LanceDB 0.8 has no parameter binding for `only_if`, so
        // the type system is the only line of defence against quote breakout
        // and SQL-metacharacter injection. Tightening here gives the same
        // safety to every future store and filter context for free.
        // ADR-005 amended 2026-04-29 to record this addition.
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(VaultError::InvalidInput(
                "boundary name must contain only ASCII letters, digits, '-', or '_'".into(),
            ));
        }
        Ok(())
    }
}

impl fmt::Display for Boundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Boundary {
    type Err = VaultError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_string())
    }
}

impl TryFrom<String> for Boundary {
    type Error = VaultError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for Boundary {
    type Error = VaultError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value.to_string())
    }
}

impl From<Boundary> for String {
    fn from(value: Boundary) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn empty_name_rejected() {
        assert!(matches!(
            Boundary::new(""),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn overlong_name_rejected() {
        let too_long = "x".repeat(MAX_BOUNDARY_LEN + 1);
        assert!(matches!(
            Boundary::new(too_long),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn boundary_at_max_length_accepted() {
        let exact = "x".repeat(MAX_BOUNDARY_LEN);
        assert!(Boundary::new(exact).is_ok());
    }

    #[test]
    fn control_characters_rejected() {
        assert!(matches!(
            Boundary::new("work\nspace"),
            Err(VaultError::InvalidInput(_))
        ));
        assert!(matches!(
            Boundary::new("with\0null"),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn sql_metacharacters_rejected() {
        // Boundary names flow into LanceDB's `only_if` SQL filter without
        // parameter binding (T0.1.4 / ADR-010). These cases would otherwise
        // be a quote-breakout into the filter string. NB: `--` and `-` alone
        // are intentionally allowed as identifier characters; they are only
        // dangerous in unquoted SQL, and our filter always single-quotes the
        // boundary value.
        for name in [
            "work'stuff",
            "work\"stuff",
            "work; DROP",
            "work OR 1=1",
            "work/*",
            "work\\stuff",
            "work space",
            "work.stuff",
        ] {
            match Boundary::new(name) {
                Err(VaultError::InvalidInput(_)) => {}
                other => panic!(
                    "expected InvalidInput for boundary {name:?}, got {}",
                    match other {
                        Ok(_) => "Ok(...)",
                        Err(_) => "different error variant",
                    }
                ),
            }
        }
    }

    #[test]
    fn safe_charset_accepted() {
        // Identifier-like names are explicitly permitted.
        for name in [
            "work",
            "personal",
            "health",
            "work-stuff",
            "work_2026",
            "Work-2",
        ] {
            assert!(Boundary::new(name).is_ok(), "expected accept for {name}");
        }
    }

    #[test]
    fn fromstr_and_tryfrom_agree() {
        let from_str = Boundary::from_str("work").unwrap();
        let try_from_owned: Boundary = "work".to_string().try_into().unwrap();
        let try_from_borrowed: Boundary = "work".try_into().unwrap();
        assert_eq!(from_str, try_from_owned);
        assert_eq!(from_str, try_from_borrowed);
    }

    #[test]
    fn default_name_is_valid() {
        let b = Boundary::default_name();
        assert_eq!(b.as_str(), "default");
        // The default name must round-trip through validation.
        let reparsed = Boundary::new(b.as_str()).unwrap();
        assert_eq!(reparsed, b);
    }

    #[test]
    fn serde_roundtrip_simple() {
        let b = Boundary::new("personal").unwrap();
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "\"personal\"");
        let back: Boundary = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn serde_rejects_invalid_input() {
        // An empty string in JSON must not deserialize into a Boundary.
        let result: Result<Boundary, _> = serde_json::from_str("\"\"");
        assert!(result.is_err());
    }

    proptest! {
        #[test]
        fn valid_boundary_names_roundtrip_through_serde(
            name in "[a-zA-Z0-9_-]{1,64}"
        ) {
            let b = Boundary::new(name.clone()).unwrap();
            let json = serde_json::to_string(&b).unwrap();
            let back: Boundary = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(b.as_str(), back.as_str());
            prop_assert_eq!(name, back.as_str().to_string());
        }
    }
}
