//! `vault-storage` — persistent storage across LanceDB (vectors), DuckDB (graph),
//! and SQLite/SQLCipher (metadata) with cascading writes.
//!
//! See `Agent Build Specification.txt` §5.2 for the public API specification.
//!
//! ## V0.1 progress
//!
//! - **T0.1.3:** [`MetadataStore`] — encrypted SQLite + tamper-evident audit log
//! - **T0.1.4:** [`LanceVectorStore`] — LanceDB-backed vector store + boundary-leak guard
//! - **T0.1.5 (in flight):** [`DuckDbGraphStore`] — DuckDB graph store with bi-temporal
//!   relationships and ADR-015 boundary-scoped entities
//! - T0.1.6: `StorageBackend` orchestrator with cascading writes + retry queue

#![forbid(unsafe_code)]

pub mod audit;
pub mod graph_store;
pub mod key;
pub mod metadata_store;
pub(crate) mod migrations;
pub(crate) mod migrations_graph;
pub mod vector_store;

pub use audit::{
    seal, verify_chain, ActorKind, AuditEvent, AuditEventType, AuditResult, PendingAuditEvent,
    AUDIT_GENESIS_HASH,
};
pub use graph_store::{
    DuckDbGraphStore, GraphStore, TraversalOptions, CROSS_BOUNDARY_RELATION_TYPES,
};
pub use key::SqlCipherKey;
pub use metadata_store::{MemoryFilter, MetadataStore};
pub use vector_store::{LanceVectorStore, VectorStore, ALPHA_WARNING_FILENAME};
