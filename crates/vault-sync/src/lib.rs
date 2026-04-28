//! `vault-sync` — encrypted CRDT-based sync. Yrs document for change merging,
//! dryoc for application-level encryption, Cloudflare R2 + Workers for storage.
//!
//! See `Agent Build Specification.txt` §5.8 for the public API specification.
//! Real implementation lands across T0.2.9 → T0.2.13 (V0.2).
//!
//! Security boundary: this crate is the only place where vault data leaves the
//! device. Application-level encryption (XChaCha20-Poly1305 via dryoc) runs
//! BEFORE TLS so even a compromised TLS connection cannot leak plaintext.
//! See BRD §11.3 for the full crypto architecture.

#![forbid(unsafe_code)]
