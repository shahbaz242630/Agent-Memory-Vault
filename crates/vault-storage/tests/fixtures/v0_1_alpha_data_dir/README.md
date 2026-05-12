# V0.1 Alpha Data-Dir Fixture (ADR-041 SQLCipher bridge Tier 2 test)

**Captured:** 2026-05-10
**Captured by:** Founder (Shahbaz) on Windows 11 dev machine, via the V0.1 Tauri UI
**Source:** V0.1 SHIPPED build at commit [`1d72aac`](https://github.com/shahbaz242630/Agent-Memory-Vault/commit/1d72aac), tag `v0.1.0`

## Purpose (post-T0.2.0 Phase 3 sub-task (e))

This fixture's `lance/` subdirectory was deleted in sub-task (e) (2026-05-12) when the V0.1→V0.2 LanceDB migration code was removed wholesale (no real V0.1 vaults exist anywhere; the migration code was dead weight). The remaining `vault.db` + `vault.db-wal` SQLCipher files are retained for the ADR-041 SQLCipher passphrase bridge test:

- `vault-app/src/keychain.rs::tier_2_real_v0_1_vault_db_bridges_and_preserves_5_rows`

That test exercises the V0.1 VAULT_KEY → V0.2 keychain-derived passphrase rekey path against a real V0.1-shaped SQLCipher file. The bridge code (ADR-041) is kept because it composes cleanly with the keychain layer and the cost of retention is small.

## Fixture content

5 throwaway anonymized memories were saved via the V0.1 Tauri UI's Add tab:

| Row | Content (verbatim) | Memory type | Boundary |
|---|---|---|---|
| 1 | `Tier 2 fixture row 1` | semantic | default |
| 2 | `Tier 2 fixture row 2` | semantic | default |
| 3 | `Tier 2 fixture row 3` | semantic | default |
| 4 | `Tier 2 fixture row 4` | semantic | default |
| 5 | `Tier 2 fixture row 5` | semantic | default |

Content-equality assertions in the bridge Tier 2 test reference these strings via SQLCipher metadata read.

## Capture key (intentionally checked in)

```
VAULT_KEY = fixture-capture-key-do-not-use-in-prod
```

This key was set as a **session-only PowerShell env var** at fixture-capture time, overriding the founder's persistent USER `VAULT_KEY` for the V0.1 process invocation only.

The fixture-capture key is **intentionally checked in** because:
- It is fixture-only and never used for real data
- The Tier 2 bridge test needs to open `vault.db` with the V0.1-era passphrase to assert on metadata
- The key has a self-documenting name that prevents accidental production use

## File inventory (post-sub-task-(e) cleanup)

```
vault.db                    — SQLCipher metadata (98 KB)
vault.db-wal                — SQLCipher write-ahead log (650 KB; SQLite replays on open)
graph.duckdb                — DuckDB graph store (12 KB; empty schema only)
graph.duckdb.wal            — DuckDB WAL (2.9 KB)
```

`lance/` subdirectory previously held V0.1 LanceDB data fragments for the now-deleted migration tests; removed in sub-task (e) when the V0.1 LanceDB migration was retired.

**Intentionally excluded from the fixture:** `vault.db-shm` (SQLCipher per-process shared-memory file; regenerated on every open, contains no permanent state).

## Capture procedure used (UI capture)

UI capture procedure:
1. From a PowerShell session: `$env:VAULT_KEY = "fixture-capture-key-do-not-use-in-prod"`
2. `& "C:\Program Files\Memory Vault\vault-tauri.exe"` (V0.1 binary)
3. In the Tauri UI's Add tab: type each of the 5 fixture row strings, choose memory type `semantic`, leave boundary as `default`, click Save. Repeat for all 5 rows.
4. Close the Tauri UI cleanly (do NOT kill via Task Manager).
5. Copy `%APPDATA%\com.shahbaz242630.memory-vault\` (excluding `vault.db-shm`) into this fixture dir.
