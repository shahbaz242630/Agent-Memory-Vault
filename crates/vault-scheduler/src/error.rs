//! Error types for `vault-scheduler`.
//!
//! Per BRD §2.4 this crate carries its own `thiserror`-based error enum and
//! converts to [`VaultError`] at the workspace boundary. The mapping is
//! deliberate (ADR-SEC-005):
//!
//! - [`SchedulerError::InvalidSpec`] maps onto [`VaultError::InvalidInput`],
//!   not [`VaultError::Scheduler`] — a rejected spec is an input-validation
//!   failure at the API boundary, caught *before* any OS scheduler is touched.
//! - Everything that represents an actual backend failure maps onto the
//!   dedicated [`VaultError::Scheduler`] category so callers can surface a
//!   scheduler-specific message.

use thiserror::Error;
use vault_core::VaultError;

/// Scheduler-specific failure categories.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// The [`crate::ScheduleSpec`] (or one of its fields) failed validation
    /// before any OS call. This is the injection-safety gate (ADR-SEC-005):
    /// a task id with illegal characters, a control character in an argument
    /// or environment value, an illegal environment-variable name, or an
    /// empty program path is rejected here — before it can reach `schtasks`,
    /// a launchd plist, or a systemd/cron line where it might break out of
    /// its intended field.
    #[error("invalid schedule spec: {0}")]
    InvalidSpec(String),

    /// The underlying OS scheduler tool exited non-zero, produced
    /// unparseable output, or could not be invoked (`schtasks` /
    /// `launchctl` / `systemctl --user`). Carries the tool's context for the
    /// operator; the user-facing layer maps this to a friendly message.
    #[error("scheduler backend failed: {0}")]
    BackendFailed(String),

    /// The current platform has no supported scheduler backend compiled in.
    /// (The three supported backends are cfg-gated per target OS; this is the
    /// fallback for any other target.)
    #[error("no scheduler backend for this platform")]
    Unsupported,

    /// I/O failure while writing a backend artefact (a launchd plist or a
    /// systemd unit file) or while spawning the backend tool.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Standard result alias used throughout `vault-scheduler`.
pub type SchedulerResult<T> = Result<T, SchedulerError>;

impl From<SchedulerError> for VaultError {
    fn from(value: SchedulerError) -> Self {
        match value {
            // A rejected spec is an input-validation failure, not a scheduler
            // subsystem failure — no OS call was ever attempted.
            SchedulerError::InvalidSpec(msg) => VaultError::InvalidInput(msg),
            // Preserve the underlying io::Error so callers that classify on
            // VaultError::Io keep working.
            SchedulerError::Io(err) => VaultError::Io(err),
            // Genuine backend failures collapse into the dedicated category;
            // the display string already carries the specific context.
            other @ (SchedulerError::BackendFailed(_) | SchedulerError::Unsupported) => {
                VaultError::Scheduler(other.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_prefixed_by_category() {
        assert!(SchedulerError::InvalidSpec("bad id".into())
            .to_string()
            .starts_with("invalid schedule spec:"));
        assert!(SchedulerError::BackendFailed("schtasks exited 1".into())
            .to_string()
            .starts_with("scheduler backend failed:"));
        assert_eq!(
            SchedulerError::Unsupported.to_string(),
            "no scheduler backend for this platform"
        );
    }

    #[test]
    fn invalid_spec_maps_to_invalid_input_not_scheduler() {
        // A rejected spec never reached the OS, so it must NOT masquerade as a
        // scheduler-subsystem failure — it is an input-validation failure.
        let converted: VaultError = SchedulerError::InvalidSpec("illegal task id".into()).into();
        assert!(matches!(converted, VaultError::InvalidInput(_)));
    }

    #[test]
    fn backend_failure_maps_to_scheduler_category() {
        let converted: VaultError =
            SchedulerError::BackendFailed("launchctl load failed".into()).into();
        assert!(matches!(converted, VaultError::Scheduler(_)));
        let unsupported: VaultError = SchedulerError::Unsupported.into();
        assert!(matches!(unsupported, VaultError::Scheduler(_)));
    }

    #[test]
    fn io_error_round_trips_through_both_layers() {
        let io_err = std::io::Error::other("simulated");
        let sched_err: SchedulerError = io_err.into();
        assert!(matches!(sched_err, SchedulerError::Io(_)));
        let vault_err: VaultError = sched_err.into();
        assert!(matches!(vault_err, VaultError::Io(_)));
    }
}
