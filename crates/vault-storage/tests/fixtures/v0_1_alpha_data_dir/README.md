# V0.1 Alpha Data-Dir Fixture (Tier 2 — real V0.1 binary capture)

**Captured:** 2026-05-10
**Captured by:** Founder (Shahbaz) on Windows 11 dev machine, via the V0.1 Tauri UI
**Source:** V0.1 SHIPPED build at commit [`1d72aac`](https://github.com/shahbaz242630/Agent-Memory-Vault/commit/1d72aac), tag `v0.1.0`
**MSI SHA-256:** `03d127371f6a881366e2f048d81f2785de97f68236c5d52747bf0100284d0a06` (Phase 5e build, 106.02 MB / 111,173,632 bytes — the actual MSI installed at `C:\Program Files\Memory Vault\vault-tauri.exe` on the founder's machine when this fixture was captured)

## Purpose

This is the **Tier 2 fixture** for the V0.1 → V0.2 plaintext-to-sealed migration test (T0.2.0 close-out plan, Phase 2). It is a faithful snapshot of what V0.1 actually wrote to disk during a real capture session, NOT a synthetic Parquet construction (that's Tier 1's job). The migration test runs against a deep copy of this fixture and asserts that the post-migration sealed dir preserves all 5 rows + framing-byte expectations.

Cross-references:
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" §4 (three-tier fixture strategy)
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2" §3 (UI capture procedure decision + CLI fallback rationale)

## Fixture content (5 memory rows)

5 throwaway anonymized memories saved via the V0.1 Tauri UI's Add tab:

| Row | Content (verbatim) | Memory type | Boundary |
|---|---|---|---|
| 1 | `Tier 2 fixture row 1` | semantic | default |
| 2 | `Tier 2 fixture row 2` | semantic | default |
| 3 | `Tier 2 fixture row 3` | semantic | default |
| 4 | `Tier 2 fixture row 4` | semantic | default |
| 5 | `Tier 2 fixture row 5` | semantic | default |

Content-equality assertions in the Tier 2 migration test reference these exact strings.

## Capture key (intentionally checked in)

```
VAULT_KEY = fixture-capture-key-do-not-use-in-prod
```

This key was set as a **session-only PowerShell env var** at fixture-capture time, overriding the founder's persistent USER `VAULT_KEY` for the V0.1 process invocation only. The persistent USER `VAULT_KEY` was never touched and never disclosed.

The fixture-capture key is **intentionally checked in** because:
- It is fixture-only and never used for real data
- The Tier 2 test needs to open `vault.db` to assert on metadata; without the key, the test cannot decrypt
- The key has a self-documenting name that prevents accidental production use

## File inventory (23 files, ~779 KB total)

```
vault.db                                       — SQLCipher metadata (98 KB)
vault.db-wal                                   — SQLCipher write-ahead log (650 KB; SQLite replays on open)
graph.duckdb                                   — DuckDB graph store (12 KB; empty schema only — V0.1 doesn't populate this from UI saves)
graph.duckdb.wal                               — DuckDB WAL (2.9 KB)
lance/
  ALPHA_DO_NOT_STORE_REAL_DATA.txt             — V0.1 ADR-010 compensating control marker (337 bytes)
  memories.lance/
    _latest.manifest                           — pointer to latest version manifest (498 bytes)
    _transactions/0..5-{uuid}.txn              — Lance transaction log (6 files: init + 5 saves)
    _versions/1..6.manifest                    — Lance manifest per version (6 files)
    data/{5 uuid}.lance                        — 5 Lance data fragments (one per saved memory, ~2.3 KB each)
```

**Intentionally excluded from the fixture:** `vault.db-shm` (SQLCipher per-process shared-memory file; regenerated on every open, contains no permanent state).

## ⚠️ CRITICAL V0.1 file-format finding (drift from the iteration 1 + 2 plan text)

V0.1 lance 0.15 wrote `.lance` files using **Lance's own binary format**, NOT raw Parquet. Empirical inspection at fixture-capture time:

- `.lance` data files: **end with `4C 41 4E 43`** = ASCII `"LANC"` (last 4 bytes)
- `_latest.manifest` and `_versions/*.manifest`: **also end with `LANC`** magic
- Files do **NOT** start with PAR1 magic (`50 41 52 31`)

This **invalidates** the iteration 1 and iteration 2 plan text that named "PAR1 magic on root files" as the V0.1 detection signal. The correct V0.1 detection signal is:

> A `.lance` file in `<table>/data/` ends with `0x4C 0x41 0x4E 0x43` ("LANC")

Phase 2 migration's detection rule must check for `LANC` at file END, not PAR1 at file START. This is a plan-amendment surface — flag in HANDOFF.md alongside the Phase 2 implementation commit.

## Capture procedure used (UI capture)

CLI capture path was unavailable: vault-cli at `1d72aac` is dead-letter-recovery + divergence-triage only — it has no `add-memory` / `put` / `store` / `create` subcommand. Verified empirically via `git show 1d72aac:crates/vault-cli/Cargo.toml` (description: *"Operator CLI for the Memory Vault. Dead-letter recovery + divergence triage. See BRD §5.2 / ADR-009 / T0.1.6."*) + `git show 1d72aac:crates/vault-cli/src/main.rs` (`Command` enum has only `DeadLetter { action }` + `DivergenceCheck`).

UI capture procedure:
1. From a PowerShell session: `$env:VAULT_KEY = "fixture-capture-key-do-not-use-in-prod"` (session-only override; persistent USER env var preserved)
2. `& "C:\Program Files\Memory Vault\vault-tauri.exe"` (launches V0.1 binary as PowerShell child; binary inherits session env vars)
3. In the Tauri UI's Add tab: type each of the 5 fixture row strings, choose memory type `semantic`, leave boundary as `default`, click Save. Repeat for all 5 rows.
4. Close the Tauri UI cleanly (X button or Alt+F4 — let the close handler run; do NOT kill via Task Manager).
5. Copy `%APPDATA%\com.shahbaz242630.memory-vault\` (excluding `vault.db-shm`) into this fixture dir.

## Re-capture procedure (if fixture refresh ever needed)

If a future change requires re-capturing the fixture (e.g., V0.1 binary lost, schema needs refresh, content changes), the same UI procedure above applies. The V0.1 binary at commit `1d72aac` can be rebuilt from source via:

```
git worktree add ../memory-vault-v0_1 1d72aac
cd ../memory-vault-v0_1
cargo build --release -p vault-tauri
# Build artifact at: target/release/vault-tauri.exe
```

(Note: V0.1 dep tree is heavy — lancedb 0.8 + lance 0.15 + datafusion 40 + arrow 51/52 + AWS SDK + openssl-vendored. Expect 60-90 min full build on a 16 GB Windows box. The shipped MSI at the founder's `C:\Program Files\Memory Vault\` can be reused if available, skipping this build.)

If `keyring`-based key sourcing ever lands in vault-cli (V1.0 multi-vault scope per ADR-040 forward-compat), the CLI capture path becomes viable and this README's CLI-fallback rationale section becomes historical.

## Discipline cross-references

- `feedback_quote_locked_artefacts_dont_paraphrase.md` — the LANC-vs-PAR1 finding above is exactly the drift this discipline exists to catch; named here verbatim instead of paraphrased
- `feedback_runtime_confirmation_after_web_spike.md` — fixture-capture itself was the runtime confirmation that V0.1 file-format assumptions in iteration 1+2 needed correction
- `feedback_source_read_call_graph_upstream_of_empirical.md` — vault-cli capability check used `git show` against `1d72aac` rather than checking out the worktree (cheaper, no working-tree disruption)
