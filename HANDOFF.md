# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-07-20 (session 21) — **🖥️ BETA UI SLICE 2 SHIPPED + FOUNDER LIVE-VERIFIED.** All four honest-static surfaces are now live on real backend commands: Boundaries (list + create, backed by new migration 0008 `boundaries` table), Agents (real ADR-SEC-001 capability-token registry + revoke), Settings (real path / version / counts / audit-chain result), and the home "recently remembered" list (now reads the vault, so **agent-written memories finally appear**). ADR-SEC-003 (desktop UI reads across all boundaries) in §1. **LIVE-VERIFIED by founder same session** (home list shows 4-June memories = headline fix proven; boundaries/agents/settings/revoke all correct). Live verification also FOUND AND FIXED a shipped bug: the native `confirm` dialog renders OK-only in this webview, so `forget` (permanent delete) and `revoke` could not be declined — replaced with a real in-app dialog, and `tests/frontend_contract.rs` (9 guards, negative-verified) now prevents its return plus any command-wiring mistake. ADR-SEC-003 (UI reads all boundaries) + ADR-SEC-004 (user deletes are permanent, NO 30-day retention; agent-deletes carved out) in §1. **NEXT = packaging/installer.** See §1.

**Prior (session 20):** **🖥️ BETA UI SLICE 1 SHIPPED + LIVE-VERIFIED.** The Claude-Design "Quiet" direction (founder's design project "Agent Memory Vault UI/UX" → `Memory Vault App.dc.html`) is implemented in `crates/vault-tauri/dist/` — vanilla JS/CSS, no framework, fonts bundled locally. 3-step onboarding (sequential boot-check animation → connect-agent with REAL `vault-cli mcp serve` snippets → first memory) + home shell (Memories tab fully live on the real Tauri commands; Boundaries/Agents/Settings honest-static until slice 2). Founder ran the full loop live against the real engine (onboard → keep memory → recall in different words → forget) — verified working. ADR-086 (UI content policy: white-label / plain-English taxonomy / honest-UI) in §1. **NEXT = UI slice 2 (backend commands so every tab is live), then packaging/installer.** See §1.

> **How to read this file:** §1 is the only thing you must act on. §2–§5 are current ground truth (incl. the post-scale roadmap in §5). §6 onward is reference you pull from when planning. Deep detail (full ADR text, session-by-session history, tuning evidence) lives in the four archives — cross-linked by ADR number. **Do not paraphrase archived ADRs — quote them.**

---

## 1 · 🟢 NEXT SESSION OPENER — 📦 PACKAGING / INSTALLER ARC (the last beta blocker). The UI is DONE: slices 1 + 2 both shipped, gate-green, and founder-live-verified. Sessions 2–18 detail is archived → `HANDOFF_V0.2_PART3_ARCHIVE.md`.

> ### ▶️ START HERE — current state (2026-07-20, session 21 close)
>
> **In one line:** the desktop UI is finished and proven against the real encrypted vault — no honest-static placeholders, no outstanding UI work — so the only thing between us and beta users is that there is no installer.
>
> ### 🔴 UNCOMMITTED WORK IN THE TREE — COMMIT THIS FIRST, BEFORE ANY NEW WORK
>
> Founder chose at session-21 close to defer the commit to next session. **The work is finished and ALL GATES ARE GREEN — it needs committing, not redoing.**
>
> **Working tree contains (verified `git status` at close):**
> - `M crates/vault-tauri/dist/app.js` — `confirmAction()` + both destructive call sites converted
> - `M crates/vault-tauri/dist/index.html` — confirm-dialog markup
> - `M crates/vault-tauri/dist/styles.css` — `.confirm-*` styles
> - `?? crates/vault-tauri/tests/frontend_contract.rs` — the 9 guards (NEW FILE, untracked — easy to miss with `git add -u`; use `git add -A`)
> - `M HANDOFF.md` — this section, ADR-SEC-004, live-verification results, tech debt, the one-click arc
>
> **Gate state at close (nothing changed after):** build 0 warnings · **396 tests 0 failed** · clippy 0 warnings (`--all-targets`) · `fmt --check` clean · no `Cargo.lock` drift.
>
> **CI precondition ALREADY SATISFIED:** slice 2's push run `29734278549` completed **success** (59m46s) before session close — verified, not assumed. The CI-gate rule is met; re-confirm with `gh run list --workflow=ci.yml -L 1` if you want belt-and-braces, then commit + push.
>
> **Prepared commit message** is in the session-21 transcript; if unavailable, the substance is: fix ungated destructive actions (native `confirm` is OK-only in this webview → `forget`/`revoke` could not be declined) + add `frontend_contract.rs` guards (negative-verified) + ADR-SEC-004 (user deletes permanent, no 30-day retention; agent-deletes carved out) + slice-2 live-verification results.
>
> **Then** start the packaging/installer arc below.
>
> **Banked this session (session 21) — UI slice 2:**
> - **Migration `0008_boundaries.sql`** — boundaries become first-class rows instead of a value implied by `memories.boundary`. Backfilled from existing memories (`created_at` = `MIN(created_at)` of that boundary's memories, i.e. when the boundary actually came into being); `default` always seeded so the list is never empty. **No FK** from `memories.boundary` → `boundaries.name`: a failed registry write must never block a memory write (recall is sacrosanct).
> - **`vault-storage/src/boundary_store.rs`** — `list_boundaries` (LEFT JOIN so empty boundaries survive; counts exclude superseded + cold-archived) + `create_boundary` (idempotent: `Ok(false)` = already existed).
> - **Six `VaultAdapter` methods** (`list_recent_memories` / `list_boundaries` / `create_boundary` / `list_agents` / `revoke_agent` / `verify_audit_chain` / `total_memory_count`) on the CONCRETE type, deliberately NOT on the `vault_mcp::Adapter` trait — that trait is the surface every connected agent reaches, and vault-wide boundary enumeration there would be a least-privilege regression (§11.2 SP-2). Same rationale as the existing `append_tauri_command_audit`.
> - **`commands.rs` → `commands/` directory** (`memory.rs` / `boundary.rs` / `agent.rs` / `settings.rs` + `mod.rs`) per BRD §5.11's stated file list; the flat file would have hit ~700 lines.
> - **Frontend** — `renderBoundaries` / `renderAgents` / `renderSettings` / `renderMemList` all read real commands; the `mv_recent` localStorage cache is DELETED and cleared on upgrade.
>
> **📐 ADR-SEC-003 (session 21) — the desktop UI reads across all boundaries.**
> - **Context:** `search_memories` hardcoded `vec![Boundary::default_name()]`. Once the Boundaries tab can list `work` / `personal`, a UI that cannot search them makes the tab decorative.
> - **Decision:** Tauri-layer reads are OWNER-scoped, spanning every registered boundary. `list_recent_memories` uses an unfiltered `MemoryFilter`; `search_memories` resolves its slice from `list_boundaries()`.
> - **Reasoning:** boundaries are mandatory access control scoping *agents* — BRD §11.4.3 rule 5: *"The LLM/agent never sees memories outside its authorized boundary."* The Tauri layer acts as `ActorKind::User`, the vault owner, who is the party that GRANTS agents their scope. Fencing the owner out of their own boundaries protects nothing.
> - **Does NOT weaken §11.4.3:** agent-facing paths still resolve their authorized slice per-request from the capability token (ADR-SEC-001 D4) and none route through these methods. Rules 3/4 (filtering at the storage layer, unbypassable) are untouched — the `boundaries` table is a name registry, never consulted on a read path.
> - **Fail-secure:** if the registry read fails, `search_memories` falls back to `default` alone (SP-4) rather than widening.
>
> **🐛 LIVE-VERIFICATION FINDING (2026-07-20) — the confirmation dialogs could not say no. FIXED.**
> Founder testing `forget` observed the memory delete and the popup appear afterwards. Root cause: the webview renders `window.confirm()` as an **OK-only message box** — there is no Cancel, so the user CANNOT decline and the destructive action proceeds regardless. Both destructive surfaces were affected: `forget` (permanent delete, unrecoverable) and `revoke` (slice-2 code — I copied slice 1's pattern without questioning it). A gate that cannot say no is not a gate.
> **Fix:** in-app promise-based `confirmAction()` (`dist/app.js` + `.confirm-*` markup/styles) that resolves only on a real choice. Cancel is the dominant button AND holds initial focus, so Enter and Esc both mean "don't"; backdrop click cancels; text set via `textContent` (the revoke prompt interpolates an untrusted agent name). The two remaining `alert()` calls are deliberately left — they are error NOTICES, not gates, so OK-only is the correct shape.
> **Lesson for future UI work:** never use `window.confirm` / `window.prompt` in this webview for anything that gates an action. It is not a browser and does not behave like one.
>
> **📐 ADR-SEC-004 (session 21) — deletion is immediate and permanent for user-initiated deletes; NO 30-day retention. Amends BRD §11.5.4.**
> - **Context:** BRD §11.5.4 specifies soft delete with 30-day retention for undo, then cryptographic shredding. Founder challenged this while reviewing the delete flow: *"if user wants to delete then let them delete."*
> - **Decision:** **user-initiated deletion (UI / owner action) is an immediate hard delete** — `DELETE FROM memories`, no retention, no trash can. This is what is already implemented; this ADR ratifies it as intentional rather than leaving it as undocumented drift from §11.5.4. **Agent-initiated deletion (MCP `memory_delete`) is explicitly CARVED OUT** and remains open — see below.
> - **Reasoning:**
>   1. **Retention contradicts the product promise.** The vault's pitch is user-owned, zero-knowledge storage. Silently retaining a memory the user deliberately deleted — potentially something sensitive — for a month is the behaviour the product is positioned AGAINST. "Delete" must mean delete.
>   2. **Soft delete introduces a resurfacing failure mode.** It would require every read path to filter deleted rows; a single missed filter surfaces a deleted memory to an AI agent mid-conversation. Recall correctness IS the product ([[project_correctness_is_the_product]]) — voluntarily adding a way for deleted content to come back is a bad trade.
>   3. **The mis-click case is already handled** by the real confirmation dialog above, without retaining anything.
>   4. Local-first single-owner vault: no shared/team context where another party's delete needs review.
> - **CARVE-OUT (open, not decided): agent-initiated deletes.** An agent deleting over MCP has NO human in the loop; a misinterpreted instruction could remove memories the user never agreed to lose and might not notice for weeks. A recovery window has genuine value **there** — and is narrower and safer than a blanket policy, since it never retains anything the owner personally chose to destroy. **Founder agreed 2026-07-20 that this is the right shape.** Decide it as part of the write-side agent-permissions arc, not as a deletion-policy change.
> - **Trade-off accepted:** no undo for owner deletes. Mitigated by the confirmation gate + the fact that the UI states plainly that there is no recovery period.
> - **Alternatives rejected:** (a) BRD §11.5.4 as written — 30-day retention for all deletes: contradicts the privacy promise (reason 1). (b) Retention only for "sensitive" memories: requires classifying sensitivity, which we cannot do reliably and which is itself a privacy surface.
> - **NOTE — sync forward-compat:** cross-device sync (deferred, [[project_sync_deferred_until_paying_users]]) needs deletion **tombstones** for CRDT convergence. A tombstone is `{id, deleted_at}` metadata ONLY — it carries no memory content — so it is compatible with this ADR. Do not let "sync needs tombstones" get mis-read as "sync needs retention".
>
> **📐 ADR-087 (session 22) — first-run model acquisition lives at the Tauri layer; the reranker downloads rather than ships. Implements BRD §5.11.**
> - **Context:** the Qwen3 reranker (1.15 GB) had **no acquisition path whatsoever** — not in `bundle.resources`, gitignored, absent from `MODEL_PROVENANCE.md`, and no download script (`grep -i "qwen\|rerank" scripts/setup-dev-env.*` → no hits). The dev copy was provisioned by hand with no repeatable procedure. Separately, `main.rs` hardcoded `rerank_model_path: None`, so the desktop app ran on the cosine gate.
> - **Decision:** model acquisition is a **Tauri-layer** responsibility, and the reranker joins Phi-4 as a **first-run download**, not an installer payload.
> - **Quoting the spec** — BRD §5.11 `vault-tauri`: *"Handles installer concerns (model download, first-run setup)"*, and under its constraints: *"First-run setup: detect missing model files, show download UI, fetch from Hugging Face mirror"* and *"Stub installer pattern: ~10MB initial install, model download on first launch."* This is implementation of an existing spec, not a new architecture. Founder independently proposed the same stub-installer shape 2026-07-20, and the distribution research reached it a third time.
> - **Division of responsibility:** **integrity** (expected SHA-256) stays in `vault-embedding` beside the consuming code — `model_fetch` references `QWEN3_RERANKER_{MODEL,TOKENIZER}_SHA256`, never restates them, so a hash has exactly one home. **Acquisition** (the URL) lives in `vault-tauri`, because where bytes come from is an installer concern; `vault-embedding` stays a pure inference crate handed paths.
> - **Why not put the downloader in `vault-embedding`:** it would need `vault-llm`'s `ensure_model_at_path` (ADR-043), and `vault-embedding → vault-llm` drags llama.cpp + Vulkan/Metal into the embedding path. BRD §2 dependency flow forbids it. `vault-tauri` sits above both and already pulls each transitively via `vault-app`, so the direct deps add no new weight.
> - **Provenance VERIFIED, not assumed (2026-07-20):** live Hub listing of `shawnw3i/Qwen3-Reranker-0.6B-seq-cls-ONNX` confirms `model.onnx` at repo **root** (not under `onnx/`) at **1,192,779,696 bytes** and `tokenizer.json` at **11,422,654 bytes** — byte-identical in size to the validated fixtures, so the pinned hashes will verify. URLs use the `/resolve/` endpoint (HF's documented client download path, most generously rate-limited). Only these **two** files are needed — confirmed by reading `reranker.rs`: `commit_from_file(model_path)` + `Tokenizer::from_file(tokenizer_path)`; the other five files in the fixture dir are unused.
> - **Bug fixed in passing (not a drive-by — it is in code this depends on):** `model_loader.rs` built its in-flight path as `path.with_extension("gguf.partial")`, which **replaces** the extension. Harmless while Phi-4 was the only caller; for `model.onnx` it produced `model.gguf.partial`, and two models in one directory could collide on a single partial name — the same shared-`.partial` class that broke the weekly CI smoke job. Now appends: `model.gguf` → `model.gguf.partial` (byte-identical to prior behaviour, regression-pinned) and `model.onnx` → `model.onnx.partial`.
> - **Absent files yield `None`, never a startup failure.** `Application::new` documents `None` as graceful degradation. Refusing to start because an optional quality component is missing would be worse than ranking on cosine — recall is sacrosanct.
> - **Deliberately NOT in this step:** the download is not triggered at startup. A silent 1.15 GB transfer would make first launch look frozen — the exact failure the distribution research flagged. `model_fetch::ensure_reranker` is the transport; the first-run progress UI is its own piece of work against the "Quiet" design.
> - **DECIDED (founder, 2026-07-20) — download completes before real use, but OVERLAPS onboarding.** Founder's first call was a plain blocking wait ("better experience is let it fully download"); he then refined it, and the refinement is the design: **run the download underneath the existing onboarding flow.** The user connects their agent, creates boundaries and adds a first memory — work they must do anyway — while the fetch completes behind the welcome animation. Same guarantee as blocking (nothing runs on a degraded engine once they are actually using the vault), but no dead time. Founder: *"the first time user is still going to take time setting up stuff like connecting with agent etc... which can also buy us time while the backend re-ranker or heavy stuff loads."*
>   - **Consequences for the progress UI:** (a) real progress with a total, surfaced in the onboarding chrome rather than as a modal blocker; (b) a retry path — a failed download must not strand a user mid-onboarding; (c) onboarding completion must GATE on download completion, so a fast typist cannot outrun the fetch and reach search on the cosine gate; (d) state the honest size before it starts (~1.15 GB reranker; ~3.6 GB total with Phi-4) — no ambush.
>   - **Measured on the founder's machine 2026-07-20:** cold download of the reranker is **96-131s** at ~9-12 MB/s (two live runs). Onboarding plausibly covers that. A slower connection will not be covered, which is exactly why (b) and (c) are requirements rather than polish.
>   - **Rejected:** background download with the app usable on the cosine gate meanwhile — faster to first use, but ships a first impression that is not the product.
>   - **Founder's fallback if the wait proves unavoidable on slow links:** say so honestly on the opening screen ("takes 1-2 minutes to set up"), and note that Managed/BYOK tiers have no local model to fetch at all — the wait genuinely disappears there rather than being artificially gated. That is a real tier difference, not a manufactured one. Fits [[project_three_mode_deployment]].
>
> **📐 ADR-088 (session 22) — model hashing moves off the async runtime to `spawn_blocking`. Found by measurement, not by reading.**
> - **Context:** the live download test (`reranker_download.rs`) reported a **warm cache check of 67.3s** — the cost of re-verifying the 1.15 GB reranker on an already-populated directory. Since `ensure_reranker` is what first-run wiring will call on every launch, that would have put 67s of pure hashing in front of the window appearing — and ~135s, because `Qwen3RerankerProvider::open` independently re-verifies the same file at load time (ADR-020).
> - **The number made no sense, which is what exposed the bug.** 67s for 1.15 GB is ~17 MB/s. Measured on the same machine, same file, `Get-FileHash` (.NET) does **545 MB/s cold / 717 MB/s page-cached**. 17 MB/s is below even unaccelerated software SHA-256 (~150-250 MB/s), so the hash function was never the bottleneck — the file READ was.
> - **Root cause:** `compute_sha256_of_file` read through `tokio::fs` inside the async fn. Every read round-trips through the blocking pool, so a 1.15 GB file became hundreds of scheduler hops. **It was also a standing violation of BRD §2** — *"All I/O is async (tokio). CPU-bound work (ML inference) is sync, called via `spawn_blocking`"* — hashing gigabytes is CPU-bound work that was stalling the runtime while it ran. This was a correctness bug that presented as a performance bug.
> - **Decision:** `compute_sha256_of_file` now delegates to a sync `compute_sha256_of_file_blocking` via `tokio::task::spawn_blocking`, using `std::fs` + `std::io::Read`. Chunk size (8 MB) and semantics unchanged; only WHERE the work runs changed. `JoinError` maps to `VaultLlmError::Io` so the caller-visible error surface is untouched.
> - **Measured result: 67.3s → 20.3s (3.3x).** Applies to Phi-4's 2.32 GB verification too, at twice the file size.
> - **HONEST GAP — the remaining 10x is probably the build profile, and is UNMEASURED.** 20.3s is still ~10x off .NET's 2.1s, but all our numbers are **debug builds**; `Get-FileHash` is always optimised. Rust debug is typically 5-10x slower on tight numeric loops, so the shipped (release) figure is plausibly low single digits — **but that is an inference, not a measurement.** Do NOT quote a release hashing number until one is taken. **Action: measure at packaging time**, when a release build exists anyway; do not spend a release compile on it before then.
> - **What this decision did NOT require:** a size+mtime "remember what we verified" shortcut was drafted and then dropped. It would have traded away tamper-detection (BRD §11.7.5 signed-binary chain) for speed we did not need. **No security property was weakened to fix this.** If a future measurement shows release hashing is still too slow, that trade returns as its own ADR-SEC entry — not as an optimisation.
>
> **🚦 Gates (all green, 2026-07-20):** `cargo build --workspace` 0 warnings (1m21s) · `cargo test -p vault-storage -p vault-app -p vault-tauri` **387 passed / 0 failed** · `cargo clippy --workspace --all-targets -- -D warnings` 0 warnings (27m29s) · `cargo fmt --all --check` clean. **+14 tests** (3 migration, 6 boundary-store validation, 5 command-layer incl. boundary-name injection cases + the ADR-086 white-label pin) — above the "security tests first" floor; the extras are surfaced, not trimmed.
>
> **⚠️ TWO TAURI GOTCHAS THAT COST TWO FAILED BUILDS — read before touching commands:**
> 1. **Adding a command is a TWO-file permission change.** `permissions/default.toml` DEFINES each `allow-*` (identifier + `commands.allow`); `capabilities/default.json` only REFERENCES it. Session 20's work order said "add `allow-*` permission in `capabilities/default.json`" — that instruction is INCOMPLETE and following it literally fails the build with *"Permission allow-X not found, expected one of …"*. Confirmed by reading `tauri-build-2.6.0/src/acl.rs`: with no explicit `AppManifest::commands` in `build.rs` (ours has none), Tauri falls back to reading the `permissions/` directory.
> 2. **`generate_handler!` cannot see through a `pub use`.** `#[tauri::command]` emits hidden companion items (`__cmd__<name>`, `__tauri_command_name_<name>`) beside the function; a re-export does not carry them. Register by DEFINING module path — `commands::memory::add_memory`, not `commands::add_memory`. Getting this wrong breaks EVERY command, including ones that previously worked.
>
> **✅ FOUNDER LIVE-VERIFICATION COMPLETE (2026-07-20) — slice 2 verified end-to-end against the real encrypted vault.**
> - **Home list** shows memories from **4 June** — conclusive proof of the headline fix: slice 1 shipped 11–12 July and the old `mv_recent` cache was only ever written by UI adds, so a June memory could ONLY have come from reading the vault.
> - **Boundaries** — all three creation paths confirmed at once: `testeval` (15→10 after delete testing) came from migration 0008's **backfill**; `work` (0) came from **`create_boundary`** via the UI; `default` (0) exists ONLY because 0008 explicitly seeds it — it has no memories so backfill could never have produced it. Counts track deletes live (15→10), so they query rather than cache.
> - **Agents** — both minted tokens listed; **revoke works**. Settings + footer counts correct.
> - **Settings** — real path / counts / version / `history verified` (a genuine audit-chain walk incl. that day's `create_boundary` row). ADR-086 white-label holds: no stack names, only the sanctioned AES-256 + Credential Manager exception.
> - **Launch recipe (exe direct, no cargo):** `VAULT_ORT_LIB_PATH` / `VAULT_MODEL_PATH` / `VAULT_TOKENIZER_PATH` → `crates/vault-embedding/test-fixtures/bge-small-en-v1.5/{onnxruntime.dll,model.onnx,tokenizer.json}`, **plus `LANCE_MEM_POOL_SIZE=268435456`** — running the exe directly bypasses `.cargo/config.toml`, so ADR-038's pool cap must be set by hand (exactly what the MSI launcher will need to do). Vault lives at `%APPDATA%\com.shahbaz242630.memory-vault`. `vault-cli` reads the same OS keychain, so it needs no passphrase — but it is a SINGLE-WRITER vault: never run vault-cli while the app is open.
> - **Vault backup:** `_backup_pre_0008/` in the vault dir holds pre-migration `vault.db` (+wal/shm). Delete when no longer wanted.
>
> **🧪 FRONTEND CONTRACT GUARDS ADDED (`crates/vault-tauri/tests/frontend_contract.rs`, 9 tests).** Pure `std` string analysis of the checked-in sources — no JS runtime, no DOM, no new dependencies, 0.01s. Closes the two gaps that actually bit us: (1) bans native `confirm(`/`prompt(` as action gates; (2) cross-checks the FOUR-file command wiring (`app.js` invoke → `#[tauri::command]` → `generate_handler!` → `permissions/default.toml` → `capabilities/default.json`). **Both of this session's build failures would have been caught by these**, in-crate and named. Includes a meta-test asserting the extractors find a non-empty, mutually-equal command surface — without it a parsing regression would make every guard pass vacuously.
> **NEGATIVE-VERIFIED, not just green:** a temporary probe reintroducing both bug classes was added to `app.js`; exactly the right 3 guards failed and the other 6 correctly stayed green; probe removed. A guard that has never been seen to fail is not evidence.
>
> **▶️ THE ARC — packaging / installer (the last beta blocker; agreed with founder 2026-07-12, unchanged).**
> There is no outstanding UI work. The five pieces:
>   - **(a) Tauri bundler MSI** — the `bundle` block in `tauri.conf.json` half-exists but points at TEST-FIXTURE resources; real packaging must bundle or download the models instead.
>   - **(b) First-run model download** with progress UI — the models are too big to ship inside the MSI. Per ADR-086 the UI says "downloading recall engine", NEVER model names.
>   - **(c) `vault-cli` placement** — onboarding hands the user an MCP snippet saying `"command": "vault-cli"`, which is only TRUE if the installer puts `vault-cli` on PATH. The installer must make the snippet honest, or onboarding ships a lie.
>   - **(d) Code-signing decision — founder's call.** Unsigned MSI ⇒ Windows SmartScreen warnings on every install; signing costs money. Needs deciding, not engineering.
>   - **(e) ADR-038: the MSI launcher MUST set `LANCE_MEM_POOL_SIZE=268435456`** (WiX pre-args). Dev runs inherit it from `.cargo/config.toml`; **installed builds do NOT.** Confirmed live this session — launching the built exe directly required setting it by hand, which is exactly the installed-build condition. Miss this and installed vaults can OOM-abort on merge-heavy writes.
>
> **⚠️ Verify before building (a):** `tauri.conf.json`'s `bundle` block was last touched for slice 1's window/title changes — read it fresh, do not assume the fixture paths are the only staleness.
>
> **▶️ THE ARC AFTER PACKAGING — one-click agent connection (researched 2026-07-20 at founder's request; the last onboarding blocker for beta).**
> **The problem:** onboarding hands a non-coder a JSON block and says "put this in the right file". Four failure points — find the file, paste without breaking JSON, know to restart the agent, and get **no feedback at all** if it didn't work. Silent failure doesn't read as "I pasted it wrong", it reads as "this product is broken".
> **Research finding — why Hermes / OpenClaw have one-click and we cannot copy it.** Both are the **agent** (MCP client); their one-click is a curated catalog inside their own app. Hermes: entries are manifests merged into the `hermes-agent` repo under `optional-mcps/` — *"There is no community submission tier; entries are added by merging a PR"* (Nous reviews each). OpenClaw: same shape via ClawHub skills. **We are the SERVER.** Their button is not ours to build — but getting LISTED in their catalogs is a distribution/outreach action, not engineering.
> **The real lever for a server author: MCP Bundles (`.mcpb`)** — the open Desktop-Extensions format under `modelcontextprotocol/mcpb`. A zip of the server + `manifest.json`; the user **double-clicks one file and it installs**. No JSON editing, no config-file hunting, no terminal, no PATH dependency. Claude Desktop also has a browsable Anthropic-reviewed extensions directory (listing via an interest form).
> **⚠️ RETRACTION — supersedes my earlier same-session proposal.** I first proposed that the app WRITE the user's agent config files itself (locate `claude_desktop_config.json` etc., merge, atomic-write). **`.mcpb` is strictly better and that proposal is withdrawn:** config-writing risks corrupting another app's config, depends on `vault-cli` being on PATH (packaging item c), and creates an arbitrary-file-write security surface we would have to defend under §11. The bundle format removes all three, and is the vendor-supported path rather than reverse-engineering file locations that can change under us.
> **⚠️ OPEN QUESTION — fold into the PACKAGING spike, do not solve twice.** A bundle is meant to be self-contained, and the recall engine is >1 GB of model files. Whether a bundle can ship that, or must download on first run, is **the same question as packaging item (b)**. Do not assume it works — verify before designing around it.
> **Shape (once the size question is answered):** (1) `.mcpb` bundle = flagship one-click, biggest audience; (2) assisted config-write ONLY for agents with no bundle support (Cursor / Codex / Antigravity) — with backup + atomic write + fixed internal path allowlist, never a user- or agent-supplied path; (3) catalog listings (Anthropic directory, Hermes PR, ClawHub) = outreach, no code.
> **Ship a "Check connection" button alongside whichever route wins.** It answers the one question a non-coder cannot answer themselves — *did it actually work?* — and it helps even when the config was pasted by hand. Arguably higher value per unit of effort than the one-click write itself.
> **Sources:** anthropic.com/engineering/desktop-extensions · github.com/modelcontextprotocol/mcpb · claude.com/docs/connectors/building/mcpb · hermes-agent.nousresearch.com/docs/user-guide/features/mcp · docs.openclaw.ai/cli/mcp
>
> **⚠️ Carried open threads:** weekly scheduled CI "real-model smoke" now failing THREE Sundays running (`28385029636`, `28803916921`, `29259145088`) while every push run is green — no longer comfortably a flake, worth reading the run logs. Welcome-animation replay length still undecided (founder call). File upload/import still deferred to V1.0.
>
> **🧪 CHOSEN NEXT TEST ARC — real end-to-end tests against the built app (founder call, 2026-07-20).**
> Founder chose full E2E over a simulated-DOM harness. **Rationale that decided it:** a DOM-simulation test would NOT have caught this session's dialog bug — a simulated browser implements `confirm` correctly, so the guard would have fired, the test would have passed, and the bug would still have shipped. Only driving the REAL webview reproduces webview-specific behaviour.
> **Sequencing (mine, stated openly): SPIKE FIRST, do not build cold** ([[feedback_spike_playbook_for_unknowns]]). Unknowns to settle in the spike, on Windows, locally: (a) does `tauri-driver` + **`fantoccini`** drive the built app — this route is **Rust-native, so it needs NO npm toolchain**, which removes the main objection to E2E; (b) can it assert the canonical case *"click forget → Cancel → memory still present"*; (c) driver availability per platform (Edge WebDriver on Windows, WebKitWebDriver on Linux) and version-pinning against WebView2.
> **CI decision deferred until the spike answers those.** Honest risk to weigh then: CI already takes ~58 min, E2E needs a full app build on both legs, and browser-driving tests are the classic flaky-CI source — and we have a standing rule that broken CI is a same-session regression ([[feedback_broken_ci_is_regression_not_techdebt]]). A permanently amber pipeline would be worse than no E2E. Local-only-until-proven is a legitimate landing spot.
>
> **🧹 Tech debt noted (not fixed — no drive-by):**
> 1. `CLAUDE.md` refers to the spec as `Agent Build Specification.txt` (spaces); the real filename is `Agent_Build_Specification.txt` (underscores). Costs a failed lookup at every session start.
> 2. **Version string is incoherent:** Settings renders `memory-vault 0.1.0 · V0.2 beta` — the `0.1.0` is real (`CARGO_PKG_VERSION`) but the crate has never been bumped, so the app reports 0.1.0 while the product is V0.2 beta. Honest but confusing to a user seeing both. Decide before beta: bump the crate to `0.2.0`, or drop the crate number from the UI.
> 3. **No operator visibility into boundaries or the audit log.** `vault-cli` has no `boundary list` and no `audit verify|tail`; the only way to inspect either is the GUI. For a product whose pitch is a tamper-evident audit trail, that is a real gap — it blocks supporting a beta user's vault and blocks the founder inspecting their own without the UI. Surfaced 2026-07-20 when CLI-verifying a UI-created boundary proved impossible.
> 4. **No frontend BEHAVIOUR coverage.** The contract guards above are source-level only; nothing exercises a click. This is what the E2E arc addresses.

---

## 1a · 📦 Session 20 opener (slice 1) — retained for context

> ### ▶️ START HERE — current state (2026-07-12, session 20 close)
>
> **In one line:** the engine is done + proven, and the product now has a real face — the beta UI shipped (slice 1) and the founder verified it live against the real engine. Remaining beta-blocker work: slice 2 (backend commands for the static tabs), then packaging/installer.
>
> **Banked this session (session 20, 2026-07-11→12) — UI slice 1:** `crates/vault-tauri/dist/` fully rebuilt (`index.html` + `styles.css` + `app.js` + `fonts/`; vanilla JS, no framework, no node toolchain): (a) **3-step onboarding** — welcome with sequential boot-check animation (one line completes → next appears → "Begin set up" fades in; ~2.8s/line per founder pacing feedback), connect-agent picker with REAL MCP snippets (`vault-cli mcp serve`; JSON for Claude Code/Desktop/Cursor/Antigravity/custom, TOML for Codex), first-memory screen (no taxonomy question — defaults semantic); (b) **home shell** — Memories tab fully live (debounced search → `search_memories`, add panel → `add_memory`, hover-"forget" → `delete_memory`, Ctrl+K focuses search), Boundaries/Agents/Settings honest-static until slice 2; (c) **fonts bundled locally** (Newsreader + IBM Plex Sans/Mono woff2, 136 KB — CSP `default-src 'self'` blocks remote and local-first demands it); (d) `tauri.conf.json`: title "Memory Vault" (Alpha tag dropped), min window 900×700. **Live E2E verified:** fresh cold build 27m58s (line-tables-only + `-j 2` recipe, ~15 GB — recipe held), founder ran onboard → keep memory → recall in different words → forget against the real encrypted vault: works.
>
> **📐 ADR-086 (session 20) — UI content policy (white-label + plain-English + honest-UI).**
> 1. **White-label:** user-visible strings never name underlying models/stack (bge / Qwen / Phi / ONNX / Lance / DuckDB / dimensions…). Capability + trust language instead: "recall engine ready — runs entirely on this device". DELIBERATE EXCEPTION: security specifics (AES-256, Windows Credential Manager) stay — 1Password-style trust language, founder-confirmed.
> 2. **Plain-English taxonomy:** semantic/episodic/procedural is write-side metadata that never gates recall (RetrievalQuery has no type filter), so the UI maps it to "a fact about me / something that happened / how I do things" (rows show fact/event/how-to); onboarding doesn't ask at all. Backend values + agent-facing MCP contract unchanged.
> 3. **Honest UI:** no fake data or claims. Boot checks state only what really happened at startup — third line is the audit log, NOT "mcp endpoint ready" (no MCP server runs in the Tauri process, ADR-034). No seeded fake memories, no fabricated counts; dead buttons (Lock vault / View audit log) dropped, not shipped broken. **File upload/import DEFERRED to V1.0 connectors arc** — dogfood-proven that document dumps fail the 512-token atomic-fact save contract.
>
> ### ✅ ALREADY DONE (don't redo — verified live 2026-07-12)
>
> - **Engine (sessions 1–19):** storage/crypto/retrieval/consolidation/multi-agent daemon — built, tested, live-proven, all committed. Details §2; archives for history.
> - **UI slice 1 (session 20, commit `ff19570`):** the full "Quiet" beta UI in `crates/vault-tauri/dist/` (index.html / styles.css / app.js / fonts/). Onboarding (welcome animation → agent connect → first memory) + home shell. **Memories tab is FULLY LIVE** on existing commands: `search_memories` (debounced), `add_memory` (onboarding + add panel), `delete_memory` (hover-forget). Founder ran the whole loop against the real encrypted vault — works. Boot recipe that worked: `CARGO_PROFILE_DEV_DEBUG='line-tables-only'` + `-j 2` + the three `VAULT_*_PATH` env vars → bge fixtures (cold 27m58s / warm seconds; ~15 GB target/).
> - **Deliberately static until slice 2 (honest placeholders, not bugs):** Boundaries tab (shows only `default`), Agents tab (shows locally-configured picks from localStorage, not the daemon registry), Settings (true-but-partly-hardcoded values), home "Recently remembered" list (localStorage cache of UI-added memories only — agent-written memories DON'T appear in it yet; search finds them fine).
>
> **🚦 CI state at session-20 close:** slice-1 push CI run `29167300494` was **in_progress** when the session ended. **FIRST ACT next session: `gh run list --workflow=ci.yml -L 1` and confirm it went green** before staging anything (per the documented CI-gate default). Frontend-only change, so failure is unlikely — but verify, don't assume.
>
> **▶️ NEXT STEP — UI slice 2: make the three static tabs + recent list real (touches `commands.rs` = §11 security surface).**
> Work order:
>   1. **Re-read BRD §11 IN FULL + §5.11** (project rule — any `commands.rs` / capabilities / permissions change is a security surface). Then write security tests BEFORE impl per §11.12 (IPC validation; boundary leakage where relevant).
>   2. **New Tauri commands** (each mirrors the existing pattern in `commands.rs`: `*_inner` testable fn + thin `#[tauri::command]` wrapper + `append_tauri_command_audit` row + register in `main.rs` `generate_handler!` + add `allow-*` permission in `capabilities/default.json`):
>      - `list_recent_memories(limit)` — newest-first across boundaries the UI can see; replaces the localStorage cache so agent-written memories appear on home.
>      - `list_boundaries()` (+ `create_boundary(name)` if the storage layer supports boundary creation without a memory — CHECK first; BRD §5.11 specs `list_boundaries`).
>      - `get_settings_info()` — real data dir path, version, audit-chain status, memory count.
>      - `list_agents()` — read ADR-SEC-001 capability-token registry so the Agents tab shows real grants (+ decide with founder whether revoke-token lands in slice 2 or later).
>   3. **Wire the frontend** — `renderBoundaries` / `renderAgents` / `renderSettings` / `renderMemList` in `dist/app.js` are already shaped to consume these; replace their static/localStorage branches.
>   4. **Gates:** frontend files don't need cargo, but the new commands do — full DoD on vault-tauri + the one-profile recipe above; founder confirms before any cargo invocation ([[feedback_confirm_before_cargo_build_and_check_disk]]).
>
> **▶️ THEN — packaging/installer arc (the real beta blocker, discussed with founder 2026-07-12):**
>   (a) Tauri bundler MSI (config half-exists in `tauri.conf.json` `bundle` block; currently points at test-fixture resources — real packaging must bundle or download models); (b) **first-run model download** with progress UI (models too big to ship in the MSI; per ADR-086 the UI says "downloading recall engine", never model names); (c) **`vault-cli` placement** — the onboarding MCP snippet says `"command": "vault-cli"`, which is only true if the installer puts vault-cli on PATH — installer must make the snippet honest; (d) **code-signing decision** = founder's (unsigned MSI → Windows SmartScreen warnings; signing costs money); (e) ADR-038 reminder: the MSI launcher must set `LANCE_MEM_POOL_SIZE=268435456` (WiX pre-args) — dev runs get it from `.cargo/config.toml`, installed builds do NOT.
>
> **⚠️ Open threads (parked, not blockers):**
>   - **Weekly scheduled CI "real-model smoke" job failing** (pre-existing): `cargo test -p vault-llm -- --ignored` failed the last two Sundays (runs `28385029636`, `28803916921`) while every push run is green (schedule trigger runs ONLY the smoke job). Investigate from run logs; suspect runner model-download/disk flake — confirm, don't guess.
>   - **Welcome animation plays full ~9s on every fresh first-run** (once per install; Replay via Settings). Pre-beta idea, founder hasn't decided: full theatre first run, ~1s quick version after. Do NOT build without founder sign-off.
>   - **File upload/import**: founder asked, deliberately DEFERRED to V1.0 connectors arc (ADR-086 — document dumps fail the atomic-fact contract). Don't re-propose for V0.2.
>
> **🅿️ GRAPH READ-CHANNEL VERDICT (why it's parked — tech-debt #9).** Hard 40-distractor dogfood (2026-06-29) showed **no graph win**: on the 2-hop probe graph-ON == graph-OFF, byte-identical; the true word-mismatched answer never reached top-10 either way. Three independent root causes, none a quick fix: (1) the reranker ranks lexical lookalikes ("is known for") above the true "specializes in" answer; (2) per-fact enrichment made **duplicate entity nodes** that break the multi-hop chain (needs real entity resolution); (3) extraction **missed edges**. The agent can already multi-hop via follow-up queries ([[project_architectural_lock_llm_out_of_read_path]]), so this is NOT a beta blocker. **Revisit ONLY if real agent/beta dogfood shows users genuinely need word-mismatched multi-hop answers the agent can't compose itself; otherwise drop it.** The pool-truncation BUG it surfaced is fixed + banked (above). Do NOT manufacture a win (founder posture). Full ADR text (ADR-SEC-002 Part 2 + Amendment 1) + the session 14–18 graph arc → `HANDOFF_V0.2_PART3_ARCHIVE.md`.
>
> **Already SHIPPED — the engine (all green + committed + pushed):** local encrypted storage (SQLite/SQLCipher + sealed LanceDB + sealed graph) · write+retrieval pipeline (BGE embed + Qwen3 reranker, recall-channels-first pool assembly) · boundaries (mandatory access control) · sleep consolidator (merge / contradiction / decay / scheduling / checkpoint+rollback / REPORT, incremental) · A1 cold archive · knowledge graph extracted + self-cleaning + encrypted at rest (**read-channel PARKED**, see above) · **multi-agent daemon (ADR-SEC-001) — built + live-proven** (two agents, real vault, boundary isolation + auth gate + single-instance guard) · MCP stdio + daemon HTTP, cross-agent proven (Claude / Cursor / Codex).
>
> **📦 What moved to the PART3 archive (frozen 2026-06-29):** the session-by-session openers for sessions 2–18 (worker/scheduler ADR-080, checkpoint+rollback ADR-081, incremental consolidation ADR-082, contradiction guard ADR-083, A1 cold archive ADR-084, Finding F / topic clustering ADR-085, the multi-agent daemon arc ADR-SEC-001, graph at-rest encryption + self-cleaning + read-path wiring ADR-SEC-002 + Part 2 + Amendment 1) with full locked-ADR text. The slim sections below (§2 onward) remain the live ground truth; quote the archive, don't paraphrase it.

## 2 · 🧭 Where the build is

V0.2 read/consolidate core is functionally complete and CI-green. The work since T0.2.3 was a long correctness-at-the-output arc (the founder thesis: *"memory is only useful if the output is correct"*). Net result:

- **Read path** returns structured facts, NO LLM at read (`StructuredReadPipeline`, ~500ms). The calling agent composes the answer. Recall-first by lock: never false-empty.
- **`memory_read`** is the primary answer path (returns structured `abstain`); **`memory_search`** is reorder-only + recall-safe (never false-empties) with an additive `weak_match` hint. (ADR-066/069/071)
- **Reranker** (Qwen3-Reranker-0.6B, cross-encoder) is the read relevance authority, lazily loaded off the MCP handshake. (ADR-059/070)
- **Consolidator** produces a per-boundary REPORT (structured knowledge state) nightly; contradiction detection is nearest-neighbor based. (ADR-053/065)
- **Cross-agent proven:** Claude, Cursor, Antigravity all read the vault correctly. Validated at 100 facts live across both tools and both model tiers.
- **Scale:** `scale_eval` harness shows correctness is scale-invariant 100→1k→10k (identical scorecard). The one 10k internal crash (a flaky, data-safe storage-worker race) is fixed + shipped (ADR-072).

**Last shipped commit:** `da10c0f` (ADR-072, 10k TOCTOU fix), CI-green run `27096332980`. Recent chain: `a3c938b`→`661d391`→`a1e4dac`→`da10c0f` all matrix-clean.

**The locked arc** ([[locked-next-arc-t03x]], amended 2026-05-26) — all four steps SHIPPED:
1. ✅ MCP `memory.write` description hardening (`93d1410`)
2. ✅ Consolidator → REPORT (Batch A, `f0cc158`, ADR-053)
3. ✅ Read returns structured facts, no LLM at read (Batch B Commit 6, `99052f2`, ADR-052/054)
4. ✅ Consolidator wired into runtime + manual CLI trigger (`f0cc158`)

Phase C (write-time decision loop) DEFERRED to V1.0+.

---

## 3 · 🔒 Architectural locks (do not relitigate without explicit founder sign-off)

- **LLM is OUT of the read path** (2026-05-26). The read consumer is itself an LLM (the agent); pre-composing prose was redundant. Vault returns structured facts; agent composes. Delivered ~170× local speedup, ~50× BYOK cost cut, ~10× Managed PAYG margin. Phi-4-mini stays at nightly consolidation only. [[project_architectural_lock_llm_out_of_read_path]]
- **Recall is sacrosanct.** A false-abstain (vault has the answer but says "I don't know") is the cardinal sin — far worse than a false-answer. Every read/search change is recall-safe by construction: reorder-only, never false-empty. [[project_memory_read_primary_search_recall_safe]]
- **Correctness of output IS the product.** Storage + retrieval are table stakes; correct output to the agent is the differentiator. Don't burn cycles on prose polish when the structured field is already correct. [[project_correctness_is_the_product]] · [[feedback_structured_contract_user_sees_via_agent]]
- **Correctness before latency** (V0.2). Get core quality to 100% first; don't preempt latency work until the founder signals the core is structurally solid. [[project_correctness_before_latency]]
- **Three-mode deployment** (Local $10 / BYOK $5mo / Managed PAYG) shares one codebase; every architectural decision must be mode-agnostic. Managed = per-user vault + per-user key. [[project_three_mode_deployment]] · [[project_managed_mode_per_user_vault]]
- **Zero-knowledge guarantee:** the server cryptographically cannot read vault contents. No crypto-path change without re-reading BRD §11 + an ADR-SEC entry.
- **Never recommend sub-7B models for read-time synthesis** (Qwen2.5-7B is the quality floor) — moot now that the LLM is out of read, but stands if read-synthesis is ever revisited. [[feedback_no_sub_7b_models_for_synthesis]]

---

## 4 · 🟠 Open threads (next arcs, NOT blockers for the 1k/10k validation)

### Thread 2 — retrieval vocabulary gap (Gap 1 SHIPPED; Gap 2 IMPLEMENTED — ADR-074, gates green, pending live validation + commit) — own arc, ACTIVE
**Status RE-DIAGNOSED 2026-06-09** (ground-truth probe on the real `seeded-vault-1k`, 3 domains — see §4.2 below; falsifies the 2026-06-08 framing). Gap 1 (read false-abstain) is SHIPPED (ADR-073). **Gap 2 is NOT "BGE can't handle paraphrase/idiom"** — natural idioms work fine ("call home" → Porto rank 1). The real root is a **vocabulary gap**: a fact phrased without the obvious keyword ("settled in **Porto**", "raising **twins**", "comes out in **hives**") gets outranked by — or in a dense-distractor field drops below — facts that carry the literal keyword. **The proven fix is document-side alias enrichment, NOT query expansion** (which backfires — it IS the keyword-soup that triggers the miss). Full evidence + fix validation in §4.2.

#### Gap 1 — read-gate false-ABSTAIN (gate layer; fact IS retrieved, gate drops it)
**The bug (confirmed live, 1k vault).** `memory_read` **false-abstained** — returned `relevant_facts: []`, `abstain: true` — on facts that ARE in the vault:
- *"how do I stay fit"* and *"exercise running cycling"* → both `abstain: true`, even though *"runs ten kilometres three times a week"* AND *"cycles to the office"* are stored (the 2nd query literally contained "running"/"cycling"). The agent only recovered by falling back to `memory_search`. A weaker agent would have told the user "I don't have that" — the exact cardinal sin the recall-safe lock exists to prevent ([[project_memory_read_primary_search_recall_safe]]).

**Root cause (measured).** `memory_read` abstains on an **absolute reranker floor (ADR-059: logit 0 = relevance 0.50)**. But the reranker scores real answers far below that — and is sometimes actively wrong:

| live query | top relevance | #2 | separation | `weak_match` | truth |
|---|---|---|---|---|---|
| "stay fit" | 0.0388 | 0.0061 | ~6× clear winner | false | real answer (runs 10km) — **read abstained** ❌ |
| "morning routine" | 0.5256 (cycles) / 0.18 (flat white) | — | — | false | both real; flat white below 0.5 floor |
| "what does the user eat" | 0.0639 | 0.0473 | ~1.3× murky | true | real answer (*Japanese cuisine* didn't even make search top-10 — ranked below cafeteria-noise) |
| "operating system" (absent) | 0.000065 | 0.000055 | flat | true | genuinely nothing — abstain correct ✅ |
| "cat breed" (absent; dog present) | 0.00028 (dog) | 0.00003 | ~9× | false | no cat — dog is no-signal-level; agent correctly said "no cat, but a dog" ✅ |

Two takeaways: (1) **real answers live at relevance 0.04–0.99; no-signal/wrong-neighbor lives at 0.00006–0.0003** — a ~100× gap. The logit-0 (0.50) floor sits on top of the real answers and mows them down. (2) `memory_search` already gets all these RIGHT (separation-based, never empties); only `memory_read`'s gate is broken. ADR-066 said "reranker is a re-orderer, NOT a precision authority" — yet ADR-059 still uses its absolute score as the abstain gate. That contradiction IS the bug.

**The fix (3 parts, every threshold backed by the live data above).**
1. **Kill the logit-0 abstain floor** in `memory_read` (the whole false-abstain).
2. **Adopt `memory_search`'s separation-based logic** + a *much* lower no-signal floor (~relevance 0.001). Real answers (≥0.04) clear it; C7/C8 (≤0.0003) don't. (Separation alone is insufficient — C8's dog separated 9× yet is no-signal-level — so combine separation with the low absolute floor.)
3. **Never hard-empty `relevant_facts`.** Even when `abstain`-leaning, return the top candidates + a `weak_match`/confidence hint and let the agent judge. Proven live: given the dog fact, the agent correctly abstained on "cat" while surfacing the dog. `abstain` becomes a *hint*, not a fact-shredder.

Net: make `memory_read` behave like `memory_search` already does. The over-inclusion/false-answer side (salary→$, cat→dog, keyboards-leak) is the *same* root (absolute reranker score is an unreliable gate) and the weak-match hint covers it too — the agent judges instead of the vault hard-deciding.

#### §4.2 Gap 2 — RE-DIAGNOSED 2026-06-09 (ground-truth probe, fix proven)
**What it is NOT.** The 2026-06-08 framing ("the idiom 'call home' misses Porto; fix = vault-side query expansion") is **FALSIFIED**. Ground-truth probing of the real `seeded-vault-1k` (new `probe_live_vault` / `probe_family_domain` / `probe_enrichment` tests in `scale_eval.rs`, run live across 3 domains — location, relationships, health) shows the bare idiom finds the fact fine: *"where does the user **call home**"* → Porto **rank 1** (0.4339); *"live"* → rank 1 (0.95); *"is the user married?"* → rank 1.

**What it actually is — a VOCABULARY GAP, two failure modes.** A fact phrased *without* the obvious keyword — "settled in **Porto**" (not "lives in"), "raising **twins**" (not "kids"), "comes out in **hives**" (not "allergy") — is outranked by, or (in a vault with a DENSE field of lexically-overlapping distractors) drops out of the candidate pool below, facts that carry the literal keyword:
1. **Recall miss** under dense matching-domain noise. The ONE outright miss was the agent's **keyword-soup** query `"home location city country lives residence"` → Porto **ABSENT**, top 0.0013, buried under Salt-Lake-City/travel distractors. (That Salt-Lake-City pool is exactly what 2026-06-08 mis-pinned on "call home".) Sparse domains (family/health) don't bury the target, but →
2. **Confidence collapse.** Keyword-soup queries score ~0.008–0.03 (no-signal level) → `memory_read` abstains even when recall holds. And a 3rd-party fact carrying the keyword ("Marcus carries an epipen for his peanut allergy", 0.96) outranks the user's own answer ("comes out in hives", 0.18) for "is the user allergic?".

**So keyword-padding is the TRIGGER, not the cure** — vault-side query expansion would replicate the harmful soup. The fix is **document-side**.

**The fix — PROVEN by A/B probe (`probe_enrichment`).** Enrich each fact's *embedded text* with normalized aliases/topics. Measured on the hardest case: bare Porto **ABSENT** → enriched Porto (`"…Topics: home, lives, residence, city, country, location"`) **rank 1 @ 0.9965** on the exact killer keyword query, with **no regression** on natural ("where does the user live": enriched #1 / bare #2). Twins: bare rank 5 → enriched rank 1 (natural AND keyword). **Where it lives:** the consolidator's Phi-4 pass already touches every fact → generate the alias/topic line there (fits [[project_locked_next_arc_t03x]] consolidator arc; keeps the LLM out of the read path). **Query-side expansion SHELVED; stronger embedder = last resort.** Full detail: [[project_1k_live_paraphrase_recall_miss]].

**Decision LOCKED 2026-06-09: generate aliases with Phi-4 at consolidation (Option B), NOT write-time agent aliases (Option A).** Rationale + recon in §1 opener step 1. Remaining ADR-074 specifics to lock: (a) Phi-4 alias prompt + output format; (b) `metadata` storage key + embed-text composition (`content + aliases`); (c) when it runs / re-embed cost (backfill of existing facts is the point). A deterministic synonym map was rejected — "settled in Porto" → "home/residence" needs comprehension, not a thesaurus.

#### Harness gap — root was DEEPER than "favorable phrasings" (FIXED 2026-06-09)
`scale_eval`'s `scale_correctness_eval` scored **"false-abstain: 0 / recall perfect"** at 1k/10k for TWO reasons: (1) favorable fixture phrasings (added plain/idiom/keyword `_phrasing` variants + a per-phrasing recall scorecard), and — the deeper one — (2) its readiness poll broke at *"Rivian searchable"* (BM25 hits before the vector lands), so the query pass ran against a **half-drained vector store** (`ready after 0s` vs the honest `1546s`) — almost no distractor competition → artificially perfect recall. **Fixed:** the poll now waits for `LanceVectorStore::count == total` (mirrors `seed_live_vault`). NOTE: even fully-drained at 1k the in-process harness can't reproduce the keyword miss without the dense-distractor condition — the faithful repro is the real-vault probe (`PROBE_VAULT_DIR`). The ruler (variants + drain fix + 3 probe tests) is uncommitted; it rides with the Gap-2 fix commit per commit-only-with-tested-fix.

**Verdict:** engine solid + premium experience excellent (Opus: rich answers, graceful abstains on blood-type/salary/OS, never hallucinated the salary-$ or cat→dog traps, offered to save missing facts). But these two recall-robustness gaps gate the "battle-tested" call. **Full evidence:** [[project_1k_live_read_false_abstain]] + [[project_1k_live_paraphrase_recall_miss]] + this session's 1k Antigravity transcript (17 queries). Related tech-debt #1 (carry-cosine-through-fusion + per-candidate filter, §8) is the same surface as Gap 1.

### Carried follow-ups (not blockers)
- **REPORT_MISSING cleanup** — run the consolidator on the live seeded vaults to clear the cosmetic `status: degraded` warning (needs `--phi4-model`, server not holding the vault).
- **`max_results` 10 → 5** — proven safe at top-5; one change at a time.
- **Antigravity `instructions.md` rewrite** — steer agents to prefer `memory_read`; empty result = not in vault.
- **`as_of` is write-time, not fact-time** — content dates aren't parsed; blocks the A5/A4 temporal contradiction cases. Open decision: settable `as_of` vs date-extraction. [[project_as_of_write_time_blocks_a5_temporal]]

---

## 5 · 🗺️ Post-scale roadmap (V0.2 remaining) — pick the start point

Once the 1k/10k live test passes (§1), the retrieval **core** is proven correct + scale-solid. These four pillars complete V0.2. Founder picks where to start; my recommended order is **1 → 2 → (fork: 3 or 4)**.

**1. Read precision (Thread 2) — close the last known quality gap.** 🟢 *recommended first*
The vault sometimes returns a confident wrong-neighbor instead of abstaining ("salary?" → catering $; "cat?" → the dog; "instrument?" → cello-correct + keyboards leaked). Fix = recall-safe `weak_match` hint on `memory_read` (let the agent judge, never drop a fact). Contained, high-value, squarely the "correctness IS the product" thesis. Full detail in §4 (Thread 2). Related: tech-debt #1 (carry-cosine-through-fusion + per-candidate filter) in §8.

**2. Sleep consolidator — make it COMPLETE on its own at scale.** 🌙 *(updated 2026-06-17 by the scale pressure-test)*
The build-out is DONE: **Scheduling** (T0.2.6) ✅, **Phase 4 decay** (T0.2.4) ✅, **Checkpoint + rollback** (T0.2.5) ✅ — all shipped. The open Pillar-2 work is now **performance, specifically incremental consolidation**: the 1k pressure-test (§1) proved the full nightly run **times out at the 1800s budget on ≥~1k facts** because every run re-processes the WHOLE vault (re-embed all ~14 min/1k → re-cluster → re-merge all). So the auto-scheduler fires nightly but never completes → no REPORT/checkpoint/decay/enrich ever land at realistic scale. **Fix arc (full scope in §1):** (1) stop re-embedding facts that already have stored vectors; (2) wire the `since`-checkpoint param so a run touches only facts changed since the last successful run (changed facts as SEEDS, partners drawn from the whole corpus — ADR + recall test required); (3) loosen the dedup gate; (4) cosine-prune contradiction pairs. The remaining unbuilt piece is A1 **cold archive** (T0.2.4's other half — write a policy ADR first).

**3. Cross-device sync (`vault-sync`) — the big multi-device feature.** 🔄
The V0.2 promise: your memory on every device, readable by any agent, **without the server ever reading it** (zero-knowledge sync). Largest + most security-sensitive surface → re-read BRD §11 first, ADR-SEC entries required. **Ship gate:** tech-debt #4 (`pending_sync` sweep + migration 0003 payload, §8) MUST land before sync beta opens.

**4. Beta packaging + 30 real users.** 🚀
Onboarding flow, desktop-app polish, getting it into hands. The V0.2 finish line (BRD §6.2: 30 beta users).

**The one real fork (a couple weeks out, founder's call):** after 1 + 2, do **sync first** (full multi-device vision before anyone tries it, longer to first users) **or beta-on-one-device first** (real users + feedback sooner; even single-device the vault is genuinely useful; sync follows). Recommendation leans beta-first per the bootstrap reality — get one device perfect + dogfood-proven before taking on the heavy sync surface.

---

## 6 · 📦 Consolidator inventory — what's built vs not (read FIRST when planning consolidator work)

`vault-consolidator` has ~1,000 LOC production + ~1,200 LOC tests. Do NOT re-discover.

**Built + tested ✅**
| Component | File | Notes |
|---|---|---|
| Phase 1 — Clustering | `phases/cluster.rs` | Cosine ≥ 0.92, top-5 NN, union-find transitive closure, deterministic. Re-embeds (metadata `Memory.embedding` is `None`). ADR-045 |
| Phase 2 — LLM decide | `phases/merge.rs::decide_merge` | JSON-schema `LlmProvider::complete_json` → `MergeOutcome::{Merge, KeepSeparate, Contradiction}`. ADR-044 |
| Phase 3 — Apply merge | `phases/merge.rs::apply_merge` | Summed `access_count` + max `confidence`, marks originals superseded (ADR-046), re-embeds. Graph rewrite WARN-deferred (tech debt §7) |
| Orchestrator | `consolidator.rs::run_consolidation` | All non-superseded → group by boundary (`BTreeMap`, deterministic) → Phase 1→2→3 → `ConsolidationReport` |
| Topic discovery | `topics.rs` | Connected-components (NOT K-means — ADR-068) |
| REPORT artifact | `report.rs` | Per-boundary structured JSON, atomic write. ADR-053 |
| Run-summary audit | `summary.rs` | Per-boundary Markdown, privacy-leak tested. ADR-047 |
| Runtime wiring | `vault-app::run_consolidation_with_safety` | Cross-process lockfile + 30-min timeout + tracing span |

**Not built ❌**
| Gap | Scoped | Status |
|---|---|---|
| Phase 4 — Decay | T0.2.4 | **Decay BUILT (ADR-075)**; cold archive still deferred — `memories_archived` returns 0 |
| ~~Checkpoint + rollback~~ | T0.2.5 | **BUILT 2026-06-16 (ADR-081, §8.13) — UNCOMMITTED.** Capture-by-diff in `run_consolidation`; `vault-cli checkpoint list`/`rollback <id>`; real `checkpoint_id` in the report + footer. Enrichment excluded; graph rollback deferred (tech-debt #2). |
| ~~Scheduling~~ | T0.2.6 | **BUILT 2026-06-14 (ADR-080, §8.12) — UNCOMMITTED pending test.** `scheduler.rs` pure timing + `Consolidator::schedule()` headless loop + app-layer production scheduler in `start_with_mcp`. Latency deferred. |
| `invalidate()` consumption | T0.2.7 Phase B | Contradictions queue to `ConflictReview`; bi-temporal `invalidate()` (ADR-051) not yet called. Partially addressed via REPORT auto-resolution on `clear_winner` |

---

## 7 · 🧰 Technique map (locked 2026-05-26) — summary

Mapped against: **A** Write · **B** Read · **C** Consolidate · **D** Sync · **E** Scale · **F** Privacy. Full table in PART2 archive.

- **Keeping:** HNSW (LanceDB top-K), cascading writes, std hashing, CoW-via-SQLite-WAL+Lance, Phi-4-mini at consolidation, BGE-small-en-v1.5 embedder, Tantivy BM25 + RRF + abstain.
- **Added this arc:** connected-components topic discovery (C), token-budgeted structured packing at read (B), startup wiring + CLI subcommand.
- **Deferring:** Cuckoo filters (sync, V0.2.9-13); per-tenant sharding / consensus / replication (V1.0+ Managed — prefer managed Postgres/Spanner over hand-rolled Raft).
- **Dropped (wrong tool):** Bloom filters, Z-order/Morton, quad trees, skip lists, external sorting.
- **Dead:** speculative decoding + the 120s p99 ceiling (Qwen is out of the read path).

The lock SIMPLIFIED the menu. The vault needs brilliant plumbing (filter + structure + pack), not exotic structures.

---

## 8 · 🐛 Tech debt — open items (live forward-pointers)

Full narrative for each in PART2 archive ("Tech debt — open items"). File pointers kept here so they don't lose their anchor.

1. **Read-relevance: per-candidate cosine filter + carry-cosine-through-fusion + retire vestigial BM25 gate.** Carry raw semantic cosine through `HybridRetriever` fusion onto `RetrievedMemory` (today `hybrid.rs:221-247` discards it), then filter per-candidate → removes double-embed, enables per-candidate precision filtering, lets the BM25 gate be retired. Closely related to Thread 2. Files: `vault-retrieval/src/strategies/hybrid.rs:221-247`, `structured_read_pipeline.rs`, `strategies/abstain.rs`. (Surfaced ADR-057)
2. **🟢 LARGELY CLOSED 2026-06-14 (ADR-078, §8.10).** Entity-extraction-at-consolidation is now BUILT — the consolidator extracts + writes entities + relationships per fact via the combined Phi-4 enrichment call (`phases/extract.rs` + `enrich_facts`). **Remaining tail:** `GraphStore::rewrite_relationships_for_memory(old, new)` for the merge path — a fact whose *content* changes re-extracts but leaves the prior content's relationships behind; `apply_merge` still has its graph-update `tracing::warn!` no-op (`phases/merge.rs::apply_merge`). Low priority while the graph is dogfood-only. Do NOT amend the BRD until the merge-rewrite tail closes.
   - **🚧 TRIPWIRE — DO NOT wire graph traversal into the read/answer path until BOTH hold (added 2026-06-15 after Shahbaz flagged "tech-debt that silently breaks the pipeline later"):** (a) the merge-path rewrite above is implemented; AND (b) graph **extraction completeness** is measured at scale. **Evidence (2026-06-15 tiny-vault scheduler run):** of 6 facts, Phi-4 produced clean entities+relationships for 5 but an **empty/incomplete graph for the Tesla fact** — its `drives`→`Tesla Model 3` edge was dropped as a dangling link (`extract.rs:219` requires both endpoints be listed entities). Root cause = **Phi-4 per-fact output variance**, NOT a code bug: `enrich_facts` processes every active fact and the lossiness is fully **instrumented** (`EnrichmentReport.{entities_created, relationships_failed, graph_write_failures, facts_failed}`) — it is counted, not silent, and CANNOT affect output today (graph is write-only in V0.2, not consumed at read). So the graph is **best-effort / incomplete by construction**; trusting it for answers before measuring + hardening extraction (prompt tuning / retry / completeness eval) would surface incomplete graph answers. This tripwire is the guard against exactly that. **Entity_type-stored-as-JSON-with-quotes was investigated 2026-06-15 and is NOT a bug** — `graph_store.rs:251-258` round-trips `EntityType` via `serde_json` symmetrically (`"person"` on disk ↔ `EntityType::Person` in memory); no fix needed.
3. **`VaultError::Storage(String)` grab-bag → structured variants.** `retry_queue.rs::is_permanent` substring-matches lance error wording (fragile; lance 4.0 wording is inconsistent). Add `SchemaMismatch`/`IoFailure`/etc., re-categorise ~30 call sites, rewrite `is_permanent` as exhaustive `match` + tripwire test. Files: `retry_queue.rs:240-275`, `vault-core/src/error.rs:139`, the ~30 `Storage(format!(...))` sites.
4. **✅ CLOSED 2026-06-13 (ADR-076, §8.8).** `pending_sync` sweep + migration 0003 cascade payload. Migration 0003 added `sequence_id` + `payload`; the overflow path persists the full cascade and `StorageBackend::drain_pending_sync` re-enqueues it (the `DivergenceDetector` Tier-0 sweep). The V0.2-sync ship-gate is met. (Note: stored the raw cascade `payload` rather than the sketched `embedding`/`boundary` columns — more faithful + version-agnostic.)
5. **Cosine NaN-vector lance upstream issue (LOW — community citizenship).** lance 4.0 filters NaN-distance rows from Cosine search (zero-magnitude vectors). Production unaffected (BGE vectors are L2-normalised, never zero). File a minimal-repro issue against `lancedb/lance`. File: `vector_store.rs:1248-1263`.
6. **🟡 Min-fix LANDED 2026-06-13 (`--test-threads=1` added to `ci.yml:702`); verify on the next Monday cron. Deeper unique-`.partial` fix still open (LOW).** Weekly real-model smoke red since 2026-05-18 — concurrent-download race (CI-infra, NOT a code regression). The `real-model-smoke` weekly cron job (`ci.yml:702`, `cargo test -p vault-llm -- --ignored`) has failed every Monday across 4 unrelated commits (`4ae8dbd`/`93d1410`/`2302842`/`a3e426b`). Root cause (source-confirmed): all 3 smoke tests run concurrently (no `--test-threads=1`) and `model_loader.rs::download_with_verify` writes to a **single shared** `.partial` path (`model_loader.rs:131`) then renames to final (`:156`); the winner's rename leaves the losers' rename hitting a vanished `.partial` → `Io NotFound code 2`. The test's own doc (`phi4_mini_smoke.rs:47-48`) assumes serial execution. **Min fix (CI-only):** add `--test-threads=1` to `ci.yml:702` — verifiable only via next Monday cron or a `run-llm-smoke`-labelled PR. **Deeper latent bug (LOW, prod single-writer + pre-download mitigates):** the shared `.partial` path means two cold-starting agent processes could corrupt each other's download — make `.partial` unique per download + treat "final already present after our stream" as success. Matters because this job is the ONLY CI coverage of the real Phi-4 consolidator path (dark for a month); re-light before leaning on the consolidator (roadmap §5 item 2). Files: `ci.yml:702`, `model_loader.rs:95-160`.

7. **`graph.duckdb` plaintext + native-encryption dead-end (LOW — graph empty in V0.2).** DuckDB native encryption can't write an encrypted DB offline on any bundled version (mbedtls is read-only; secure write needs the network `httpfs`/OpenSSL extension). Real path: bundle the httpfs/OpenSSL helper INSIDE the app and `LOAD` it from a local file. Fold into the Pillar-3 sync security review or whenever the graph first holds shippable data. (ADR-078 §8.10.)
8. **🔁 SUPERSEDED 2026-06-15 — `/FI _SECURE_SCL` shim REMOVED, replaced by a v143 (14.44) MSVC toolset pin (LOW — CI-infra workaround, pending CI-green verification).** The shim (`.github/msvc_fmt_secure_scl_shim.h` + the two `CXXFLAGS_*` steps, ADR-079) was a dead end: it only reached DuckDB's cc-rs build and leaked a feature macro into llama.cpp's ggml → a 2nd VS2026 break (`std::hardware_destructive_interference_size`). **Fix (Option 2):** keep the tuned `windows-2025` image but pin the MSVC toolset to **v143 = 14.44** (the VS2022-era compatibility toolset still shipped on the VS2026 image — the exact compiler that produced the last green CI `d613614`) via `ilammy/msvc-dev-cmd@v1` (`toolset: '14.44'`) on ALL 3 Windows jobs (clippy, build-and-test, real-model-smoke). This fixes BOTH the DuckDB fmt AND the ggml break at the root (cc-rs + Ninja/CMake both pick up the pinned `cl.exe`/INCLUDE/LIB). Shim file deleted. **The thing to remove later is now the toolset pin** — drop it once `libduckdb-sys` vendors a newer fmt AND `llama-cpp-sys-2` supports VS2026. Files: `.github/workflows/ci.yml` (header comment + the 3 `Pin MSVC toolset to v143` steps). **Residual risk (CI-only-validated):** if cc-rs ignores the vcvars env and re-derives VS2026 via vswhere for the DuckDB build, the DuckDB compile could still pick 14.51 — watch the first CI run's libduckdb-sys log; if so, add a cc-rs-specific toolset hint.

9. **🅿️ Graph READ-CHANNEL parked — revisit only on proven beta demand (PARKED 2026-06-29, session 18).** The knowledge graph is built, self-cleaning, and encrypted at rest (ADR-SEC-002 Parts 1 & 2 — all committed + CI-green). Wiring it into the *answer* path (the `GraphRetriever` recall channel, ADR-SEC-002 Part 2 + Amendment 1) is the parked piece. **Verdict from the hard 40-distractor dogfood (2026-06-29):** the pool-truncation bug fix works (graph hits now reach the reranker — unit guard `graph_hit_survives_a_full_candidate_pool` green), but graph-ON == graph-OFF byte-identical on the 2-hop probe — **no win**. Three independent root causes, none a quick fix: (a) the cross-encoder ranks lexical "is known for" lookalikes above the true "specializes in" answer; (b) **per-fact enrichment creates duplicate entity nodes** (two "St. Mary's Hospital" etc.) that break the multi-hop chain → needs real **entity resolution** (a substantial arc); (c) extraction misses edges (e.g. `Patel→founded→Helixon`). **Why parked, not pursued:** the agent can already multi-hop via follow-up queries ([[project_architectural_lock_llm_out_of_read_path]] — vault returns facts, agent composes), so word-mismatched multi-hop is not a beta blocker. **Revisit trigger:** real agent/beta dogfood showing users genuinely need word-mismatched multi-hop answers the agent can't compose itself. If that demand never shows, **drop the read-channel** (the extracted+encrypted graph can still serve other uses). The read-channel must NOT ship ON meanwhile (traverse cost, no proven benefit). **RESOLVED session 19 (option A — banked OFF):** the proven pool-truncation fix + its regression guard ship active in `reranked_retriever.rs` (recall-channels-first pool assembly — a general win that protects the semantic channel too); the graph channel ships **OFF by default** in `application.rs`, with the tested 2-hop mechanism (`strategies/graph.rs`) + the `graph_readpath_dogfood.rs` harness preserved behind opt-in `VAULT_ENABLE_GRAPH_CHANNEL=1` for a future revisit. Full design record: ADR-SEC-002 Part 2 + Amendment 1 (now in `HANDOFF_V0.2_PART3_ARCHIVE.md`). Supersedes the old tech-debt #2 read-path tripwire (the tripwire did its job — extraction completeness was the right gate, and the dogfood is the measurement it called for).

Also tracked as SHIPPED-design-record in PART2 archive: `bulk_upsert` promotion to the `VectorStore` trait (730× faster bulk insert, shipped `c091281`).

---

## 8.5 · 🆕 ADR-073 (IN FLIGHT) — recall-safe `memory_read`: reorder-only + separation/no-signal abstain hint, never hard-empty

**Status:** SHIPPED 2026-06-08 (committing; CI pending). All 5 DoD gates green (fmt/build-0-warn/clippy-0-lint/vault-retrieval 80+6 tests/vault-mcp 41 tests). **Live-verified on the 1k vault across BOTH model tiers** (Flash + Opus): "how do i stay fit" now ANSWERS via `memory_read` (was `abstain:true` empty); blood-type/OS/salary still abstain with no fabrication; cat→dog surfaces the dog helpfully. Fixes Thread-2 Gap 1 (§4). Amends ADR-054 (read response shape, additive) + ADR-066 (recall-first read) + supersedes the ADR-059 read-side floor-drop. Full text stays here until the next archive freeze. (Gap 2 still open — see §4.)

**Context.** 1k live dogfood proved `memory_read` false-abstains on stored facts: `apply_reranker` (`structured_read_pipeline.rs`) hard-drops every candidate below `reranker.relevance_floor()` (≈ logit −2.5) and sets `abstain = candidates.is_empty()`. Real answers score below that floor ("runs 10km" for "stay fit" = logit −3.21) → dropped → false-abstain. Meanwhile `memory_search` (`RerankedRetriever`) is reorder-only + never empties and got these right. The two paths diverged; read must converge to search's recall-safe behavior. (Evidence: [[project_1k_live_read_false_abstain]].)

**Decision.**
1. **`apply_reranker` becomes reorder-only** — mirror `RerankedRetriever::rerank_pool`: sigmoid-map each logit to `[0,1]`, keep ALL candidates, sort by relevance DESC. No floor-drop. (The `RERANK_CANDIDATE_CAP` truncation stays — it bounds reranker cost, doesn't hide answers.)
2. **`abstain` is computed by a combined hint, not a drop** — `abstain = candidates.is_empty() || weak_match`, where `weak_match` is TRUE when EITHER (a) `top_relevance < READ_NO_SIGNAL_FLOOR` (≈0.01; catches lone/few no-signal facts the Lisbon-guard + cat→dog class), OR (b) the top is not separated from the pool per `search_hint`'s rule (top < `STRONG_RELEVANCE` 0.5 AND top < `SEPARATION_RATIO` 3× the runner-up; catches flat clusters like the salary trap). Separation alone is insufficient (a lone no-signal fact reads as "separated") — hence the floor; the floor alone is insufficient (the 0.025 salary cluster clears it) — hence separation. Both, combined.
3. **Never hard-empty `relevant_facts`** — when retrieval returned candidates, they are ALWAYS returned (reordered, truncated to `max_candidates`), even when `abstain=true`. The floor governs only the abstain HINT, never whether a fact is shown — so a mis-set floor can never hide a real answer (recall-safety by construction; the cardinal rule holds regardless of floor placement). The agent judges; the cat→dog live case proved a capable agent abstains-in-prose correctly when given facts + an honest hint.
4. **Response gains `top_relevance: f32`** (the rank-1 relevance for agent transparency). `abstain` IS read's weak-match signal — refined from "facts empty" to "no confident match; the facts shown (if any) are low-confidence." (No separate `weak_match` field — it would be identical to `abstain`; `top_relevance` carries the nuance, mirroring `memory_search`'s hint.)

**Thresholds (all backed by the 1k live data, none guessed):** real answers ranged relevance 0.0388–0.99; no-signal/wrong-neighbor 0.00006–0.004; salary distractor cluster 0.014–0.025 flat. `READ_NO_SIGNAL_FLOOR = 0.01` sits in the ~10× gap between the lowest real answer (0.0388) and the highest no-signal (0.004); `STRONG_RELEVANCE`/`SEPARATION_RATIO` reuse `search_hint`'s pinned 0.5 / 3×.

**Not chosen:** (a) lowering the existing floor-drop threshold — still drops real answers below it, still hard-false-abstains; rejected because recall must be unconditional. (b) Pure `search_hint` separation (no floor) — false-ANSWERS on lone no-signal facts (Lisbon-guard / cat→dog). (c) A redundant `weak_match` field — identical to `abstain`. (d) Touching the no-reranker cosine-gate fallback — out of scope; that path's `Vec::new()`-on-below-floor still maps to `abstain=true`+empty, unchanged.

**Security:** no crypto/boundary-filter change; boundary authorization upstream is untouched.

**Harness note:** `scale_eval` greenwashed this (measured pool recall, not the live `memory_read` gate, with favorable phrasings). Part of this work updates the harness to call the real tool + assert on `abstain`/`relevant_facts`/`top_relevance` + add paraphrase query variants.

**Tests changed (contract change — surfaced, not silent):** `reranker_filters_candidates_below_floor` (now: both kept, keep-ranked-first, reorder-only), `reranker_abstains_when_all_candidates_below_floor` (now: abstain=true BUT fact still present + low top_relevance), `union_semantic_recall_rescues_keyword_starved_fact` (now: distractor also kept, cello ranked #1, abstain=false). New tests: stay-fit-class (low-but-separated → abstain=false + fact present), salary-class (flat cluster → abstain=true + facts present), no-signal-floor (lone deep-negative → abstain=true + fact present), top_relevance field population.

---

## 8.6 · 🆕 ADR-074 (IN FLIGHT) — document-side alias enrichment at consolidation (Gap-2 vocabulary-gap fix)

**Status:** IMPLEMENTED 2026-06-10; **all 5 DoD gates green** (fmt / build-0-warn / clippy-0-lint / `vault-consolidator` 113 tests incl. 13 new `enrich` / `vault-app` 58 tests). NOT yet committed; live rank-lift validation with the real Phi-4 model is the next step. Fixes Thread-2 Gap 2 (§4 / §4.2). Honours [[project_architectural_lock_llm_out_of_read_path]] (Phi-4 at consolidation only) + [[project_locked_next_arc_t03x]] (consolidator arc). Full text stays here until the next archive freeze.

**Context.** Ground-truth probing of the real `seeded-vault-1k` (2026-06-09) re-diagnosed Gap 2 as a **vocabulary gap**: a fact phrased without the obvious keyword ("settled in **Porto**", "raising **twins**") is outranked by — or in a dense-distractor field drops below — facts carrying the literal keyword. The agent's keyword-soup query is the *trigger*, so query-side expansion was FALSIFIED (it replicates the harmful soup). The `probe_enrichment` A/B proved the fix is **document-side**: bare Porto ABSENT → enriched Porto rank 1 @ 0.9965 on the killer query; twins rank 5 → 1; no regression. Evidence: [[project_1k_live_paraphrase_recall_miss]].

**Decision — Option B (Phi-4 at consolidation), NOT write-time agent aliases (Option A):** the proven miss is an *existing* fact (write-time aliases only help future writes), and write-time leans on agent-generated aliases (the lever this session proved unreliable). Three locked parts:

1. **Alias generation (`phases/enrich.rs::generate_aliases`).** One `LlmProvider::complete_json` call per fact — mirrors `topics::label_one_cluster` (temp 0, fixed seed `0x0A11_A5E5`, `max_tokens` 64, JSON schema `{"aliases":[4..8 strings]}`, JSON-only system prompt). Asks for alternative search keywords NOT already prominent in the text (synonym / category / type). Output normalised to trimmed lowercase, de-blanked. Empty/malformed → `Err` (skip-and-retry, never run-abort).
2. **Storage + embed-text (`metadata` key + composition).** Aliases stored on `Memory.metadata.enrichment = {"aliases": "a, b, c", "content_fp": "<fnv1a-hex>"}` (no schema migration — `metadata` is free-form `serde_json`; existing keys preserved). The **embedded** text is `compose_embed_text` = `"<content> Topics: <aliases>"` (the proven probe shape, pinned by `compose_embed_text_matches_probe_shape`). **`Memory.content` (display text) is NEVER modified** → the alias line cannot leak into the read response (read returns `content`). Aliases are a **vector-channel boost only**; BM25 still indexes clean `content`. The persisted vector is replaced in-place via `StorageBackend::update_memory` (atomic metadata + vector update, by id).
3. **When it runs / cost (`Consolidator::enrich_facts`).** A new consolidator step over the active (non-superseded, non-invalidated) set, wired in the app-layer safety wrapper AFTER `run_consolidation` and BEFORE `generate_reports` (parallels `generate_reports`; under the same 30-min timeout). **Idempotent:** each fact records an FNV-1a `content_fp`; a fact already enriched for its current content is skipped, so the first run backfills the whole vault and steady-state runs only re-embed newly-written / changed facts (a merge or update mints fresh content → fresh fingerprint → re-enrich). FNV-1a (not `DefaultHasher`) is stable across toolchain versions → no spurious whole-vault re-embed after a Rust upgrade.

**Failure + operational semantics (locked-next-arc Step 4):** a per-fact LLM / embed / `update_memory` failure is logged-and-counted (`EnrichmentReport::facts_failed`) and the loop continues — one bad fact never aborts the run, and the fact retries next cycle (no fingerprint written). Two operational notes from tracing the real path: (a) **first backfill on a large vault can exceed the 30-min consolidator timeout** (~1k facts × ~3–5s/Phi-4 call); because each `update_memory` commits immediately and the pass is idempotent, a timed-out run still makes durable progress and **re-running resumes** (self-heals over 2–3 runs — no per-run cap added; alpha-scale vaults of a few hundred facts finish in one run). (b) **Re-embeds drain async** through the cascade queue (like merges today), so the one-shot `vault-cli consolidate run` exits before the new vectors land; they apply when a worker next opens the vault (restart Antigravity / MCP server).

**Not chosen:** (a) write-time agent aliases (Option A — doesn't fix existing facts; relies on the unreliable agent-alias lever). (b) Vault-side query expansion (FALSIFIED — IS the keyword-soup that triggers the miss). (c) A deterministic synonym map ("settled in Porto" → "home" needs comprehension, not a thesaurus). (d) Putting aliases into `content` (would leak into display + pollute BM25). (e) A per-run enrichment cap (YAGNI at alpha scale; timeout-resume already bounds risk — revisit if the live 1k run shows timeout pain).

**Security:** no crypto / boundary-filter change. Enrichment operates within a single boundary's facts via the existing storage traits; the alias text is derived from the fact's own content by the local Phi-4 (no cross-boundary read, no network).

**Live validation — DONE 2026-06-10, real Phi-4, real 1k vault.** Two `#[ignore]` probes ride with this commit: `vault-consolidator` `real_phi4_alias_quality` (loads the real GGUF, prints aliases for the killer facts) and `vault-app` `scale_eval::probe_real_enrichment_1k` (drops the 3 keyword-poor killers into a throwaway `seeded-vault-1k` copy, records bare rank, enriches ONLY them via the real `enrich_one` path, re-measures by direct LanceDB vector search — fast A/B, no full-vault enrichment / merge-cost). **Result (real Phi-4 aliases, 1k dense field):**

| killer | killer query | bare | → enriched |
|---|---|---|---|
| Porto ("settled in Porto") | "home location city country lives residence" | **ABSENT (>top-50)** | **rank 1** |
| twins ("raising twins") | "children kids son daughter offspring family" | rank 1 | rank 1 |
| hives ("comes out in hives") | "is the user allergic to anything" | rank 4 | **rank 1** |

**Prompt-tuning finding (the reason to validate-before-commit):** the *first* real-Phi-4 run lifted Porto only ABSENT→rank 6 — Phi-4 returned Portugal-anchored *phrases* (`portugal residence change`) instead of the generic single words the query uses. Tuned `generate_aliases` to ask for **single-word generic category/type keywords** (neutral job/pet examples, NOT the eval cases) → Porto's aliases became `portugal, settlement, residence, city, relocation, migration` → **rank 1**, hives/twins unchanged. All three killers now #1 end-to-end. Run cmd: `$env:PROBE_VAULT_DIR=<throwaway 1k copy>; $env:PHI4_MODEL_DIR=<models dir>; cargo test -p vault-app --test scale_eval probe_real_enrichment_1k -- --ignored --nocapture`.

---

## 8.7 · 🆕 ADR-075 (IN FLIGHT) — Phase 4 confidence decay (T0.2.4)

**Status:** SHIPPED 2026-06-13; all 4 DoD gates green (fresh DuckDB-1.4 build 0-warn / `vault-storage` + `vault-consolidator` tests 0-fail / clippy 0-lint / fmt). Implements BRD §5.6 Phase 4 line 994 (the *decay* half; cold archive deferred). Honours [[project_architectural_lock_llm_out_of_read_path]] (no LLM in decay).

**Context.** Phase 4 was unbuilt (`memories_archived` hardcoded 0; no decay pass). The sleep consolidator must fade stale knowledge so retrieval (which weights by confidence) demotes it over time without ever deleting it.

**Decision.**
1. **Policy (`phases/decay.rs::plan_decay`)** — a fact not accessed in `decay_after_days` (180) has `confidence ×= 0.9` (BRD line 994 verbatim). Pure planner over the active set; skips superseded / invalidated facts and 0.0-confidence no-ops.
2. **Metadata-only application (`StorageBackend::apply_decay`)** — sets confidence + an idempotency marker (`metadata.decay.last_decay_at`); **never re-embeds** (re-embedding from raw `content` would clobber the ADR-074 enriched vector). New `memory.decayed` audit event distinguishes a decay from a user edit.
3. **Idempotency (BRD line 1022)** — the marker means a back-to-back run does not re-decay; a fact re-decays only after a full decay period elapses.
4. **Wiring** — runs as Phase 4 in `run_consolidation` (after contradiction, before report); `ConsolidationReport.memories_decayed` + the summary Decay section carry the count.

**Cold archive (BRD lines 995-996) DEFERRED** — a first-class `Memory` state change (schema + retrieval-filter reach) far larger than decay; its own batch keeps this one debuggable. `memories_archived` stays 0.

**Tests:** 10 planner + 3 `apply_decay` + 2 summary + 1 real-BGE end-to-end (`cold_fact_decays_through_consolidation_and_is_never_lost`). The "no memory ever lost" property holds — decay only mutates confidence.

---

## 8.8 · 🆕 ADR-076 (IN FLIGHT) — sync ship-gate: `pending_sync` cascade payload (migration 0003)

**Status:** SHIPPED 2026-06-13; 4 gates green. **Closes tech-debt #4** (V0.2 sync ship-gate).

**Context.** `DivergenceDetector::sweep_pending_sync` was a V0.1 stub returning 0 — the cap-overflow catch-up table carried only `(memory_id, operation, queued_at)`, not enough to reconstruct a `retry_queue` row. Cross-device churn (V0.2 sync) makes a silently-dropped overflow entry a real data-recovery gap.

**Decision.**
1. **Migration 0003** adds `sequence_id INTEGER` + `payload BLOB` to `pending_sync` (nullable / defaulted — legacy rows read NULL payload and are *skipped*, never re-enqueued broken).
2. **Overflow path persists the full cascade** — both overflow call sites pass the in-scope `audit_seq` + `payload_bytes` to `tx_upsert_pending_sync`.
3. **Real sweep (`StorageBackend::drain_pending_sync`)** — oldest-first, atomically per entry: while `retry_queue` < cap, re-insert the stored cascade + delete the pending row in one tx. Stops at cap; payload-less rows skipped. `DivergenceDetector` calls it as Tier-0.

**Deviation from the handoff sketch:** stored the cascade **payload (+ `sequence_id`)** rather than separate `embedding`/`boundary` columns — more faithful (the stored bytes hand straight back to the retry insert) and schema-version-agnostic.

**Security:** payload lives in the SQLCipher-encrypted `vault.db` — encrypted at rest, no new plaintext surface, no crypto-path change.

**Tests:** full overflow → drop-vector → sweep → worker-reapply → vector-restored loop; payload-less legacy skip; payload round-trip; migration-columns check.

---

## 8.9 · 🆕 ADR-077 (IN FLIGHT) — DuckDB 1.2.2 → 1.10503.1 (libduckdb 1.4 LTS) upgrade

**Status:** SHIPPED 2026-06-13; 4 gates green on a **fresh full-workspace cold build** (`cargo clean` first).

**Context.** DuckDB 1.4 LTS (Sept 2025) adds native database encryption (`ATTACH … (ENCRYPTION_KEY …)`, AES-256-GCM over the main file + WAL + temp files) — the clean path to closing the V0.2 graph-encryption gap (`graph_store.rs:41-42`), which pinned 1.2.2 could not do.

**Decision.** Adopt the dependency upgrade **now** (de-risked on a clean rebuild of the whole workspace), but **DEFER the encryption wiring** (the `ATTACH ENCRYPTION_KEY` in `graph_store.rs` + ADR-SEC + §11 threat-model review + security tests) to its own task. Lands the heavy/risky dep bump on a verified clean tree so the later encryption work is pure code, not a dep gamble.

**Verification.** Spike built `vault-storage` clean (17m36s, exit 0). Then a full `cargo clean` + fresh `cargo build --workspace -D warnings` compiled **all 12 crates** against 1.4 (29m57s, 0 warnings); tests + clippy green.

**Cost accepted (`Cargo.lock` churn):** arrow 54→58 (workspace now carries arrow 57 **and** 58 — lance stays on 57; they don't cross paths), strum 0.25→0.27, + new crossterm / zip / zopfli / zlib-rs. The Cargo.toml CRT-conflict note (esaxx-rs `/MT` vs duckdb-sys `/MD`) is unaffected — `esaxx_fast` is already dropped.

**Next task (graph encryption — still deferred):** wire `ATTACH 'graph.duckdb' (ENCRYPTION_KEY <derived from master key>)` + ADR-SEC entry + §11 threat-model walk + security tests.

---

## 8.10 · 🆕 ADR-078 (IN FLIGHT) — graph-filling: entity + relationship extraction at consolidation

**Status:** SHIPPED 2026-06-14; all DoD gates green on a fresh `cargo clean` full-workspace rebuild (DuckDB 1.4.4). **Closes tech-debt #2** (entity-extraction-at-consolidation). Honours [[project_architectural_lock_llm_out_of_read_path]] (Phi-4 at consolidation only) + [[project_locked_next_arc_t03x]] (consolidator arc). Full text stays here until the next archive freeze.

**Corrects ADR-077 (§8.9):** that ADR's "libduckdb 1.4 LTS" label was WRONG — `=1.10503.1` is **DuckDB 1.5.3** (off-LTS; its bundled C++ fails the Windows CI `fmt/format.h` compile → `a1c0ff9` is CI-RED). Pin corrected to `=1.4.4` (the real LTS). ADR-077's encryption goal is **falsified by spike**: NO bundled DuckDB version can securely write an encrypted DB offline (mbedtls is read-only; secure write needs the network `httpfs`/OpenSSL extension → breaks offline/zero-knowledge — confirmed on 1.4.4 AND 1.5.3). Graph encryption deferred to "bundle the helper locally, when the graph holds shippable data" (tech-debt #7). A `rstrtmgr` link fix (`vault-storage/build.rs`) covers DuckDB 1.4's `AdditionalLockInfo` → Windows Restart-Manager dependency that `libduckdb-sys` forgot to link.

**Context.** The DuckDB `GraphStore` (entities + bi-temporal relationships) shipped at T0.1.5 but nothing ever FILLED it — `apply_merge` skipped the graph with a `tracing::warn!` no-op (tech-debt #2), so `graph.duckdb` held zero data. Product reason to fill it now (Shahbaz, 2026-06-14): the graph must hold real data before it (and its eventual encryption) is worth anything; "it's empty so don't encrypt it" is unbuilt work, not a feature.

**Decision — extract via the EXISTING enrichment call, not a new pass.** The nightly enrichment (ADR-074) already sends every fact to Phi-4 once (for search aliases). A separate extraction pass would DOUBLE the per-fact LLM cost and worsen the ~90-fact latency wall. Instead the one call now returns three products: `aliases` + `entities` + `relationships`. **Validated by a live tuned Phi-4 probe** (`phases::enrich::real_phi4_combined_extraction_quality`, `#[ignore]`): combined output keeps single-word keyword quality (no recall regression) and produces correctly-typed entities + sensibly-directed links. Three parts:

1. **Combined call (`phases/enrich.rs`).** `generate_aliases` → `generate_enrichment` returning `{aliases, graph}`; one `complete_json` against a schema carrying all three arrays (entity `type` enum = `EntityType` snake_case names). Aliases stay recall-critical (empty aliases ⇒ `Err`/retry); the graph is best-effort (empty ⇒ no error). `EnrichedFact` gains a `graph: ExtractedGraph` field.
2. **Parse + cleanup + write (NEW `phases/extract.rs`).** `parse_extracted` is best-effort (NEVER errors): maps the type label (unknown ⇒ `Concept`, never `Custom` junk), drops empty/over-long names, dedups entities, normalises relations to snake_case, and **drops any relationship whose endpoints are not in the entity list** (the model occasionally references an unlisted name). `write_extracted_to_graph` **gets-or-creates** each entity (new `GraphStore::get_entity` lookup) so nightly re-runs reuse ids instead of hitting the `(name, type, boundary)` UNIQUE constraint, then creates the relationships — all scoped to the memory's own `Boundary` (ADR-015 privacy holds).
3. **Wiring (`consolidator.rs::enrich_facts`).** After `update_memory` persists the enriched vector, the graph is written. **Ordering is load-bearing:** vector first (writes the `content_fp` fingerprint), graph second — so a transient graph-write failure is never re-extracted into DUPLICATE edges on the next run. `EnrichmentReport` gains `entities_created` / `entities_reused` / `relationships_created` / `relationships_failed` / `graph_write_failures`.

**Idempotency.** Extraction rides inside the fingerprint-gated `enrich_one`, so a steady-state run never re-extracts an unchanged fact → no duplicate entities/relationships (proven by the `enrich_facts_fills_graph_with_entities_and_relationships` e2e: fact → linked entities, traversable, second run = zero duplicates).

**Not chosen / deferred:** (a) a separate extraction LLM pass (doubles latency); (b) a local NER model (no NER lib in-tree; Phi-4 already loaded at consolidation); (c) **relationship-rewrite-on-merge** — a content change (merge/update) re-extracts but leaves the prior content's relationships behind; retiring them needs the `rewrite_relationships_for_memory` primitive (tech-debt #2's tail) — out of scope for this milestone, harmless while the graph is dogfood-only; (d) graph encryption (deferred — see above).

**Security:** no crypto / boundary-filter change. Extraction operates within a single boundary's facts via the existing `GraphStore` traits; entity/relationship text is derived from the fact's own content by the local Phi-4 (no cross-boundary read, no network). `create_relationship`'s ADR-015 cross-boundary guard is untouched.

**Tests:** `vault-storage` `get_entity` ×4 (absent / full-fidelity / type+boundary scoping / get-or-create no-dup); `vault-consolidator` `phases::extract` ×8 (label mapping, relation normalisation, dangling-drop, case-insensitive endpoint resolve, dedup + self-loop drop, malformed-safe) + the `enrich_facts` e2e graph-fill + existing enrichment tests green (no regression).

---

## 8.11 · 🆕 ADR-079 (IN FLIGHT) — Windows CI fix: VS2026 removed `stdext::checked_array_iterator` (bundled-DuckDB fmt break)

**Status:** committing now; CI-only change, NOT locally testable (see below). Restores `main` to green after two consecutive RED commits (`a1c0ff9`, `d2b9b9b`). Corrects the ADR-078/§1 misdiagnosis that the DuckDB pin caused the Windows red.

**Root cause (proven from CI run `27484651556` logs + cross-checked upstream).** GitHub's `windows-2025` runner image migrated to **Visual Studio 2026 (MSVC 14.51.36231)** during the 2026-06-08→06-15 rollout (the build log path is `Microsoft Visual Studio\18\Enterprise`). VS 2026 **removed** `stdext::checked_array_iterator` from the MSVC STL headers entirely (a long-deprecated non-Standard extension; confirmed removed, not merely deprecated — see o3de/o3de#19754: *"these functions literally do not exist anymore"*). DuckDB's bundled `fmt` (~v5.x, vendored in `libduckdb-sys`) still references it under a bare `#ifdef _SECURE_SCL`; VS 2026 **still defines** `_SECURE_SCL`, so the bundled C++ build takes that branch and fails:

```text
fmt/format.h(326): error C2061: syntax error: identifier 'checked_array_iterator'
```

This is independent of DuckDB version — `1.4.4` AND `1.5.3` bundle the same ancient fmt, so neither the `=1.10503.1→=1.4.4` correction nor any crate bump escapes it. The last green commit (`d613614`, 2026-06-10) predates the image migration; nothing in our code regressed. `_SILENCE_STDEXT_ARR_ITERS_DEPRECATION_WARNING` does NOT help (the type is gone, not deprecated; and the build already uses `-W0`).

**Decision.** A forced-include (`/FI`) shim header (`.github/msvc_fmt_secure_scl_shim.h`) `#include`s `<yvals.h>` (which sets `_SECURE_SCL` and has an include guard) then `#undef _SECURE_SCL`; later STL includes are guard-no-ops, so the macro stays undefined and fmt falls back to its raw-pointer `checked_ptr = T*` branch — the exact path Linux/macOS already compile (known-good; DuckDB builds clean there). Wired into BOTH Windows CI jobs (clippy + build/test) via `CXXFLAGS_x86_64_pc_windows_msvc`, which **cc-rs (libduckdb-sys) reads but CMake (llama-cpp-sys-2's Vulkan build) does not** — so the llama/Vulkan build, the reason we are on `windows-2025` at all, is untouched.

**Not chosen:** (a) the silence macro (type removed, not deprecated); (b) a DuckDB crate bump (same bundled fmt across versions); (c) reverting to `windows-2022` (re-breaks the llama `vulkan-shaders-gen` C1083 build — the documented reason for `windows-2025`); (d) pinning an older MSVC v143 toolset (re-introduces toolset/CMake interaction risk, larger blast radius); (e) hand-writing a `checked_array_iterator` replacement (error-prone vs. just disabling the dead branch).

**Local-test relaxation (per Shahbaz, 2026-06-14 session 2).** The failure is specific to the CI runner's VS 2026 image and **cannot be reproduced on the founder's local machine** (older MSVC that still ships the type — local builds were green throughout). So local DoD gates verify nothing here; CI is the only meaningful verification. Committed + pushed without a local build run by explicit founder direction; CI-green is the gate. **Risk if wrong:** the `<yvals.h>`-defines-`_SECURE_SCL` assumption is the one empirical link not provable locally — if a different header defines it, CI fails the same way and we iterate.

**Security:** none — build-time compiler flag only, no runtime/crypto/boundary surface.

**Tech debt:** remove the shim + CI step once `libduckdb-sys` vendors a newer fmt (or drops the `stdext` usage). Tracked as tech-debt #8.

---

## 8.12 · 🆕 ADR-080 (IN FLIGHT) — consolidator scheduling (T0.2.6): production scheduler is app-layer

**Status:** BUILT 2026-06-14; all 5 DoD gates green locally on a fresh cold build. UNCOMMITTED pending end-to-end + dogfood test (§1 opener). Implements BRD §5.6 line 953 (`Consolidator::schedule`) + the `scheduler.rs` slot + the §6 "Scheduling — Not built" gap. Honours [[project_architectural_lock_llm_out_of_read_path]] (Phi-4 at consolidation only).

**Context.** `Consolidator::schedule()` was a `todo!()` panic stub and nothing triggered consolidation automatically — the nightly brain only ran when manually invoked. T0.2.6 makes the vault self-maintaining.

**Decision — the production scheduler lives in `vault-app`, not `Consolidator::schedule()`.** The dependency rule (app → consolidator, never upward) forces this: the full *correct* nightly pipeline needs the app-only cross-process lockfile, the 30-min timeout, the ADR-074 enrichment pass, and per-boundary REPORT-to-disk — and the consolidator is filesystem-agnostic by architecture lock, so it cannot call `Application::run_consolidation_with_safety`. If `schedule()` were the trigger it would silently skip enrichment + REPORTs → incorrect output. So:

1. **`vault-consolidator/src/scheduler.rs` (NEW)** — pure timing: `next_run_after(now, run_at)` (strict-after, so firing exactly at `run_at` schedules tomorrow, never an immediate re-fire) + `duration_until_next_run(now: DateTime<Local>, run_at)`. No async / no clock side-effects → exhaustively unit-testable (7 tests: today/tomorrow/exact-match/one-second-before/month+year rollover/positive-and-bounded/delta-match). BRD §5.6 `run_at` is local time, so arithmetic is local (one-night DST slop accepted at alpha scale).
2. **`Consolidator::schedule()`** — implemented as the headless loop (sleep via the helper → `run_consolidation` → `enrich_facts`; a failed run is logged and the loop waits for the next `run_at`, never tears down). Infinite loop; the `VaultResult<()>` return matches the BRD signature. Documented as the library/embedder path; the app does not call it.
3. **App-layer production scheduler (`application.rs`)** — extracted `run_consolidation_under_safety(consolidator, vault_root)` from the existing method (both the method and the scheduler now call it); a shutdown-aware `run_consolidator_schedule` loop mirrors the proven `RetryWorker::run` pattern (`select!` on `sleep(wait)` vs `cancel.changed()` so Ctrl-C is prompt); spawned in `start_with_mcp` **only when a consolidator is configured**, tracked on `ApplicationHandle.consolidator_handle`, aborted + awaited in `shutdown()`.

**Latency explicitly out of scope (Shahbaz, 2026-06-14).** Correctness of wiring first; the 30-min budget / incremental-phase work is deferred. A scheduled full run on the ~90-fact dev vault may exceed 30 min today — acceptable for now (idempotent passes self-heal; the timeout is a safety guard, not a correctness gate). Revisit latency after the core is proven correct via dogfood.

**Not chosen:** (a) the loop in `Consolidator::schedule()` as the production path (skips enrichment + REPORT — incorrect output); (b) a callback-param on `schedule()` (would diverge from the BRD signature); (c) an external cron (thesis violation — BRD §1.4 "we do not host scheduled cron jobs"; the in-process tokio scheduler is the local-first equivalent); (d) chasing the 30-min budget now (latency deferred).

**Security:** none — scheduling is timing only; the run it triggers reuses the unchanged `run_consolidation_with_safety` path (lockfile + boundary-scoped storage traits).

**Tests:** `vault_consolidator::scheduler` ×7 (pure timing). The auto-scheduler *firing* has no unit test (a 24 h wait isn't testable) — validated live in the §1 STEP-1/STEP-3 dogfood instead, with reasoning recorded here rather than a brittle paused-clock integration test.

---

## 8.13 · 🆕 ADR-081 (IN FLIGHT) — Checkpoint & Rollback (T0.2.5): capture-by-diff, enrichment excluded, top-level CLI

**Status:** BUILT 2026-06-16; all 5 DoD gates green on a fresh `cargo clean` cold build. UNCOMMITTED at time of writing. Closes the T0.2.5 "undo a bad nightly run" gap (BRD §5.6 line 998 + §6.2). Full text stays here until the next archive freeze.

**Context.** The vault now self-runs the nightly consolidation (T0.2.6 scheduler, ADR-080). A bad run (an over-eager merge, a wrong contradiction call) would silently corrupt the user's memory with no recourse — the trust-critical gap for unattended beta. A2 records, every run, an undo-log of exactly what changed, restorable by id.

**Decision.**
1. **Storage layer owns the checkpoint store** (`vault-storage/src/checkpoint.rs`): `create_checkpoint` (insert + prune to N=7, one txn), `rollback_checkpoint` (load → restore 'modified' via the existing `update_memory`, delete 'created' via `delete_memory`, mark `rolled_back` — three separate txns so the metadata lock is never held across the cascading writes, which would deadlock), `list_checkpoints`. Pre-image = versioned `{Memory, embedding}` blob in the SQLCipher DB (inherits zero-knowledge encryption-at-rest). Tables: migration v4.
2. **Capture is a before/after DIFF** (`vault-consolidator/src/checkpoint.rs::diff_to_entries`), NOT per-mutation hooks. Justification: every `run_consolidation` mutation (merge-supersede, dedup, contradiction-`invalidate`, decay) is **metadata-only on an existing row**; the only insertions are new merged rows. So the complete change set = diff of a full-enumeration snapshot taken before vs after the run. This is robust (captures whatever changed regardless of phase), needs **zero changes to the mutation sites**, and is far less error-prone than threading a recorder through 6 call sites. The pre-image embedding is reconstructed EXACTLY (not fetched — the vector store has no get-by-id) via `enrich::stored_embed_text`: raw `content`, or `compose_embed_text(content, alias_line)` when the fact is enriched-for-current-content (the `alias_line` is persisted verbatim in `metadata.enrichment.aliases`, so re-embedding reproduces the stored vector byte-for-byte; deterministic embedder).
3. **Enrichment is EXCLUDED from rollback scope.** The separate `enrich_facts` pass is additive + content-preserving (it never touches `Memory.content`; it only adds recall aliases to `metadata` + re-embeds). Undoing it would merely strip a recall boost the next run re-adds — it is not destructive, so it need not be reverted. The destructive operations (merge/dedup/contradiction/decay) ARE all captured. (Corrects the original plan's "enriched rows are 'created'" wording — enrich updates in place, it does not create rows.)
4. **CLI is a top-level `vault-cli checkpoint {list,rollback <id>}`**, NOT under `consolidate`. Rollback/list are storage-only (no models); `consolidate` requires the `--bge-*`/`--phi4-model` flags. A top-level command needs none of them — mirrors the storage-only `dead-letter` / `divergence-check` commands.

**Founder-locked carryover (2026-06-15):** capture only-changed pre-images (scales to 10k); **DEFER graph (DuckDB) rollback** until the graph enters the read path (tech-debt #2 tripwire — graph is write-only in V0.2); retention **N=7**.

**Tests:** vault-storage ×8 unit (empty-reject, create→list, rollback modified/created/mixed exact, prune-to-N, unknown-id error, double-rollback error) + migration table-existence; vault-consolidator ×2 integration every-cycle (`rollback_restores_pre_consolidation_state_exactly`, `rollback_reverts_combined_dedup_and_decay` — real BGE + MockLlm, assert post == pre EXACTLY + no-memory-lost + double-rollback guard); vault-cli ×2 parse; summary footer test updated (real id + `vault-cli checkpoint rollback` hint, replacing the `pending-T0.2.5` placeholder).

**Not chosen / deferred:** (a) per-mutation capture hooks (fragile, 6 sites); (b) a `VectorStore::get_embedding` primitive (unnecessary — reconstruction is exact); (c) rolling back enrichment (additive, self-healing — see Decision 3); (d) graph rollback (deferred, tech-debt #2).

---

## 8.14 · 🆕 ADR-082 (IN FLIGHT) — incremental consolidation (Pillar 2 scale fix): seed by watermark, compare against the whole corpus

**Context.** The session-5 1k pressure-test (§1 scorecard) proved a full nightly run cannot complete on this hardware — every run re-processes the WHOLE vault (re-embed all → re-cluster → re-merge → re-contradiction → re-enrich → rebuild REPORT). BRD §5.6 line 936 ALREADY specifies incremental ("for each memory **added since last consolidation**"); the shipped `since: None` full-scan was the deviation, not a new design.

**Decision.** A run is scoped by a `since` watermark — `run_consolidation(since: Option<DateTime<Utc>>)`.
- **D1** Watermark storage = a dedicated single-row `consolidation_state` table (migration `0005`), NOT the checkpoint table (which isn't written for a no-op run, so it can't reliably advance).
- **D2** Watermark value = the run's **START** time (so a fact created mid-run is picked up next run, never skipped).
- **D3** Advance the watermark **only on full-pipeline success** (`run_consolidation` → `enrich_facts` → `generate_reports` → REPORT persist). A timed-out / crashed / errored run leaves it untouched → the next run retries the same backlog. No lost work.
- **D4 (the load-bearing invariant).** Changed facts are **seeds**; each seed is compared against the **whole active corpus**. Phase 1 enumerates seeds via `since` but validates neighbour edges against ALL active ids (not the seed set); Phase 2b searches LanceDB per seed (the whole boundary). So a new fact still merges / contradiction-checks against an OLD untouched fact. Getting this wrong silently loses merge/contradiction recall — the cardinal sin — so it is gated by **R1** (clustering, `tests/incremental_consolidation.rs`) and **R2** (contradiction, `tests/contradiction_resolution.rs`).
- **D5** `since = None` stays the full-sweep path (cold start / periodic deep-clean), behaviourally unchanged (the proven A5 in-memory all-pairs path is preserved).
- **D6** A watermark read failure **fails open to a full sweep** (a slow run beats a missed merge/contradiction).
- **D7** Retired lingering vectors (superseded/invalidated/deleted, whose LanceDB vector lingers) are dropped by validating neighbours against the active-id set.

**Scope SHIPPED (session 6, this commit) — Steps 1-3:** watermark (storage migration `0005` + `consolidation_state.rs`) + incremental Phase 1 (`cluster.rs`) + incremental Phase 2b (`candidates::contradiction_candidate_neighbours` + `consolidator.rs`) + app/headless watermark wiring + R1/R2 tests. This lets a 1k nightly run COMPLETE (merge/contradiction no longer fill the 30-min budget); the only O(N) cost left is REPORT topic-discovery's embed-all (~14 min, now fits) + the one-time enrich backfill.

**Deferred (named follow-ups, NOT in this commit):**
- **Step 4** — reuse stored vectors (new `vector_store` `get_by_id`) so REPORT topic-discovery stops re-embedding the corpus → extends the win to 10k.
- **Catch-up scheduling** — on app start, if the watermark is stale (> ~24h), run once then resume nightly (the "laptop asleep at 3 AM" fix).
- **Full-sweep CLI command** + a **configurable timeout** so the one-time cold-start backfill can complete (next session, STEP 1).
- **Enrich-cap** — chunk the first-ever backfill across nights (enrich is idempotent → converges).
- **Loosen the deterministic dedup gate** (0/102 dense-template clusters caught → all hit the LLM).

**Consequences.** Nightly cost → O(facts changed), not O(vault). Trade-off: content-EDITED facts keep their `created_at`, so a `created_at`-based `since` re-enriches them (fingerprint) but does NOT re-merge / re-contradiction-check them nightly — the periodic full sweep covers that (documented V0.2 limitation).

## 8.15 · 🆕 ADR-083 (IN FLIGHT) — contradiction over-retention guard: single-valued attributes vs distinct events

**Context.** The session-7 1k diagnostic (the two cosine-distribution probes, `scale_eval::probe_contradiction_pair_distribution`) falsified BOTH proposed "1,730-pair" speed fixes: the candidate floor is already 0.70 + top-K (so the pairs are not "unpruned"), and raising it past ~0.82 drops the real Tesla/Rivian contradiction (0.823); AND the merge/dedup gate is CORRECTLY not collapsing the ≥0.92 pairs because they are **distinct facts** (different person/date/place), not duplicates — loosening it would destroy real data. The pair count is largely an artifact of pathological synthetic distractor data (`generate_distractors` template-clones), not a product defect; the nightly incremental run (the real product) is unaffected. The ONE real correctness risk surfaced (Finding B) is the **contradiction judge over-retiring distinct-but-similar facts**: the prompt taught single-valued-attribute updates (employer/city/colour) but never the difference between *"changed my city"* (supersede) and *"two separate coffee meetings"* (both true). Over-retention is the one **unrescuable** failure — a retired fact is gone from the active set; no downstream agent can recover it (the read-path "trust the agent" model does NOT apply at consolidation, where no agent is in the loop).

**Decision.** Teach the pairwise judge the single-valued-vs-event distinction in the PROMPT (guide the model, do not add a deterministic gate — honours the "trust the LLM's judgment" lock):
- **D1** A contradiction requires the shared attribute be **SINGLE-VALUED** — one the subject holds only ONE current value of (employer, city of residence, marital status, favourite colour). A new value supersedes the old → retire older.
- **D2** Facts describing distinct **EVENTS / occurrences** a person accumulates many of (meetings, trips, purchases, deliveries, tasks, messages, recaps, sign-ups, sessions) are NOT contradictions even when worded near-identically: a difference in date/day/time/place/people is the signature of two distinct events → `shared_attribute=null, contradiction=false, stale='neither'` (the existing null-shared-attribute aggregator gate is the second safety net).
- **D3** Two few-shot examples added (coffee-Monday vs coffee-Thursday; two office recaps), plus the schema `shared_attribute` description tightened to "single-valued … null if … distinct events/occurrences".

**Posture — "keep when unsure" (founder-agreed 2026-06-20).** The real-Phi-4 verification proved the prompt alone fixes the CLEAR cases but Phi-4-mini wobbles on the genuinely-ambiguous middle (5/7 on the first run: coffee + recaps fixed ✅; Berlin→Lisbon + Vega→Atlas retire ✅; "Denver — Sam vs Aisha coordinating" reassign-or-two-sessions and "Tesla→Rivian" own-two-cars are model-ceiling cases). The decision: bias hard toward KEEPING — over-retention is the one unrescuable failure (a retired fact leaves default retrieval), whereas under-retention is agent-rescuable (the read path picks current truth by `as_of`). A wins-on-the-clear-cases prompt + the existing safety nets is the right altitude; do NOT force the model to make a retire it cannot reliably make.

**Bloat answer (the question "won't keeping cause bloat?").** Keeping distinct events does NOT mean unbounded growth: clear duplicates still MERGE and clear updates still RETIRE; only the ambiguous middle is kept. Accumulated true facts are managed by **demote-not-delete** — confidence **decay** (ADR-075, BUILT: stale facts sink in ranking) + **cold-archive** (A1, DESIGNED-not-built: facts untouched ~365d leave default retrieval) + the **reranker** (read is top-K + rerank, so vault size does not linearly degrade precision). **A1 cold-archive is the named bloat follow-up** and moves up the priority list as the structural anti-bloat tool. A stronger consolidation model (BYOK/Managed only — nightly is latency-tolerant) is a pocketed option for the fuzzy-but-resolvable cases; LOCAL mode stays at Phi-4-mini (hardware-capped).

**Recall-safety.** The change can ONLY convert "contradiction → retire" into "keep both" — it strictly REDUCES retirement, so it cannot newly lose a genuine update. Verified by the real-Phi-4 `#[ignore]` probe `real_phi4_distinct_events_not_retired` (the acceptance gate, three buckets): **clear events** (coffee, recaps, Paris trips) MUST keep both [hard assert]; **clear single-valued updates** (Berlin→Lisbon, Vega→Atlas) MUST retire the older [hard assert]; **genuinely ambiguous** (Denver coordinator, Tesla→Rivian) are informational-only [printed, not asserted — neither outcome is wrong]. A MockLlm test cannot prove a prompt change, so the real-model probe IS the verification ([[feedback_runtime_confirmation_after_web_spike]]).

**Scope.** Prompt + schema-description edit in `phases/contradiction.rs` (`CONTRADICTION_PAIR_SYSTEM_PROMPT` — single-valued-vs-event principle + examples 7/8/9 + the explicit "when in doubt, keep both" instruction — `CONTRADICTION_PAIR_SCHEMA` + module doc) + the real-Phi-4 acceptance probe. No aggregator/recency logic change (the Bug-1 recency stale-pick + the null-shared-attribute gate are untouched).

**NOT in scope (explicitly).** The full-sweep pair-count "speed" — judged a test-data artifact (real vaults are not template-dense) + a one-time backfill cost the incremental feature does not pay; deferred, not fixed. Finding E (a 100-fact contradiction that did not resolve) is the *under*-retention direction — same prompt area, tracked separately; this guard does not address it. The ambiguous-middle precision ceiling is accepted, not chased (whack-a-mole against a 3.8B model); the anti-bloat burden is carried by decay + A1 archive instead.

## 8.16 · 🆕 ADR-084 (IN FLIGHT) — A1 cold archive: soft `archived_at` state, out of default retrieval, reversible

**Context.** Cold archive is the named anti-bloat tool the ADR-083 "keep when unsure" posture leans on (BRD §5.6 lines 995-996 — the other half of Phase 4, decay being the first). With over-retention now the deliberate bias, accumulated true facts need a structural demote-not-delete path so the default retrieval pool does not grow unbounded. The plumbing was already half-stubbed: `ConsolidatorConfig.archive_after_days` (default 365) and `ConsolidationReport.memories_archived` existed but the count was hard-coded `0`, and the "no memory ever lost" property (BRD §5.6 line 1023) already names **archived** as a legal third end-state alongside active and superseded.

**Decision — soft state, not a separate encrypted store (founder-agreed 2026-06-20).** BRD §5.6 line 995 says "move to cold archive (encrypted blob, removed from active stores)". We implement the *intent* (out of default retrieval, searchable via an explicit call) with a soft marker, NOT the literal separate-blob store:
- **D1** New nullable `Memory.archived_at: Option<DateTime<Utc>>` (migration `0006`, column + partial index `WHERE archived_at IS NOT NULL`). `Some(t)` = cold-archived; `None` = active. A first-class state column mirroring `valid_until` / `superseded_by`, NOT a metadata-JSON hack — the property test treats archived as first-class and the consolidator filters it at SQL level. `#[serde(default)]` keeps pre-ADR-084 rollback pre-image blobs deserializable (no `CHECKPOINT_PAYLOAD_FORMAT_VERSION` bump).
- **D2** The fact stays in the already-SQLCipher-encrypted `vault.db`. A cold fact is equally unreadable to a server whether it sits in the main table with a marker or in a separate blob, so the zero-knowledge guarantee is unchanged and **no new crypto path is opened** (the separate-blob store would have — re-read §11, new key usage, new format). The separate store is a large-scale hot-index-shrink optimization deferred to V1.0+; we don't have that scale.
- **D3** Reversible by construction — archive never deletes; un-archiving is clearing the marker, and a bad nightly archive is undone by the existing checkpoint rollback (the pre-archive image restores `archived_at = None`).

**Retrieval.** Default retrieval already gated a "non-current" bucket via the single `include_archived` flag (superseded merge-losers + ADR-051 expired facts). Cold-archived facts join that same bucket — `retain(!superseded && !expired && !archived)` by default, all three surfaced when `include_archived = true` (the BRD's "explicit search archive call"). No new flag, no naming collision: `include_archived` now honestly means "include the whole archived/historical bucket". `MemoryFilter.include_archived` (default `false`) gates it at the SQL layer for the consolidator's active-set enumerations.

**Phase 4 archive pass.** New `phases/archive.rs` — pure `plan_archive(memories, archive_after_days, now)` selecting active, non-superseded, non-expired, **not-already-archived** facts idle past the threshold; applied by `Consolidator::archive_memories()` via the metadata-only `StorageBackend::apply_archive` (sets `archived_at`, emits one `memory.archived` audit event, preserves the ADR-074 enriched vector — no re-embed). Runs AFTER decay in the same Phase 4 (a fact past both thresholds is decayed AND archived this run; archive is the terminal cold state). Idempotent for free — the `archived_at` column IS the marker, and an archived fact is no longer in the `MemoryFilter::default()` active set the pass enumerates (no metadata marker needed, unlike decay). `memories_archived` and the summary "Archived: N" line now carry the real count.

**Checkpoint correctness.** The rollback diff is pre-vs-post; both snapshot reads now use `include_superseded + include_archived` so a fact this run archives (`archived_at` None → Some) is seen as **Modified** (captured for rollback), not Deleted. The run's active working set correspondingly excludes archived (`superseded_by.is_none() && !is_archived()`).

**Recall-safety / no-memory-lost.** Archive only moves facts OUT of *default* retrieval, never deletes — they remain in `vault.db` and surface via `include_archived`. The "no memory ever lost" property test (`properties.rs`) was upgraded from a two-way (active|superseded) to the full three-way partition (active|superseded|archived) per BRD §5.6 line 1023, reading post-state with both filters on.

**Scope.** `Memory.archived_at` + `is_archived()` (vault-core); migration 0006 + INSERT/UPDATE/3 SELECTs/row-decoder + `MemoryFilter.include_archived` + `apply_archive` + `memory.archived` audit event (vault-storage); `phases/archive.rs` + `archive_memories()` wiring + real `memories_archived` + summary (vault-consolidator); default-exclude filter (vault-retrieval); `include_archived` doc (vault-mcp). Tests: `plan_archive` units, `apply_archive` storage units, 2 retrieval tests (8e/8f), `archive_integration.rs` E2E (real BGE), three-state property partition, migration 0006 test.

**NOT in scope (explicitly).** The literal separate encrypted archive store (V1.0+ scale optimization, D2). A user-facing MCP "search archive" tool — the storage + retrieval plumbing supports it today (`include_archived: true`); exposing a dedicated MCP surface is a small follow-up. Removing archived vectors from the LanceDB hot index (they stay, filtered post-search — the index-shrink is the deferred separate-store win).

## 9 · 📇 ADR index

Full text of every ADR lives in an archive — cross-link by number, **quote don't paraphrase** ([[feedback_quote_locked_artefacts_dont_paraphrase]]).

**In-flight (full text in HANDOFF, not yet archived):** **ADR-084** (A1 cold archive — soft `archived_at` state in the encrypted vault.db, out of default retrieval, reversible, no new crypto path; Phase 4 second half; verified by `archive_integration` + three-state property partition; §8.16) · **ADR-083** (contradiction over-retention guard — single-valued attributes supersede, distinct events accumulate; prompt-taught, verified by `real_phi4_distinct_events_not_retired`; §8.15) · **ADR-082** (incremental consolidation — Pillar 2 scale fix: seed by `since` watermark, compare against the whole corpus; cross-corpus invariant gated by R1/R2; §8.14) · **ADR-081** (Checkpoint & Rollback T0.2.5 — capture-by-diff, enrichment excluded from rollback, top-level `vault-cli checkpoint` command, §8.13) · **ADR-080** (consolidator scheduling T0.2.6 — production scheduler is app-layer; pure `scheduler.rs` timing + `Consolidator::schedule()` headless loop, §8.12) · **ADR-079** (Windows CI fix: VS2026 removed `stdext::checked_array_iterator` → `/FI` `_SECURE_SCL`-undef shim for bundled-DuckDB fmt, §8.11 — corrects the ADR-078/§1 "1.4.4 fixes CI" misdiagnosis; shim is a dead end, revert + toolset-pin pending) · **ADR-078** (graph-filling: entity + relationship extraction at consolidation, §8.10 — closes tech-debt #2; corrects ADR-077 to DuckDB 1.4.4 + defers encryption) · **ADR-077** (DuckDB dep upgrade — corrected to `=1.4.4` LTS, §8.9) · **ADR-076** (sync ship-gate `pending_sync` payload, §8.8) · **ADR-075** (Phase 4 confidence decay, §8.7) · **ADR-074** (document-side alias enrichment at consolidation, §8.6) · **ADR-073** (recall-safe `memory_read`, §8.5 — SHIPPED `a3e426b`).

**Most relevant to current/next work (full text in `HANDOFF_V0.2_PART2_ARCHIVE.md`):**
| ADR | Title | Status |
|---|---|---|
| **072** | sealed-store `get_opts` never returns a short buffer for a bounded range (10k TOCTOU fix) | SHIPPED `da10c0f` |
| **071** | reranked + recall-safe `memory_search`; `memory_read` is the primary answer path | SHIPPED `661d391` (+ Option B `a1e4dac`) |
| **070** | lazy reranker load off the handshake path | SHIPPED `a3c938b` |
| **069** | read recall-union: hybrid ∪ semantic candidate pool | SHIPPED `a2cee13` |
| **068** | topic discovery by connected-components, not K-means | SHIPPED `76ffc9b` |
| **067** | `memory_search` recall-first: hybrid candidates, no hard BM25 gate | SHIPPED `76ffc9b` |
| **066** | recall-first read: reranker as re-orderer + no-signal floor, not precision authority | SHIPPED |
| **065** | contradiction candidate generation by nearest neighbor, not K-means topics | SHIPPED |
| **064** | read-side subject framing for the reranker (`DOC_SUBJECT_FRAME "The user — "`, Bug-2 fix) | SHIPPED |
| **061** | clustering robustness to vector-store / metadata divergence | SHIPPED |
| **060** | topic-level contradiction detection (A5 ship-gate) | SHIPPED |
| **059** | cross-encoder reranker (Qwen3-Reranker-0.6B) as the read relevance gate (supersedes ADR-057 cosine floor) | SHIPPED `87d0b72` |
| **058** | wire per-boundary REPORT generation into the consolidation run | SHIPPED |
| **057** | deterministic cosine relevance gate for `memory_read` | SUPERSEDED by ADR-059 |
| **056** | dogfood-surfaced correctness fixes (Commit 8) | SHIPPED |
| **055** | `vault-cli mcp serve` subcommand-split design | SHIPPED |
| **054** | MCP `memory.read` response health-warning contract (6 codes; Amendment 2 dropped `DELTA_LOG_UNAVAILABLE`) | SHIPPED `99052f2` |
| **053** | per-boundary REPORT artifact shape + storage + lifecycle (+ Amendment 1: `topic_names_unavailable`) | SHIPPED `f0cc158` |
| **052** | Qwen-7B retirement from read path (supersedes ADR-048/049 in effect) | SHIPPED `99052f2` |
| **051** | bi-temporal storage semantics + `invalidate()` API contract | SHIPPED |
| **047** | `summary.rs` placement + RunState/AMWC field extensions | SHIPPED |
| 048, 049 | Qwen-7B read pipeline + model lock | SUPERSEDED by ADR-052 |

**Live V0.2-era ADRs, full text in `HANDOFF_V0.2_PART1_ARCHIVE.md`:** ADR-044 (+Amendment 1, `LlmProvider`/`Phi4MiniProvider`), ADR-045 (Cluster output contract), ADR-046 (`mark_superseded` + `MemorySuperseded` audit), plus ADR-037–043 (lancedb upgrade, concurrent-upsert serialisation, Keychain/master-key derivation, V0.1→V0.2 SQLCipher bridge, Phi-4-mini selection, model download/integrity).

**V0.1-era ADRs (001–036):** full text in `HANDOFF_V0.1_ARCHIVE.md`.

---

## 10 · 📐 Standing rules (CLAUDE.md-promoted defaults)

Full rules in `~/.claude/projects/C--Projects-GitHub-Memory-Vault/memory/`.

- **Confirm before every commit + push.** One combined approval covers both; per-action (yes-commit ≠ yes-push for the *next* task). Co-Authored-By: bare `Claude <noreply@anthropic.com>`, no model qualifier.
- **CI green per-commit.** Every code commit shows CI green matrix-wide (`gh run list --workflow=ci.yml -L 1`) before staging the next. Local DoD ≠ CI green. Relaxation is the founder's to invoke per-batch, acknowledged in the commit body.
- **Confirm before any cargo build/test/clippy/check/run + check disk first** (laptop freezes during compile; disk runs tight). Report disk + target size in the ask. Only `cargo fmt` is safe. Run gates in background (`run_in_background=true`).
- **Strictly-serial cargo.** Never parallel cargo on the same workspace (kills incremental cache → 30GB+ wipe + 30-min rebuild). Order: check → test → clippy → fmt → `git status`.
- **Cargo on Windows = PowerShell** (Strawberry Perl path order for the sqlcipher/openssl vendoring; MSYS2 perl in Bash lacks the modules). Set `LIBCLANG_PATH` + prepend to PATH each fresh shell.
- **fmt runs LAST**, with `git status --short` between final `cargo fmt --all --check` and `git add` to catch drift (esp. `Cargo.lock`).
- **Admin-only changes ride with the next code commit** (HANDOFF/ADR/tech-debt/doc edits never get their own commit — saves a ~45-min CI cycle). Spike examples + eval harnesses + baselines bundle with the tested code that consumes them, never alone.
- **No drive-by refactoring.** Log it under Tech Debt (§7) and continue.
- **Surface plan amendments BEFORE code** (recon-class changes, signature changes, new primitives, floor-forecast breaches). Inline architectural decisions produce an ADR in the same commit.
- **Plain English when asking the founder questions** (non-coder product owner); reserve technical density for code/commits/ADRs/HANDOFF.
- **Never commit the project-level CLAUDE.md** (gitignored, local-only).
- **HANDOFF line "Last updated" is a lagging indicator.** For current-state questions, source-read §1 + cross-check `git log --oneline`.
- **Definition of Done (BRD §0.1):** build zero-warnings + affected-crate tests pass + clippy `-D warnings` clean + `fmt --check` passes + HANDOFF updated. All five or it's not done.

---

## 11 · 🗂️ Archives

- **`HANDOFF_V0.1_ARCHIVE.md`** — frozen 2026-05-06. T0.1.1–T0.1.12 narratives, ADRs 001–036, V0.1 tech-debt closures.
- **`HANDOFF_V0.2_PART1_ARCHIVE.md`** — frozen 2026-05-13 (T0.2.3 commit 2). T0.2.0–T0.2.3c2 narratives, ADRs 037–046 + amendments.
- **`HANDOFF_V0.2_PART2_ARCHIVE.md`** — frozen 2026-06-08 (PART2 split). T0.2.3c3 → T0.3.x narratives, ADRs 047–072 full text, the read-correctness + consolidator-REPORT + A5-contradiction arcs, full tech-debt narratives, technique map, consolidator inventory, V0.2 backend/tuning config.
- **`HANDOFF_V0.2_PART3_ARCHIVE.md`** — frozen 2026-06-29 (PART3 split, session 19). Sessions 2–18 §1 openers verbatim + full locked-ADR text: ADR-080 (scheduler/worker), ADR-081 (checkpoint+rollback), ADR-082 (incremental consolidation), ADR-083 (contradiction guard), ADR-084 (A1 cold archive), ADR-085 (topic clustering / Finding F), ADR-SEC-001 (multi-agent daemon), ADR-SEC-002 + Part 2 + Amendment 1 (graph at-rest encryption + self-cleaning + read-path wiring + 2-hop; the read-channel parked as tech-debt #9).

Cross-link out for detail; **do not paraphrase** archived ADRs or spec text — quote them.

When V0.2 closes (T0.2.13 ship + hard-gate clearance), a fresh slim HANDOFF.md opens for V1.0 per BRD §6.3.

---

## 12 · 🔧 Key reference (paths, models, commands, env)

**Repo:** https://github.com/shahbaz242630/Agent-Memory-Vault.git · **Local:** `C:\Projects\GitHub\Memory Vault` · **Spec:** `Agent Build Specification.txt` (BRD, canonical).

**Binary:** `C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe`
**Models / fixtures:** bge-small + qwen3-reranker fixtures under `crates/vault-embedding/test-fixtures/`.
**Real vault (production):** `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\{vault.db, lance, graph.duckdb}` (Tauri bundle id `com.shahbaz242630.memory-vault`). Dev vault is throwaway dogfood data — safe to wipe. [[project_dev_vault_is_throwaway_test_data]]
**Seeded test vaults:** `C:\Projects\seeded-vault-{100,1k,10k}`.

**Env (fresh PowerShell shell):**
```powershell
$env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"; $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
$env:LANCE_MEM_POOL_SIZE = '268435456'   # matters for heavy concurrent WRITES, not read-only tests
```

**Scale harness:** `cargo test -p vault-app --test scale_eval` (set `SCALE_EVAL_N` to size; real BGE + Qwen3-reranker, own temp vault). Live seeder: the `seed_live_vault` `#[ignore]` test (env `SEED_N` + `SEED_VAULT_DIR`).

**Disk note:** C: runs tight (~20 GB free at this session; `target/` ≈ 129 GB). Always check before a build. Surgical `cargo clean -p <crate>` first; full `cargo clean` is escalation.

---

## 13 · 🧪 Full-aspect live test campaign — scorecard + failure root-causes (2026-06-11)

Driven via a scripted MCP **stdio** client (`C:\Projects\mcp-probe\client.py`, NOT in repo) against `seeded-vault-mixed` (~94 messy+clean dogfood facts) + `seeded-vault-tiny` (6-fact consolidation demo). Antigravity quota was down so I acted as the MCP client directly (the structured contract the agent receives). **No production code changed.**

| Aspect | Verdict | Evidence |
|---|---|---|
| Write / Read / Update / Delete | ✅ | CRUD round-trip: write→read→update(content replaced)→delete(gone) |
| Search + recall-safety + `weak_match` | ✅ | never empty (even nonsense query → n=5, `weak_match=true`); `weak_match=false` only on real hits |
| Access control — reject unauthorized | ✅ | write to `secret` → `{-32001, "access denied"}` |
| Access control — accept authorized | ✅ | write to `testeval` → id returned |
| Boundary isolation | ✅ | `testeval` marker visible w/ testeval authorized, invisible w/ personal-only (n=10) |
| Encryption at rest — `vault.db` | ✅ | header = random bytes, not `SQLite format 3` (SQLCipher) |
| Graph encryption — `graph.duckdb` | ❌/⚠️ | `DUCK` magic = PLAINTEXT (tech-debt #7) |
| Merge / dedup | ✅ | tiny vault: 2 near-dup run-facts → 1, both originals superseded |
| REPORT (structured knowledge state) | ✅ | `personal.report.json` 4 auto-named topics, dates captured |
| Enrichment (Gap-2 recall lift) | ✅ | 1k MCP A/B (Porto ABSENT→1) + tiny-vault consolidate (4 enriched, 0 failed) |
| Abstain — clear absence (cat) | ✅ | `abstain=true`, surfaces dog Biscuit, invents no cat |
| Abstain — salary | ❌ | `abstain=false`, surfaces "$450 room booking" (conf 0.41) |
| Abstain — blood type / OS | ⚠️ | `abstain=false` but top_rel ~0.01–0.02 (marginal) |
| Wrong-neighbor precision | ⚠️ | distractor ranks #1: live→"mother in Lisbon", kids→"Marcus's kids", allergy→"Marcus's peanut" |
| Contradiction **resolution** | ⚠️ | Tesla/Rivian both stay active (0 resolved, 0 queued) even with `as_of` set |
| Decay / archive | ❌ | not built (T0.2.4) |

**One-line root-cause per non-pass item:**
- **Graph plaintext** — ADR-010's DuckDB encryption layer (scoped T0.2.0) never actually shipped; the store still opens plaintext (runtime still WARNs). Low risk only because the graph is empty in V0.2 (entity extraction unbuilt, tech-debt #2).
- **Salary $-trap** — the reranker scores money-shaped facts ("$450 booking", "rent 1200") as relevant to "salary" and there is no per-candidate category/precision filter to veto a confident wrong-category match; the abstain gate is purely reranker-score-driven and the score cleared the no-signal floor.
- **Blood/OS marginal abstain** — the no-signal floor (~0.01) sits just below where a couple of barely-related distractors score (0.011–0.019), so they squeak over and `abstain` stays false even though nothing relevant exists.
- **Wrong-neighbor #1 ordering** — the reranker ranks a semantically-adjacent fact about *someone/something else* (the mother, Marcus, the dog) above the user's own fact; there is no subject/ownership signal distinguishing "about the user" from "about an associate."
- **Contradiction not resolved** — NN-pair + Phi-4 judge did not flag Tesla vs Rivian as a contradiction (two cars can coexist / pair not surfaced), and `as_of` is write-time so there is no fact-time recency signal to force supersession; both remain active.
- **Decay/archive** — simply not implemented yet (Phase 4 / T0.2.4 never started; `memories_archived` returns 0).

**Verdict:** storage / retrieval / security / structural plumbing is **correct on messy data**; every gap is in the **precision/abstain** layer (read-precision arc, roadmap §5 item 1) or **temporal resolution** (`as_of`/A5) or **unbuilt nightly features** (decay/archive). Wave 3 (live Flash vs Pro on `seeded-vault-mixed`) is the remaining acceptance — does a real agent land the right answer from this structured output.

## 13.1 · 🧪 Wave 3 — DONE (live Flash + Opus 4.6 in Antigravity, 2026-06-12)

Live-agent run on `seeded-vault-mixed` (un-enriched). **Both models landed correct answers on essentially every trap** — the agent layer rescues a genuinely messy vault ranking. No code changed; CI stays green on `d613614`.

- **Gemini Flash (weak):** 14/15 atomic clean + 1 expected temporal partial (car: listed Tesla+Rivian, didn't resolve). On a multi-intent *sentence* Flash **mashed all 4 intents into one query** (`"languages sports teams reading holiday"`, top_relevance 0.040) → McLaren + The Expanse **buried out of the result window** → answer complete but **partly papered over with lucky-correct guesses**.
- **Claude Opus 4.6 (strong):** **decomposed** the same sentence into 4 focused `memory_search` calls → fully grounded, fully correct, both category traps held (Blade Runner out of "reading", Madrid framed as work not holiday), even synthesized accurate cross-links (Portuguese↔Porto, City↔Manchester from the wide recall pools).

**Probe replay — vault-level ground truth (raw `memory_read`, natural-question GRADE_QUERIES, agent stripped away).** Only **2 of 10 traps are vault-clean** (cat→abstain=True@0.25; instrument→cello #1@0.98). The other 8 are messy at the source:
- **Wrong-neighbour #1 at high confidence (0.88–0.99):** "where do you live" → **#1 = mother/Lisbon (0.99)**, Porto not even top-5; "have kids" → #1 = Marcus's kids (0.88), twins #3; "allergic" → #1 = Marcus's peanut (0.95), user's penicillin/shellfish #2/#3. The reranker confidently ranks an *associate's* fact above the user's own.
- **Salary trap fires at vault level:** abstain=False, #1 = "$450 booking" (0.41). Flash/Opus both rescued by reasoning from self-describing content.
- **Marginal abstain misses:** blood-type top 0.011, OS top 0.019 — both squeak over the `READ_NO_SIGNAL_FLOOR` 0.01, abstain stays False.
- **Contradiction unresolved:** car → Tesla(0.997)+Rivian both active, no supersession (`as_of` is write-time).

**Two findings (1 kept, 1 retracted):**
1. **KEPT — reranker brittle on terse keyword queries.** Natural questions score 0.88–0.99; terse fragments collapse to noise (Opus's `"sports teams follow"` → top 0.0022, "supports manchester city" ranked **#8 below junk**). Two query-style failure modes both → noise: weak-agent *mash* (dilution, facts buried) + strong-agent *keyword-strip* (facts present, ranked below noise). Fix = steer agents to **decompose AND phrase as natural questions** (`instructions.md` follow-up, §4). Memory: [[project_reranker_brittle_on_terse_queries]].
2. **RETRACTED — `search_hint.rs` weak_match is NOT buggy.** A mid-run hypothesis that `weak_match=false` on a noise-level separated top needed the ADR-073 no-signal floor was **falsified by a code read**: the separation-based (not magnitude) design is deliberate and documented (canonical example "cello 0.0469"), and `weak_match=false` is honest because matches genuinely exist in the pool. Do not change it.

**Net:** outcomes are good on both model tiers, but the **vault's ranking is genuinely messy** — the agent rescue is a crutch (a model weaker than Flash would faceplant on salary/allergy). This is the strongest evidence yet for **roadmap §5 item 1 (read precision): a subject/ownership signal so "about the user" beats "about an associate" + a category veto for the salary-shape trap.** Recall-safety ([[project_memory_read_primary_search_recall_safe]]) is the hero that makes the messy ranking survivable. The Gap-2 enrichment lift was NOT exercised here (mixed vault un-enriched); optionally enrich it (surgical `enrich_one` loop) to also test Porto-in-soup.

### 13.2 · Gap #4 (car/temporal) — ADR-075 fact-time SPIKED + REVERTED 2026-06-12; route to agent-steering, not vault resolution

Attempted Arc B (gap #4): a consolidation-time Phi-4 **fact-time extractor** (Option B, vault-owned; new `phases/fact_time.rs` + `effective_fact_time` recency input + Phase-2b wiring) to break the write-time recency tie that leaves Tesla+Rivian both active. Scaffolded, compiled clean (0 warnings), and **gated on a real-Phi-4 end-to-end spike** (`real_phi4_car_resolution`) **before any commit**. Spike result (110s) — **the car does NOT cleanly auto-resolve, for two independent reasons:**
1. **The conservative judge correctly refuses.** Real Phi-4 returned `contradiction=false` / `stale=[]` for "Drives a Tesla Model 3." + "Finally picked up my Rivian R1T last month." — owning two cars is genuinely possible; the judge only flags with an explicit replacement signal ("having sold the Tesla"), which the real content lacks. Making it more aggressive risks wrongly retiring coexisting facts (recall cardinal sin).
2. **The date-less old fact inverts recency.** Phi-4 DID extract the Rivian's "last month" → 2026-05-11 correctly, but the Tesla (no date in its text) falls back to write-time (today) → it looks *newer* than the Rivian → recency would retire the **wrong** (Rivian) car. `effective_fact_time`'s write-time fallback is unreliable for mixed dated/undated pairs.

**Decision (Shahbaz): reverted the scaffold; do NOT force vault-side car resolution.** This is the genuinely-ambiguous case the agent-decides lock ([[project_architectural_lock_llm_out_of_read_path]]) is *for* — both Flash & Opus presented both cars correctly above. **Re-route gap #4 to agent-steering** (the car steer, bundled with the gap-#7 terse-query steer — both landed this session as MCP tool-description edits, NOT an `instructions.md`: no such file exists; the tool descriptions are the cross-platform lever per [[project_mcp_descriptions_cross_platform_lever]]). Cheap, safe, no recall risk. The fact-time *extraction tech works* (Phi-4 nailed the relative date) — it's just the wrong lever for this case; the agent-settable `as_of` (2026-05-30 decision) remains the safe write-time path for explicit dates. Spiking caught this in 110s, before a build+commit+live-test cycle. Arc B code reverted (working tree back to CI-green `d613614` for the consolidator). Memory: [[project_as_of_write_time_blocks_a5_temporal]] (UPDATE 2026-06-12).

### 13.3 · 🆕 Gap-table reclassification (2026-06-12, Shahbaz) — NO confirmed-broken output; #1/#2 are insurance, not must-fix

**The reframe (Shahbaz caught the inconsistency):** Wave 3 showed the agent produces CORRECT OUTPUT on *every* tested trap — salary, allergy, wrong-neighbour, instrument, car. So the same logic that closed the car (#4 — "agent handles it, don't force a vault fix") applies to #1/#2/#3 too. They were over-stated as "must-fix." **By the founder thesis (correctness of OUTPUT is the product) there is NO confirmed-broken item in the gap table.** Distinction that survives: #4 (car) has *no single correct answer* (ambiguous → fixing is *wrong*); #1/#2 *have* a correct answer the vault mis-ranks (fixing is *safe* — reorder-only, no deletion — but *not urgent* since output is already correct).

| # | Gap | Output correct today? | Status | Note |
|---|---|---|---|---|
| 1 | Wrong-neighbour #1 ranking | ✅ agent rescues | 🟡 **Insurance** | Build only if a correct fact gets truncated out of the agent's ~20-candidate view at scale, OR to harden Managed-mode (unknown weak agents). Measured at vault level §13.1. Roadmap §5.1. |
| 2 | Salary $-trap | ✅ agent rescues | 🟡 **Insurance** | Same arc as #1. |
| 3 | Blood/OS marginal abstain | ✅ agent handles | 🅿️ **Parked** | Tightening the floor risks killing real low-score answers; recall lock wins. |
| 4 | Car / contradiction | ✅ agent shows both | ✅ **Decided — agent-steer** | Ambiguous; fact-time spiked + reverted (§13.2). Steer SHIPPED-pending-gates this session. |
| 5 | `graph.duckdb` plaintext | n/a | 🟢 **Low-pri** | Fold into Pillar 3 (sync) security review; graph empty in V0.2. |
| 6 | Decay / archive | n/a | 🟢 **Planned build** | Part of Pillar 2 (T0.2.4) — not separate work. |
| 7 | Reranker brittle on terse queries | ✅ Opus decomposed | 🟠 **Steer SHIPPED-pending-gates** | MCP tool-description edits this session (staged uncommitted). |

**Pillar reclassification:** Pillar 1 (read precision = #1/#2) **de-prioritised to insurance** — was "the #1 arc," downgraded today because output is already correct via the agent. Pillars 2 (consolidator auto-run — has the ~90-fact hardware wall), 3 (sync), 4 (beta/daily-use) unchanged. **Product call pending:** keep hardening (insurance) vs pivot to real daily dogfood (lean: dogfood-first, the core produces correct output and is ready to *use*).

**Working-tree state at this close:** (a) `crates/vault-mcp/src/server.rs` — gap-#7 + car steer tool-description edits, **staged, NOT gated/committed** (Shahbaz: gates tomorrow bundled with more code). (b) `HANDOFF.md` — this update. (c) Consolidator Arc B fully reverted (matches `d613614`). (d) Out-of-repo: memory `project_as_of_write_time_blocks_a5_temporal` UPDATE + NEW `project_reranker_brittle_on_terse_queries` + MEMORY.md index line. CI still green on `d613614`; next commit must gate the server.rs change + CI-verify.
