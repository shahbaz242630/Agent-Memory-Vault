//! `vault-app` — application core. The only place where concrete implementations
//! of all module traits are wired together. All other crates depend on `dyn Trait`,
//! never on concrete types like `BgeSmallProvider` or `Phi4MiniProvider`.
//!
//! See `Agent Build Specification.txt` §5.10 for the public API specification.
//! Real implementation lands in T0.1.10.

#![forbid(unsafe_code)]
