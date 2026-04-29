//! `vault-storage` — persistent storage across LanceDB (vectors), DuckDB (graph),
//! and SQLite/SQLCipher (metadata) with cascading writes.
//!
//! See `Agent Build Specification.txt` §5.2 for the public API specification.
//!
//! ## V0.1 progress
//!
//! - **T0.1.3 (this commit):** [`MetadataStore`] — encrypted SQLite + tamper-evident audit log
//! - T0.1.4: LanceDB vector store implementing `MemoryStore` trait
//! - T0.1.5: DuckDB graph store implementing `GraphStore` trait
//! - T0.1.6: `StorageBackend` orchestrator with cascading writes + retry queue

#![forbid(unsafe_code)]

pub mod audit;
pub mod key;
pub mod metadata_store;
pub(crate) mod migrations;

pub use audit::{
    seal, verify_chain, ActorKind, AuditEvent, AuditEventType, AuditResult, PendingAuditEvent,
    AUDIT_GENESIS_HASH,
};
pub use key::SqlCipherKey;
pub use metadata_store::{MemoryFilter, MetadataStore};
