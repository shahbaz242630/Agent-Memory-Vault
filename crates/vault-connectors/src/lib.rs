//! `vault-connectors` — auto-population from Gmail, Calendar, Notion, etc.
//! via read-only OAuth scopes (BRD §11.8.1). All extracted memories enter a
//! review queue before commit; auto-commit can only be enabled by the user
//! after a 3-month track record of approvals.
//!
//! See `Agent Build Specification.txt` §5.9 for the public API specification.
//! V1.0 (T1.0.4 → T1.0.8) ships Gmail + Calendar; Notion/Slack/GitHub deferred to V1.1+.

#![forbid(unsafe_code)]
