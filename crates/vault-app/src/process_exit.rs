//! `ProcessExit` — testability boundary for ADR-locked process-exit
//! semantics in [`crate::application::handle_signals`].
//!
//! ## Why this trait exists
//!
//! T0.1.10 Phase 2a locked the second-Ctrl-C → `std::process::exit(130)`
//! force-exit semantics in [`crate::application::handle_signals`]. The
//! exit code 130 (128 + SIGINT) is a load-bearing operational invariant
//! — wrapper scripts and CI pipelines distinguish "user-requested forced
//! exit" (130) from "general failure" (1). A future regression that
//! changed the exit code to e.g. 1 would silently corrupt that
//! distinction.
//!
//! `std::process::exit` cannot be tested directly (calling it from a
//! `#[test]` terminates the test runner). Two ways to make the
//! semantics testable:
//!
//! 1. **Mock the exit call** via a swappable trait. Production wires
//!    [`LiveProcessExit`] (calls `std::process::exit`); tests wire a
//!    capturing mock that records the code without actually exiting.
//!    Cost: one trait dispatch per second-Ctrl-C event (negligible).
//! 2. **Skip testing** the force-exit path — loses regression coverage
//!    on an ADR-locked invariant.
//!
//! Per Shahbaz v2 Phase 4 plan-paragraph review (2026-05-05) clarification 1:
//! ADR-locked behavior earns the small production indirection. Picked (1).
//!
//! ## Usage
//!
//! Production callsite in [`crate::application::Application::start_with_mcp`]
//! constructs `Arc::new(LiveProcessExit) as Arc<dyn ProcessExit>` and
//! passes to `handle_signals`. Tests construct a `CapturingProcessExit`
//! (in `#[cfg(test)]`) and assert the captured code after triggering the
//! second-Ctrl-C path.

/// Process-exit abstraction. Production impl calls `std::process::exit`;
/// test impl records the code without exiting.
///
/// `Send + Sync` because `handle_signals` is awaited from a `tokio::spawn`
/// task that may run on any worker thread.
pub trait ProcessExit: Send + Sync {
    /// Terminate the process with the given exit code. Diverges (`-> !`)
    /// per `std::process::exit`'s contract; production callers rely on
    /// this for the second-Ctrl-C force-exit.
    fn exit(&self, code: i32) -> !;
}

/// Production impl. Delegates to `std::process::exit`.
pub struct LiveProcessExit;

impl ProcessExit for LiveProcessExit {
    fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }
}

/// Test impl. Records the exit code in a shared `Mutex<Option<i32>>` and
/// panics with a marker payload that tests can catch via
/// `std::panic::catch_unwind`. Diverges (`-> !`) so callers can use it
/// in match arms expecting `!`.
///
/// Only available under `#[cfg(test)]` — production code MUST NOT use
/// this; would invert the discipline.
#[cfg(test)]
use std::sync::{Arc, Mutex};

#[cfg(test)]
pub struct CapturingProcessExit {
    captured: Arc<Mutex<Option<i32>>>,
}

#[cfg(test)]
impl CapturingProcessExit {
    pub fn new() -> Self {
        Self {
            captured: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns a clone of the captured-exit-code handle so the test can
    /// assert AFTER the panic has been caught.
    pub fn captured_handle(&self) -> Arc<Mutex<Option<i32>>> {
        self.captured.clone()
    }
}

#[cfg(test)]
impl Default for CapturingProcessExit {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl ProcessExit for CapturingProcessExit {
    fn exit(&self, code: i32) -> ! {
        *self
            .captured
            .lock()
            .expect("CapturingProcessExit mutex poisoned") = Some(code);
        // Panic with a marker payload. Tests use std::panic::catch_unwind
        // to convert this divergence into a Result, allowing assertion on
        // captured_handle without terminating the test binary.
        panic!("CapturingProcessExit::exit({code})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the testability infrastructure: `CapturingProcessExit::exit`
    /// records the code AND panics, so callers can capture the code
    /// post-panic via `std::panic::catch_unwind`. This proves the
    /// ADR-locked second-Ctrl-C-130 path is testable WITHOUT actually
    /// exiting the process.
    ///
    /// Phase 4b will use this fixture for the full
    /// `handle_signals_second_ctrl_c_force_exits_with_130` lifecycle
    /// test once signal-source dependency injection lands (Phase 4b
    /// prereq — `tokio::signal::ctrl_c` cannot be mocked directly,
    /// requires extracting the signal source as a separate trait dep).
    #[test]
    fn capturing_process_exit_records_code_and_panics_for_test_capture() {
        let exit = CapturingProcessExit::new();
        let captured_handle = exit.captured_handle();

        // Pre-call: nothing captured yet.
        assert_eq!(*captured_handle.lock().expect("mutex"), None);

        // Calling exit() panics. catch_unwind converts the divergence
        // to a Result so the test can continue to assert on the
        // captured value.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exit.exit(130);
        }));

        // The panic fired (divergence happened).
        assert!(
            result.is_err(),
            "CapturingProcessExit::exit MUST panic to preserve the `-> !` divergence \
             contract; otherwise production callers couldn't use it in match arms \
             expecting `!`."
        );

        // The exit code was recorded BEFORE the panic.
        assert_eq!(
            *captured_handle.lock().expect("mutex"),
            Some(130),
            "CapturingProcessExit MUST record the exit code BEFORE panicking so \
             tests can assert on the captured value (ADR-locked second-Ctrl-C-130 \
             path)."
        );
    }
}
