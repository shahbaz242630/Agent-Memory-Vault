-- 0007_agent_tokens.sql
-- ADR-SEC-001 (local multi-agent vault daemon) — per-agent capability tokens.
--
-- The shared daemon (rmcp streamable-HTTP on loopback) replaces stdio's implicit
-- OS-process trust: any local process can reach a loopback socket, so each agent
-- must present a bearer capability token to connect (BRD §11.4.4 step 1 +
-- §11.12 vault-mcp "capability tokens for every connection"). The token scopes
-- the agent to a set of boundaries (ADR-SEC-001 D3/D4/D6): WHICH boundaries it
-- may reach. Within a shared boundary, agents still see each other's memories
-- (D5 — boundaries are the walls, not agents).
--
-- ZERO-KNOWLEDGE / SP-1: the plaintext token is NEVER stored. Only its BLAKE3
-- hash is persisted; at auth time the daemon hashes the presented token and
-- looks it up here. Revocation is SOFT (revoked_at) so `agent list` keeps the
-- history and the audit trail survives; the auth lookup filters revoked_at IS
-- NULL. Lives in the already-SQLCipher-encrypted vault.db — same posture as
-- every other table, no new crypto path (ADR-SEC-001 D9).

CREATE TABLE IF NOT EXISTS agent_tokens (
    agent_name  TEXT PRIMARY KEY,   -- stable agent identity (lowercase kebab-case)
    token_hash  TEXT NOT NULL,      -- BLAKE3 hex of the bearer token; plaintext NEVER stored
    boundaries  TEXT NOT NULL,      -- JSON array of authorized boundary names
    created_at  TEXT NOT NULL,      -- RFC3339 (UTC)
    revoked_at  TEXT                 -- RFC3339 (UTC); NULL = active
);

-- Auth hot path: the daemon resolves an agent by the hash of the presented
-- token on every request. UNIQUE because one token maps to exactly one agent.
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_tokens_hash
    ON agent_tokens(token_hash);
