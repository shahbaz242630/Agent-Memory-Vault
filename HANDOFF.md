# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)
**Last updated:** 2026-05-06 (V0.1 internal alpha SHIPPED at tag `v0.1.0`; V0.1→V0.2 HANDOFF split landed at the same commit; V0.2 task decomposition pending at next session)
**Updated by:** Claude (Opus 4.7)

> **📁 V0.1 historical record:** `HANDOFF_V0.1_ARCHIVE.md` — frozen as of 2026-05-06. Full T0.1.1 → T0.1.12 phase narratives, ADRs 001-036 full text, tech-debt closures, plan-iteration histories. Cross-link out to that file when V0.2 work needs V0.1 detail; do NOT paraphrase.

---

## Current Status

**Active task:** **V0.2 task decomposition pending.** Next session opens V0.2 plan-paragraph drafting per BRD §6.2 scope. Working tree clean at the V0.1 alpha-cut commit. Three-platform CI matrix (`[ubuntu-latest, windows-latest, macos-latest]`) green; founder MSI installed at `C:\Program Files\Memory Vault\` (Phase 5e artefact, SHA-256 `03d12737...`).

**Per saved-memory `feedback_plan_iteration_depth_scales_with_design_surface.md`:** V0.2 alpha-distribution (the natural first task per BRD §6.2 + ADR forward-pointers) is **contract-establishing** scope (Windows + macOS code-signing pipelines + OS keychain migration + vault-tauri `--mcp-stdio-server` subcommand-split + Tauri 2 starter-template diff audit + ES module migration). Plan-iteration depth: **2-3 iterations expected** before lock.

---

## V0.1 retrospective (one-paragraph summary)

**V0.1 internal alpha SHIPPED 2026-05-06** across **12 sequential tasks** (T0.1.1 → T0.1.12) and **36 ADRs**. CI matrix sustained green across `[ubuntu-latest, windows-latest, macos-latest]` from T0.1.11 Phase 1 onwards. Founder dogfood (T0.1.12) ran 6+ hours cumulative on Windows 11 dev box per ADR-029 branch (2): 11 memories saved, 14+ search queries, full-laptop reboot persistence test ✅, Start Menu launch path test ✅, persistent VAULT_KEY user env-var test ✅. **2 findings filed, both Phase 3 lib→bin Tauri-template-default-drop class** — Finding #1 (search returns max_results regardless of relevance, mitigated by lowering UI default 10→3 per Phase 5e), Finding #2 (stray Windows console window alongside Tauri UI from missing `windows_subsystem = "windows"` attribute, fixed in Phase 5e). Finding #3 honest closure note per ADR-036 BRD §6 line 1393 amendment (≥3 issues → ≥2 issues OR honest closure). **Saved-memory `feedback_runtime_confirmation_after_web_spike.md` triple-validated** during T0.1.11 (Phase 1 ort/ORT version coupling + Phase 5b rmcp stdin-hang per ADR-034 + Phase 5c Tauri 2 `window.__TAURI__` default per ADR-035). Founder-distribution MSI artefact: SHA-256 `03d127371f6a881366e2f048d81f2785de97f68236c5d52747bf0100284d0a06` (106.02 MB / 111,173,632 bytes; Phase 5e build). **Full V0.1 historical record:** see `HANDOFF_V0.1_ARCHIVE.md`.

---

## V0.2 scope (BRD §6.2)

Per Agent_Build_Specification.txt §6.2:

- **Sleep consolidator** — background entity/relationship extraction from raw memories during idle periods. Builds the knowledge graph that semantic-only retrieval can't directly access.
- **Boundaries hardening** — multi-boundary UI + provenance + access-control surface beyond V0.1's hardcoded "default" boundary.
- **Cross-device sync** — encrypted Yjs CRDT-based sync across the user's devices via end-to-end encrypted relay. Server cryptographically cannot read.
- **30-user beta cohort** — alpha-distribution task: signed installers + onboarding flow + telemetry + feedback loop.

**Polished onboarding** per BRD §5.11 V0.2 — V0.2 likely introduces a frontend framework (React/Vue/Svelte) + bundler (Vite). When that lands, ADR-035's `withGlobalTauri:true` flips back to `false` (default) and dist/index.html is replaced.

---

## V0.2 task decomposition

**TBD at next session's plan-paragraph drafting.** Forward-pointers from V0.1 ADRs suggest the natural first task is **alpha-distribution** consolidating multiple V0.2 hard-gates (see "V0.2 hard-gate ADR forward-pointers" section below). T0.2.x task numbering will land at first V0.2 plan iteration.

---

## Locked-into-code invariants (forward-carried verbatim from V0.1)

These invariants are pinned by V0.1 code or tests — V0.2 work that touches the relevant subsystems MUST preserve them or explicitly amend with ADR + reasoning. **Verbatim per `feedback_quote_locked_artefacts_dont_paraphrase.md` discipline.**

### Storage / Cascading
- **`MAX_RETRY_QUEUE_DEPTH = 10_000`** (`crates/vault-storage/src/cascading.rs:73`)
- **`MAX_ATTEMPTS = 8`** + schedule **1, 2, 4, 8, 16, 30, 60, 120 seconds** + **±25% jitter** (`crates/vault-storage/src/retry_queue.rs:45`, schedule per ADR-009 amendment)
- **`LAST_ERROR_MAX_BYTES = FAILURE_REASON_MAX_BYTES = 4096`** (`crates/vault-storage/src/retry_queue.rs:54` + `crates/vault-storage/src/dead_letter.rs:33`)
- **`RetryQueue::poll_due` ordering: `(sequence_id ASC, next_attempt_at ASC)`** — strict FIFO per memory_id anchored to audit `seq` per ADR-017
- **`dead_letter.resolution` enum strings:** `'retried_succeeded'` | `'retried_failed'` | `'acknowledged'` | `'auto_recovered'`
- **`JitterSource: Send + Sync`** supertrait (extended at T0.1.6 C1b for tokio::spawn `Send` constraint at T0.1.10)
- **`is_permanent` classifier** covers `DimensionMismatch` / `AccessDenied` / `Storage(msg).contains("schema")` per ADR-009 amendment
- **`StorageBackend::open` returns `Ok` with `DegradedMode` flag on `validate_readable` failure** — never errors out so vault-cli triage stays available per ADR-018
- **`VectorStore::validate_readable` + `GraphStore::validate_readable` MUST exercise data-decode** (read row + parse column values), NOT metadata-only — per ADR-018 ("any manifest/fragment store recreates the blind spot if the check is metadata-only")
- **`VectorStore::contains(id) -> bool` is O(1) per id** (LanceDB impl uses `count_rows(Some("id = '<uuid>'"))`)

### Application / Lifecycle
- **Cascading retry worker spawned by `Application::start()`** — `tokio::sync::watch::channel(false)` + `worker.run(rx)` spawn + `Sender` returned for shutdown signaling. Per ADR-034 V0.1 fix-forward (was `start_with_mcp` pre-Phase-5b)
- **`AppConfig` field names CI-enforced** (rename-prohibition discipline pin from T0.1.10 Phase 2b)
- **`AppConfig::Debug` impl redacts `SqlCipherKey` field** as `<redacted>` (regression check from T0.1.10 Phase 2b)

### Retrieval / Embedding
- **`EMBEDDING_DIM = 384`** (`crates/vault-embedding/src/provider.rs:29`) — bge-small-en-v1.5
- **`MAX_QUERY_BYTES = 2_048`** (`crates/vault-retrieval/src/retriever.rs:51`) — Q3 query-length cap
- **`MAX_RESULTS_CAP = 100`** (`crates/vault-retrieval/src/retriever.rs:46`) — Q3 max_results cap
- **`SemanticRetriever::retrieve` score formula:** `score = 1.0 - cosine_distance` (per Q7 contract), exactly one site
- **Score sort order:** score-DESC then `created_at`-DESC (per Q9 contract), then `take(max_results)`

### Frontend / Tauri
- **`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`** at top of `crates/vault-tauri/src/main.rs` — kills stray console window on Windows release builds (per Phase 5e Finding #2 fix)
- **`app.withGlobalTauri = true`** in `tauri.conf.json` — exposes `window.__TAURI__` to bundled HTML+JS (per ADR-035; flips back to `false` at V0.2 ES module migration)
- **`bundle.resources` map in tauri.conf.json:** `onnxruntime.dll` + `model.onnx` + `tokenizer.json` paths (per ADR-019 Phase 5a; V0.2 alpha-distribution adds per-platform `bundle.windows.resources` / `bundle.macOS.resources` / `bundle.linux.resources` syntax to drop the cross-platform placeholder hack)

---

## V0.2 hard-gate ADR forward-pointers

ADRs that pin V0.2 work — each MUST be addressed before V0.2 alpha-cohort distribution opens. Full ADR text in `HANDOFF_V0.1_ARCHIVE.md`.

| ADR | One-line summary | V0.2 trigger |
|---|---|---|
| **ADR-010** | LanceDB stores plaintext on disk for V0.1 only | **HARD GATE before T0.2.0** — encryption-at-rest must ship before any V0.2 beta user receives the product. ADR-010 banners (modal + persistent strip) removed at this gate. |
| **ADR-031** | V0.1 unsigned Windows MSI deviation | **HARD GATE before V0.2 alpha-cohort opens** — Windows code-signing cert procurement (~$200-500/yr) + WiX signing pipeline + GitHub Actions secret + DoD test asserting signed MSI. Removes 3 V0.1 compensating controls (founder-only / SHA-256 record / SmartScreen Run-anyway). |
| **ADR-029 amendment** | macOS signing pipeline (Apple Dev ID already enrolled) | **HARD GATE before V0.2 alpha-cohort opens (macOS portion)** — App Store Connect API key + GitHub Actions secret + notarization workflow (~1 day setup; cert procurement already done). |
| **ADR-032** | V0.1 SQLCipher passphrase from `VAULT_KEY` env var | **V0.2 alpha-distribution must migrate to OS keychain** — keyring-core ecosystem mid-migration at V0.1; revisit at V0.2 plan time + spike picks branch (D) keychain. Plaintext-in-user-env-registry compensating control retired. |
| **ADR-033** | macOS ORT-loading test cfg-skip (upstream microsoft/onnxruntime#24579 OrtEnv mutex race) | **Revisit triggers (any):** (a) upstream PR fixing 1.22.x (not Node-only); (b) Mac procured for founder dogfood pre-V0.2; (c) we drop ort for a different ONNX runtime. Re-enable cfg-skip when trigger fires. |
| **ADR-034** | V0.1 vault-tauri is UI-only; MCP integration deferred | **V0.2 alpha-distribution adds `vault-tauri.exe --mcp-stdio-server` subcommand-split** — clap dep + headless mode + concurrent-data-access mutex design (UI-mode and MCP-mode mutually exclusive OR daemon architecture). Claude Desktop / Cursor / etc. integration via this path. |
| **ADR-035** | V0.1 `withGlobalTauri:true` for minimal HTML+JS frontend | **V0.2 alpha-distribution likely introduces frontend framework + bundler (Vite)** — `withGlobalTauri` flips to `false` (default), dist/index.html replaced with bundled ES module imports from `@tauri-apps/api/core`. Same task adds explicit `script-src 'self'` (or hash-pinned) CSP directive to close the defense-in-depth gap named in ADR-035 compensating control #1. |
| **ADR-036** | BRD §6 V0.1 acceptance bar amendment (≥3 → ≥2 OR honest closure) | **No V0.2 trigger — permanent V0.1-only amendment.** V0.2 / V1.0 acceptance bars are separate text. |

---

## Active ADRs with V0.2+ implications (one-line summaries)

Beyond hard-gates above, the following V0.1-era ADRs remain active and shape V0.2 work. Full text in archive.

- **ADR-001** — CI runs on `[ubuntu-latest, windows-latest, macos-latest]` 3-platform matrix
- **ADR-002** — `#![forbid(unsafe_code)]` on all crates
- **ADR-003** — vault-tauri ships as binary at T0.1.11; library kept alongside for testable utilities
- **ADR-004** — `CLAUDE.md` is gitignored and never committed (local-only)
- **ADR-005** — `Boundary` validated newtype `[a-zA-Z0-9_-]{1,64}`
- **ADR-006** — `rusqlite` `bundled-sqlcipher-vendored-openssl` + monthly OpenSSL CVE check
- **ADR-007** — No manual `Debug` impls on types holding sensitive runtime state
- **ADR-008** — dryoc 0.7 path #1 (DryocStream-as-single-message) — locked at T0.1.4 follow-up; consumed at T0.2.9 sync
- **ADR-009** — Retry queue policy gates T0.1.6 (amended C1b)
- **ADR-011** — `protoc` per-machine build-time dep + monthly CVE check
- **ADR-012** — LanceDB feature minimization investigated; AWS SDK + dual-arrow accepted as V0.1 cost
- **ADR-013** — `chrono` `=0.4.39` pin (advanced from 0.4.38 at T0.1.9 Phase 1)
- **ADR-014** — ALPHA file write failure: WARN + proceed (file is secondary, log is primary)
- **ADR-015** — `Entity` and `Relationship` boundary-scoped at schema layer (BRD §5.1 deviation)
- **ADR-016** — Connection-ownership: orchestrator routes through `MetadataStore::with_transaction`
- **ADR-017** — Cascade-ordering invariant: strict FIFO per `memory_id` by audit `seq`
- **ADR-018** — `FullySynced` deferral + eager corruption validation in `StorageBackend::open`
- **ADR-019** — ort native-lib distribution: `load-dynamic` + bundled dylib + path-resolution layering
- **ADR-020** — Model + tokenizer integrity: paired-files SHA-256, fail-fast at startup
- **ADR-022** — `tokenizers` feature-trim (drop `esaxx_fast` + `progressbar`) for MSVC C runtime
- **ADR-023** — MCP wire format: no capability token field in V0.1; deferred to V0.2 sync/multi-agent
- **ADR-024** — vault-mcp JSON-RPC error mapping + locked `mcp.tool_invoke` details_json schema
- **ADR-025** — MCP trust-boundary contract: tool args UNTRUSTED, app-supplied `authorized_boundaries` TRUSTED
- **ADR-026** — rmcp pinned `=1.5.0` + April 15 RCE class scope analysis (server-not-host)
- **ADR-027** — Pre-dispatch validation: tracing-only, no audit-chain append
- **ADR-028** — Memory update semantics: preserve provenance, overwrite content + classification
- **ADR-029** — V0.1 founder dogfood platform: Windows accepted, BRD amended (Mac → Mac or Windows)
- **ADR-030** — vault-tauri MCP role: server-only, no host functionality in V0.1

