//! Per-agent capability token store (ADR-SEC-001, local multi-agent daemon).
//!
//! The shared daemon replaces stdio's implicit OS-process trust: a loopback
//! socket is reachable by any local process, so every agent must present a
//! bearer capability token (BRD §11.4.4 step 1, §11.12 vault-mcp "capability
//! tokens for every connection"). A token resolves to ONE agent record holding
//! that agent's authorized boundaries; the daemon uses those as the per-request
//! authorized-boundary slice (ADR-SEC-001 D4).
//!
//! **SP-1 / zero-knowledge:** the plaintext token is NEVER stored. Only its
//! BLAKE3 hash is persisted ([`hash_capability_token`]); the same function runs
//! at mint time (`vault-cli agent add`) and at auth time (the daemon), so a
//! presented token maps deterministically to the stored hash. Revocation is
//! soft (`revoked_at`) — the auth lookup filters `revoked_at IS NULL`, while
//! `list_agent_tokens` keeps the full history for `agent list` + audit.
//!
//! Storage: the `agent_tokens` table (migration 0007) in the already
//! SQLCipher-encrypted `vault.db` (ADR-SEC-001 D9 — no new crypto path).

use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use tracing::instrument;

use vault_core::{Boundary, VaultError, VaultResult};

use crate::StorageBackend;

/// A registered agent's capability record. Read model only — it never carries
/// the secret token (only the hash is stored, and that is not surfaced here).
#[derive(Clone, Debug)]
pub struct AgentToken {
    /// Stable agent identity (lowercase kebab-case, e.g. `claude`, `work-coder`).
    pub agent_name: String,
    /// Boundaries this agent may reach. Used as the per-request authorized slice.
    pub boundaries: Vec<Boundary>,
    /// When the token was minted (RFC3339 UTC, stored).
    pub created_at: DateTime<Utc>,
    /// When the token was revoked, if ever. `None` = active.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Hash a bearer capability token for storage / lookup — BLAKE3, hex-encoded.
///
/// The SAME function is used at mint time (vault-cli) and at auth time (the
/// daemon), so a presented token resolves to its stored hash deterministically.
/// SP-1/SP-5: a one-way hash of a high-entropy random token — no plaintext at
/// rest, no crypto-DIY (BLAKE3 is the codebase's existing audit-chain hash).
pub fn hash_capability_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

impl StorageBackend {
    /// Register a new agent with its hashed token + authorized boundaries.
    /// Fails if `agent_name` already exists (the PK) — callers rotate via
    /// `revoke` + re-add, or re-scope via [`Self::set_agent_boundaries`], rather
    /// than silently overwriting an existing agent's token.
    ///
    /// `token_hash` MUST come from [`hash_capability_token`]; the plaintext
    /// token is never passed to this layer.
    #[instrument(skip_all, fields(agent = %agent_name, n_boundaries = boundaries.len()))]
    pub async fn register_agent_token(
        &self,
        agent_name: &str,
        token_hash: &str,
        boundaries: &[Boundary],
    ) -> VaultResult<()> {
        let agent_name = agent_name.to_string();
        let token_hash = token_hash.to_string();
        let boundaries_json = encode_boundaries(boundaries)?;
        let created_at = Utc::now().to_rfc3339();

        self.metadata()
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "INSERT INTO agent_tokens \
                     (agent_name, token_hash, boundaries, created_at, revoked_at) \
                     VALUES (?1, ?2, ?3, ?4, NULL)",
                    rusqlite::params![agent_name, token_hash, boundaries_json, created_at],
                )
                .map_err(|e| VaultError::Storage(format!("register agent token: {e}")))?;
                Ok(())
            })
            .await
    }

    /// Resolve an agent by the hash of a presented bearer token — the auth hot
    /// path. Returns `None` when no ACTIVE (non-revoked) agent matches, which
    /// the daemon maps to a generic 401 (SP-4 fail-secure, no info leak).
    #[instrument(skip_all)]
    pub async fn lookup_agent_by_token_hash(
        &self,
        token_hash: &str,
    ) -> VaultResult<Option<AgentToken>> {
        let token_hash = token_hash.to_string();
        self.metadata()
            .with_conn_blocking(move |conn| {
                conn.query_row(
                    "SELECT agent_name, boundaries, created_at, revoked_at \
                     FROM agent_tokens WHERE token_hash = ?1 AND revoked_at IS NULL",
                    rusqlite::params![token_hash],
                    decode_agent_columns,
                )
                .optional()
                .map_err(|e| VaultError::Storage(format!("lookup agent by token hash: {e}")))?
                .map(decode_agent_row)
                .transpose()
            })
            .await
    }

    /// List every registered agent (active AND revoked), name-ordered — backs
    /// `vault-cli agent list`.
    #[instrument(skip_all)]
    pub async fn list_agent_tokens(&self) -> VaultResult<Vec<AgentToken>> {
        self.metadata()
            .with_conn_blocking(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT agent_name, boundaries, created_at, revoked_at \
                         FROM agent_tokens ORDER BY agent_name",
                    )
                    .map_err(|e| VaultError::Storage(format!("prepare list agents: {e}")))?;
                let rows = stmt
                    .query_map([], decode_agent_columns)
                    .map_err(|e| VaultError::Storage(format!("query agents: {e}")))?;
                let mut out = Vec::new();
                for r in rows {
                    let raw = r.map_err(|e| VaultError::Storage(format!("read agent row: {e}")))?;
                    out.push(decode_agent_row(raw)?);
                }
                Ok(out)
            })
            .await
    }

    /// Re-scope an ACTIVE agent's authorized boundaries. Takes effect on the
    /// agent's next request (ADR-SEC-001 D4 — boundaries resolved per-request).
    /// Returns `false` if no active agent of that name exists.
    #[instrument(skip_all, fields(agent = %agent_name, n_boundaries = boundaries.len()))]
    pub async fn set_agent_boundaries(
        &self,
        agent_name: &str,
        boundaries: &[Boundary],
    ) -> VaultResult<bool> {
        let agent_name = agent_name.to_string();
        let boundaries_json = encode_boundaries(boundaries)?;
        let rows = self
            .metadata()
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "UPDATE agent_tokens SET boundaries = ?1 \
                     WHERE agent_name = ?2 AND revoked_at IS NULL",
                    rusqlite::params![boundaries_json, agent_name],
                )
                .map_err(|e| VaultError::Storage(format!("set agent boundaries: {e}")))
            })
            .await?;
        Ok(rows == 1)
    }

    /// Revoke an ACTIVE agent's token (soft — sets `revoked_at = now`). The next
    /// request bearing that token resolves to `None` → 401. Returns `false` if
    /// no active agent of that name exists.
    #[instrument(skip_all, fields(agent = %agent_name))]
    pub async fn revoke_agent_token(&self, agent_name: &str) -> VaultResult<bool> {
        let agent_name = agent_name.to_string();
        let now = Utc::now().to_rfc3339();
        let rows = self
            .metadata()
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "UPDATE agent_tokens SET revoked_at = ?1 \
                     WHERE agent_name = ?2 AND revoked_at IS NULL",
                    rusqlite::params![now, agent_name],
                )
                .map_err(|e| VaultError::Storage(format!("revoke agent token: {e}")))
            })
            .await?;
        Ok(rows == 1)
    }
}

