//! `vault-mcp` — MCP adapter layer + server. Wraps the official `rmcp` SDK behind
//! a vault-specific abstraction so we can swap MCP implementations or absorb
//! protocol changes without touching business logic.
//!
//! See `Agent Build Specification.txt` §5.7 for the public API specification.
//! V0.1 (T0.1.9) ships stdio transport with `memory.search` and `memory.write`.
//! HTTP/SSE transport is deferred to V1.1+.
//!
//! Security boundary: this crate enforces mandatory access control. Per BRD §11.4.3,
//! the boundary filter must be applied at the storage level (not application code)
//! so buggy retrieval logic cannot return cross-boundary results.

#![forbid(unsafe_code)]
