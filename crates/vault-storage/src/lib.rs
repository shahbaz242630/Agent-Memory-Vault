//! `vault-storage` — persistent storage across LanceDB (vectors), DuckDB (graph),
//! and SQLite/SQLCipher (metadata) with cascading writes.
//!
//! See `Agent Build Specification.txt` §5.2 for the public API specification.
//! Real implementation lands across T0.1.3 → T0.1.6.

#![forbid(unsafe_code)]
