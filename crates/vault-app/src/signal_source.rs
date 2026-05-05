//! `SignalSource` — testability boundary for `tokio::signal::ctrl_c`.
//!
//! Mirrors the [`crate::process_exit::ProcessExit`] pattern from T0.1.11
//! Phase 4a — production wires [`LiveSignalSource`] (which delegates to
//! `tokio::signal::ctrl_c`); tests wire `MockSignalSource` (in `#[cfg(test)]`)
//! with a queue of pre-loaded signal events to drive
//! [`crate::application::handle_signals`] through its first-Ctrl-C and
//! second-Ctrl-C paths without touching the real OS signal handler.
//!
//! ## Why
//!
//! `tokio::signal::ctrl_c` cannot be mocked directly — it talks to the
//! OS signal handler. To test the ADR-locked second-Ctrl-C-130 path
//! (T0.1.10 Phase 2a) end-to-end, the signal source needs to be a
//! swappable trait dependency.
//!
//! Combined with `ProcessExit` (4a), the lifecycle test wires a
//! `MockSignalSource` queue of `Ok(())` events + a
//! `CapturingProcessExit`, runs `handle_signals`, and asserts:
//! - first signal event → shutdown_signal channel received `true`
//! - second signal event → CapturingProcessExit captured exit code 130
//!
//! ## Workspace convention
//!
//! Uses `#[async_trait]` per the workspace pattern (verified Phase 4b v2:
//! `Cargo.toml:41` + `vault-app/Cargo.toml:13` already pull async-trait;
//! `vault-mcp::Adapter` trait at `vault-mcp/src/adapter.rs:36` uses the
//! same macro). Source-grounded pick over plain `async fn in trait` per
//! Shahbaz Phase 4b v2 review.

use async_trait::async_trait;

/// Signal-source abstraction. Production impl wraps
/// `tokio::signal::ctrl_c`; test impl pops events from a pre-loaded queue.
///
/// `Send + Sync` because `handle_signals` is awaited from a `tokio::spawn`
/// task that may run on any worker thread — same constraint as
/// `ProcessExit`.
#[async_trait]
pub trait SignalSource: Send + Sync {
    /// Wait for the next Ctrl-C / SIGINT signal. Returns `Ok(())` on
    /// signal received; `Err(io::Error)` if the underlying signal
    /// handler couldn't install (rare; matches `tokio::signal::ctrl_c`'s
    /// return shape).
    async fn next_signal(&self) -> std::io::Result<()>;
}

/// Production impl. Delegates to `tokio::signal::ctrl_c`. The cross-
/// platform support (Unix + Windows) is per T0.1.10 Phase 2a verification
/// against `docs.rs/tokio/1.52.1/tokio/signal/fn.ctrl_c.html`.
pub struct LiveSignalSource;

#[async_trait]
impl SignalSource for LiveSignalSource {
    async fn next_signal(&self) -> std::io::Result<()> {
        tokio::signal::ctrl_c().await
    }
}

/// Test fixture: pops events from a pre-loaded `VecDeque`. When the
/// queue is empty, `next_signal` blocks forever (mirroring a real
/// signal stream that has no pending signals — production code's
/// `await` would also block).
///
/// Use [`MockSignalSource::with_queue`] to construct with a specific
/// event sequence; lifecycle tests load `vec![Ok(()), Ok(())]` for the
/// double-Ctrl-C path or `vec![Err(io::Error::other("broken"))]` for
/// the signal-stream-broken graceful-exit path.
#[cfg(test)]
pub struct MockSignalSource {
    queue: tokio::sync::Mutex<std::collections::VecDeque<std::io::Result<()>>>,
}

#[cfg(test)]
impl MockSignalSource {
    pub fn with_queue(events: Vec<std::io::Result<()>>) -> Self {
        Self {
            queue: tokio::sync::Mutex::new(events.into()),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl SignalSource for MockSignalSource {
    async fn next_signal(&self) -> std::io::Result<()> {
        let mut q = self.queue.lock().await;
        match q.pop_front() {
            Some(event) => event,
            None => {
                // Queue empty — block forever. In a real test, this
                // means handle_signals has consumed all expected
                // signals and is now waiting; the test harness uses
                // a `tokio::time::timeout` wrapper to bound the wait.
                std::future::pending::<()>().await;
                unreachable!("std::future::pending never resolves")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the testability infrastructure: queue events are popped in
    /// FIFO order; subsequent calls when queue is empty would block
    /// forever (verified by NOT awaiting the third call in this test).
    #[tokio::test]
    async fn mock_signal_source_pops_queue_in_order() {
        let mock = MockSignalSource::with_queue(vec![Ok(()), Err(std::io::Error::other("broken"))]);

        // First call returns Ok.
        let first = mock.next_signal().await;
        assert!(
            first.is_ok(),
            "MockSignalSource::next_signal MUST pop the first queue \
             entry (Ok(())) on first call; got {:?}",
            first.err()
        );

        // Second call returns the queued Err.
        let second = mock.next_signal().await;
        assert!(
            second.is_err(),
            "MockSignalSource::next_signal MUST pop the second queue \
             entry (Err) on second call; got {:?}",
            second.ok()
        );
    }
}