/// Serialize a boundary slice to the stored JSON-array-of-names form.
fn encode_boundaries(boundaries: &[Boundary]) -> VaultResult<String> {
    let names: Vec<&str> = boundaries.iter().map(Boundary::as_str).collect();
    serde_json::to_string(&names)
        .map_err(|e| VaultError::Storage(format!("serialize agent boundaries: {e}")))
}

/// rusqlite row → raw column tuple. Kept separate so `query_row` and
/// `query_map` share one column-extraction shape.
fn decode_agent_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, Option<String>)> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, Option<String>>(3)?,
    ))
}

/// Raw column tuple → [`AgentToken`], parsing the JSON boundary array and the
/// RFC3339 timestamps. A malformed stored row surfaces as `VaultError::Storage`
/// rather than a silent skip (a corrupted auth record must be loud).
fn decode_agent_row(
    (agent_name, boundaries_json, created_at, revoked_at): (String, String, String, Option<String>),
) -> VaultResult<AgentToken> {
    let names: Vec<String> = serde_json::from_str(&boundaries_json).map_err(|e| {
        VaultError::Storage(format!("decode agent boundaries {boundaries_json:?}: {e}"))
    })?;
    let boundaries = names
        .iter()
        .map(Boundary::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| VaultError::Storage(format!("invalid stored boundary: {e}")))?;
    let created_at = DateTime::parse_from_rfc3339(&created_at)
        .map_err(|e| VaultError::Storage(format!("decode created_at {created_at:?}: {e}")))?
        .with_timezone(&Utc);
    let revoked_at = match revoked_at {
        None => None,
        Some(s) => Some(
            DateTime::parse_from_rfc3339(&s)
                .map_err(|e| VaultError::Storage(format!("decode revoked_at {s:?}: {e}")))?
                .with_timezone(&Utc),
        ),
    };
    Ok(AgentToken {
        agent_name,
        boundaries,
        created_at,
        revoked_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::SqlCipherKey;

    const DIM: usize = 4;
    const TEST_AT_REST_KEY: [u8; 32] = [0x7a; 32];

    async fn open_test_storage() -> (StorageBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = SqlCipherKey::new("agent-token-store-test");
        let storage = StorageBackend::open_with_at_rest_key(
            &dir.path().join("metadata.db"),
            &dir.path().join("vectors"),
            &dir.path().join("graph.duckdb"),
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .expect("open StorageBackend");
        (storage, dir)
    }

    fn boundaries(names: &[&str]) -> Vec<Boundary> {
        names.iter().map(|n| Boundary::new(*n).unwrap()).collect()
    }

    #[test]
    fn hash_is_deterministic_and_input_sensitive() {
        assert_eq!(
            hash_capability_token("sekret-token"),
            hash_capability_token("sekret-token"),
            "same token must hash identically (lookup depends on it)"
        );
        assert_ne!(
            hash_capability_token("token-a"),
            hash_capability_token("token-b"),
            "different tokens must hash differently"
        );
        // Hex of a 256-bit BLAKE3 digest = 64 chars; never the plaintext.
        let h = hash_capability_token("sekret-token");
        assert_eq!(h.len(), 64);
        assert_ne!(h, "sekret-token");
    }

    #[tokio::test]
    async fn register_then_lookup_by_token_hash_round_trips() {
        let (storage, _dir) = open_test_storage().await;
        let token = "claude-token-xyz";
        let hash = hash_capability_token(token);

        storage
            .register_agent_token("claude", &hash, &boundaries(&["work", "personal"]))
            .await
            .unwrap();

        let found = storage
            .lookup_agent_by_token_hash(&hash)
            .await
            .unwrap()
            .expect("registered agent resolves by its token hash");
        assert_eq!(found.agent_name, "claude");
        assert_eq!(
            found.boundaries,
            boundaries(&["work", "personal"]),
            "authorized boundaries round-trip"
        );
        assert!(found.revoked_at.is_none(), "freshly registered = active");
    }

    #[tokio::test]
    async fn unknown_token_hash_resolves_to_none() {
        let (storage, _dir) = open_test_storage().await;
        storage
            .register_agent_token(
                "claude",
                &hash_capability_token("real"),
                &boundaries(&["work"]),
            )
            .await
            .unwrap();

        assert!(
            storage
                .lookup_agent_by_token_hash(&hash_capability_token("forged"))
                .await
                .unwrap()
                .is_none(),
            "a token that was never minted must not resolve (fail-secure)"
        );
    }

    #[tokio::test]
    async fn revoke_makes_token_stop_resolving_but_keeps_history() {
        let (storage, _dir) = open_test_storage().await;
        let hash = hash_capability_token("doomed");
        storage
            .register_agent_token("ex-agent", &hash, &boundaries(&["work"]))
            .await
            .unwrap();

        assert!(storage.revoke_agent_token("ex-agent").await.unwrap());

        assert!(
            storage
                .lookup_agent_by_token_hash(&hash)
                .await
                .unwrap()
                .is_none(),
            "a revoked token must not resolve at auth time"
        );
        // History survives for `agent list` + audit.
        let all = storage.list_agent_tokens().await.unwrap();
        let row = all.iter().find(|a| a.agent_name == "ex-agent").unwrap();
        assert!(
            row.revoked_at.is_some(),
            "revoked agent retained with marker"
        );

        // Revoking a non-active agent reports false (idempotent-ish).
        assert!(!storage.revoke_agent_token("ex-agent").await.unwrap());
        assert!(!storage.revoke_agent_token("never-existed").await.unwrap());
    }

    #[tokio::test]
    async fn set_boundaries_re_scopes_live() {
        let (storage, _dir) = open_test_storage().await;
        let hash = hash_capability_token("scoped");
        storage
            .register_agent_token("work-coder", &hash, &boundaries(&["work"]))
            .await
            .unwrap();

        assert!(storage
            .set_agent_boundaries("work-coder", &boundaries(&["work", "personal"]))
            .await
            .unwrap());

        let found = storage
            .lookup_agent_by_token_hash(&hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            found.boundaries,
            boundaries(&["work", "personal"]),
            "re-scope is reflected on the next lookup (D4 live)"
        );

        assert!(
            !storage
                .set_agent_boundaries("ghost", &boundaries(&["work"]))
                .await
                .unwrap(),
            "re-scoping a missing agent reports false"
        );
    }

    #[tokio::test]
    async fn duplicate_agent_name_is_rejected() {
        let (storage, _dir) = open_test_storage().await;
        storage
            .register_agent_token("dup", &hash_capability_token("a"), &boundaries(&["work"]))
            .await
            .unwrap();
        let err = storage
            .register_agent_token("dup", &hash_capability_token("b"), &boundaries(&["work"]))
            .await;
        assert!(
            err.is_err(),
            "re-registering an existing agent name must fail (PK), not silently overwrite"
        );
    }
}
