//! `vault-app` — application core. The only place where concrete implementations
//! of all module traits are wired together. All other crates depend on `dyn Trait`,
//! never on concrete types like `BgeSmallProvider` or `Phi4MiniProvider`.
//!
//! See `Agent Build Specification.txt` §5.10 for the public API specification.
//!
//! ## V0.1 progress
//!
//! - **T0.1.9 Phase 2 Step 6:** [`VaultAdapter`] — production impl of
//!   `vault_mcp::Adapter` (4 deps: `Arc<dyn Retriever>` +
//!   `Arc<dyn EmbeddingProvider>` + `StorageBackend` + `MetadataStore`).
//!   Per Q2 carry-forward (`T0.1.9_PLAN.md`), this is the
//!   minimal-just-the-type landing — Application / config / lifecycle
//!   bind together at T0.1.10.
//! - **T0.1.10 (pending):** `Application` struct + `Application::start` +
//!   `Application::shutdown` + config loading. Wires VaultAdapter at
//!   startup with concrete implementations of the four trait deps.

#![forbid(unsafe_code)]

mod adapter;

pub use adapter::VaultAdapter;
