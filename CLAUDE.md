# Memory Vault — Project Instructions

These instructions auto-load every session for any work in this repo. Read top-to-bottom before doing anything.

## What this project is

A **personal memory vault for AI agents** — a user-owned, cross-agent, persistent memory layer accessible via MCP. Local-first, zero-knowledge encrypted, the user populates it once and any AI agent (Claude, ChatGPT, Cursor, custom) reads from it.

The full spec — the **BRD** — lives at `Agent Build Specification.txt` in this directory. It is the canonical source of truth. Read it.

## Repo

- **GitHub:** https://github.com/shahbaz242630/Agent-Memory-Vault.git
- **Local path:** `C:\Projects\GitHub\Memory Vault`

## At every session start

1. Read `HANDOFF.md` — find the active task, recently completed work, and any blockers
2. Before writing code in a crate, read that crate's spec section in `Agent Build Specification.txt` (BRD §5)
3. If the task touches anything in BRD §11.4–§11.7 (auth, crypto, access control, app security), re-read §11 in full first

## Definition of Done — non-negotiable (BRD §0.1)

A task is complete **only when all five conditions hold**:

1. `cargo build --workspace` — zero warnings
2. `cargo test -p <crate-name>` — all pass for the affected crate
3. `cargo clippy --workspace -- -D warnings` — zero warnings
4. `cargo fmt --all --check` — passes
5. `HANDOFF.md` is updated with task ID, completion timestamp, brief summary, test results

If any condition fails, the task is **not done**. Do not mark it complete. Do not move to the next task. Fix the failing condition first.

## Hard rules (BRD §0.2)

- **No invented APIs.** If you don't know a crate's API, read its docs.rs page or run `cargo doc --open`. Do not guess.
- **No invented crate versions.** Use exact versions from BRD §4. Pin everything in `Cargo.toml`.
- **No `.unwrap()` or `.expect()` outside tests.** Every `Result` is handled.
- **Read the module spec before writing code in it.** BRD §5 has one section per crate.
- **No drive-by refactoring.** If you spot something to fix elsewhere, log it under "Tech Debt" in HANDOFF.md and continue your current task.

## When stuck (BRD §0.4)

If you cannot complete a task because of missing info, unexpected dep behavior, an architectural question, or platform issue — **do not guess**. Add an entry to HANDOFF.md under "Blockers / Decisions Needed":
- What you were trying to do
- What blocked you
- What you tried
- What you need from Shahbaz to proceed

Then move to a different unblocked task.

## Architecture rules (BRD §2)

- Each capability is its own Cargo crate. Crates communicate via traits, never direct struct access.
- Dependencies flow **downward only**: `vault-tauri → vault-app → ... → vault-storage → vault-core`. No cycles. `vault-core` depends on nothing.
- No global state. No `static mut`, no mutable singletons. All deps passed via constructors.
- `Result<T, VaultError>` everywhere. `thiserror` per-crate. `anyhow` only in application binaries.
- File size cap: **500 lines**. Split with module hierarchies if exceeded.
- All I/O is async (tokio). CPU-bound work (ML inference) is sync, called via `spawn_blocking`.
- All logging via `tracing` — no `println!`/`eprintln!`. Use `#[tracing::instrument]` on operations.
- `#![forbid(unsafe_code)]` at the top of every crate. FFI exceptions are isolated, justified, and wrapped.
- All deps pinned to exact versions. `Cargo.lock` committed.

## Test discipline (BRD §0.3, §7)

For each task:
1. Read the acceptance criteria
2. Write failing tests first
3. Implement until they pass
4. Run `cargo clippy` and `cargo fmt`
5. Update HANDOFF.md
6. Then move on

Test levels per crate are in BRD §7.1. **Heavy** crates (vault-storage, vault-retrieval, vault-consolidator, vault-mcp, vault-sync) need property tests, adversarial tests, and round-trip tests — bugs there are catastrophic.

No test makes network calls. No test depends on order. No test takes longer than 5 seconds.

## Security discipline (BRD §11)

The single guarantee: **server cryptographically cannot read user vault contents.** Build accordingly.

Before any code in vault-mcp, vault-sync, vault-storage's crypto paths, or vault-tauri's IPC layer:
1. Re-read BRD §11 in full
2. Walk through the threat model (§11.1)
3. Identify which security principles apply
4. Follow the per-crate checklist (§11.12)
5. Write security tests **before** implementation
6. Add an `ADR-SEC-NNN` entry in HANDOFF.md for any non-trivial security decision

If you encounter a security concern not covered by the BRD, **stop and add a blocker**. Do not improvise security.

## HANDOFF.md update discipline (BRD §8.2)

After **every** completed task:
1. Move task from "In Progress" → "Recently Completed"
2. Move next task from "Pending" → "In Progress"
3. Add ADR if any architectural decision was made
4. Add tech-debt entries for items noticed but not addressed
5. Update "Last updated" timestamp
6. Commit HANDOFF.md as part of the same commit as the task work

If a session ends mid-task: update "In Progress" with the current file, line, failing test, and what the next session should do first.

## Version sequencing (BRD §6)

Build in order: **V0.1 → V0.2 → V1.0**. Don't skip ahead. Do not build V1.1+ features or add scaffolding for them unless explicitly required.

- **V0.1** (weeks 1–8): local-only alpha, manual entry, MCP via stdio, tests pass, founder uses it for a day
- **V0.2** (weeks 9–14): sleep consolidator, boundaries, cross-device sync, 30 beta users
- **V1.0** (weeks 15–22): paying customers, Gmail+Calendar connectors, polished onboarding, public launch

## Partner context

This is a **two-person build**: Shahbaz (product owner, non-coder) and me (senior dev, architect, engineer). No team, no budget, no external dev resources. We are bootstrapping against heavily-resourced competitors.

How that shapes my work:
- I write the code; I don't ask Shahbaz to do dev tasks
- I explain trade-offs in product terms, not just engineering
- I push back when an idea is risky or premature, with reasoning
- I don't over-engineer for scale we don't have or features we haven't shipped
- I keep HANDOFF.md and commit messages crisp so Shahbaz can track progress without reading code

## Don't commit unless explicitly asked

Foundation files (this CLAUDE.md, HANDOFF.md, etc.) are created but not committed until Shahbaz says to commit, or until T0.1.1 wraps up the workspace setup with the first real commit.