---

## Open tech-debt (V0.2 deadlines)

V0.2-deadline items forward-carried from V0.1. Full original entries with cross-links + reasoning in archive's Tech Debt Backlog section. Recurring monthly checks (cargo audit / OpenSSL CVE / protoc / chrono / rustc / ort / tokenizers) continue under `.github/workflows/monthly-tech-debt.yml` automation.

- **BRD v1.3 T0.2.7: vector index intervention (HNSW or IVF)** — empirical 382ms median retrieval at 1K memories vs 200ms BRD §5.5 ceiling; intervention codified in BRD as explicit T0.2.7 deliverable + acceptance bar (vault-retrieval perf gate stays `#[ignore]`-d until T0.2.7 ships)
- **Phase 5c: `#[tracing::instrument]` on 5 Tauri command handlers** — observability gap surfaced at Phase 5c diagnosis; V0.2 alpha-distribution lands instrumentation across `crates/vault-tauri/src/commands.rs`'s 5 `*_inner` async fns
- **Phase 5b: WiX UpgradeCode + version-bump for auto-upgrade install** — V0.2 alpha-cohort needs auto-upgrade ergonomics; custom WiX template with stable UpgradeCode GUID
- **Phase 5a: Cross-platform Mac/Linux dylib bundling deferred** per ADR-029 branch (2) Windows-dogfood lock — V0.2 alpha-distribution adds `libonnxruntime.dylib` + `libonnxruntime.so` bundle.resources entries
- **Phase 5a: V0.1 MSI installer fatness ~106 MB** (model.onnx is 133 MB before MSI compression) — V1.0 mitigation candidates: smaller model swap (Matryoshka truncation), post-install download, shared system path
- **Phase 5e: Tauri 2 starter-template diff audit** — Phase 3 lib→bin conversion missed at least 3 Tauri-template defaults (`withGlobalTauri` per ADR-035 + `windows_subsystem` per Phase 5e Finding #2 + possible-fourth TBD); V0.2 alpha-distribution audits before introducing bundler + framework changes
- **`VaultError::WorkerSpawnFailed` variant re-evaluation at V0.2 alpha cut** — currently unreachable; remove if no concrete consumer surfaces
- **MCP server graceful-shutdown** — V0.1 known limitation; V0.2/V1.0 work when rmcp adds transport-level close API OR supervisor-pattern lifecycle lands
- **Disk-full handling on user device** — V0.2 hardening; SQLITE_FULL detection + `VaultError::DiskFull` variant + Tauri error toast
- **`VaultError::Storage(String)` grab-bag → structured variants** — `is_permanent` substring-matching cleanup; T0.2.x stand-alone refactor task
- **`pending_sync` sweep — extend schema migration 0003 with cascade payload** (`embedding BLOB` + `boundary TEXT`) at T0.2.x
- **macOS CI matrix `macos-latest` re-verification** — 1-2 month label migration cadence per actions/runner-images README; re-verify at each V0.2 task plan time
- **512-token context ceiling vs T0.2.x connector ingestion** — bge-small-en-v1.5 max sequence; nomic-embed-text-v1.5 candidate at T0.2.x connector kickoff
- **Annual model-currency re-check** — re-run T0.1.9 model spike at V0.2 release + before V1.0 GA
- **Future-swap evaluation criteria locked** — license / first-party ONNX / prefix-injection compat / MTEB-R parity / ≤512 MB
- **`.gitattributes` for line-ending normalisation** — quick win when convenient
- **Build env vars need persistent home** — `.cargo/config.toml` `[env]` block or `scripts/dev-build.sh` helper

---

## Cross-platform CI state

**Sustained green across `[ubuntu-latest, windows-latest, macos-latest]` from T0.1.11 Phase 1 onwards** through V0.1 alpha-cut (Phase 5e). Workflow file: `.github/workflows/ci.yml`.

- **fmt** — single ubuntu-latest job (rustfmt OS-agnostic)
- **clippy** — 3-platform matrix; `cargo clippy --workspace --all-targets -- -D warnings`
- **build-and-test** — 3-platform matrix; `cargo build --workspace --all-targets` + `cargo test --workspace`
- **Linux Tauri 2 native deps** preinstalled via apt step gated on `runner.os == 'Linux'`
- **macOS provisioning** invokes existing `setup-dev-env.sh` Darwin branch
- **vault-embedding test fixtures** cached via `actions/cache@v4` keyed on `setup-dev-env.{sh,ps1}` + `integrity.rs` hash
- **Tauri-build bundle.resources cross-platform existence-check workaround** — `touch` placeholder steps for clippy + non-Windows build-and-test (Phase 5d); V0.2 alpha-distribution removes via per-platform bundle-config syntax
- **Monthly tech-debt sweep** (`.github/workflows/monthly-tech-debt.yml`) — first Monday each month at 09:00 UTC; auto-opens GitHub issue with cargo audit + version-pin verification report

---

## Standing rules (CLAUDE.md-promoted defaults)

Per saved memory + CLAUDE.md (gitignored, local-only). One line each — full reasoning in saved-memory files.

- **Per-step CI green check before staging next push** — `gh run list --workflow=ci.yml -L 1` showing `success` on previous push before staging next; promoted to documented default 2026-05-04 (6/6 vault-code data points across T0.1.10).
- **Admin changes ride with next code commit, no standalone CI cycle** — HANDOFF.md edits, BRD amendments, ADR-only updates, tech-debt notes, doc-only changes do NOT get their own commit. Saves a 45-min CI cycle per admin commit.
- **(α) relaxation clause** — explicit user direction can relax strict gating for batched commits when time-cost of strict gating exceeds inherited-failure risk; relaxation acknowledged in commit message body.
- **Spike-before-lock** — uncertain API surfaces get a minimum-viable scratch spike before production code; methodology declared upfront (web research / compile-and-run / hybrid); findings captured in ADR.
- **Source-read the call graph BEFORE designing empirical investigations** — when triggers fire and a hypothesis ranking exists, source-read the failing operation's call graph FIRST (T0.1.10 Phase 1b lesson).
- **Quote locked artefacts, don't paraphrase** — operational doc text saying "per ADR-X" or "per §Y" must match the cited artefact verbatim or quote it.
- **Floor forecasts are pre-declarations, not estimates** — when a plan paragraph forecasts +N tests, breaching it (even by 1) requires plan-amendment surface BEFORE commit, not silent slip.
- **Trust `gh run list` / `gh run view` actual status, not `gh run watch` exit code** — watch-tool failures are network/rate-limit/session-drop transients, NOT CI failures.
- **Stop-and-escalate when scope drift or contract divergence surfaces** — don't paper over with `prop_assume!` or silent skips.
- **Plan-iteration depth scales with design-surface size** — contract-establishing tasks warrant 2-3 iterations; consume-existing-contracts tasks warrant 1 iteration + optional spike.
- **Web-research spike requires runtime confirmation in next phase** — TRIPLE-VALIDATED across T0.1.11 (Phase 1 ort/ORT + Phase 5b rmcp stdin-hang + Phase 5c Tauri 2 default). Pattern-recognition signal: V0.2 plan-time candidate discipline upgrade is "scheduled-runtime-confirmation in execution phases" — don't promote prematurely.
- **Confirm before every commit and every push** — combined approval per saved-memory `feedback_confirm_before_commit_push.md`. On approval, commit AND push without re-asking.
- **Run cargo from PowerShell on Windows, not Bash** — ADR-006's bundled-sqlcipher-vendored-openssl chain needs Strawberry Perl modules; PowerShell PATH resolves Strawberry first.
- **Surgical `cargo clean -p <crate>` first; full `cargo clean` is escalation** — stale-cache symptoms get per-crate clean first.
- **Never run parallel cargo invocations on the same workspace** — package-cache file lock; gates run strictly serial fmt → clippy → build → test.
- **Co-Authored-By tag uses bare "Claude"** — no model-version qualifier.
- **Broken CI is a regression, fix in same session** — never defer to "CI follow-up" tech debt.

---

## V0.1 archive cross-link

Full V0.1 historical record — every ADR's full text, every phase narrative, every plan-iteration history, every closed tech-debt entry, every retrospective — lives at:

**`HANDOFF_V0.1_ARCHIVE.md`** (frozen 2026-05-06; do not edit)

Convention: when V0.2 work needs V0.1 detail, **cross-link to the archive section, do NOT paraphrase** (per saved-memory `feedback_quote_locked_artefacts_dont_paraphrase.md`).
