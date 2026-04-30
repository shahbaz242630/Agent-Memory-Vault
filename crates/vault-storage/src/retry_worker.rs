//! [`RetryWorker`] — drives `retry_queue` entries through their cascading
//! downstream writes (T0.1.6 Phase C1b).
//!
//! ## Two entry points
//!
//! - [`RetryWorker::step`] runs **exactly one** iteration: poll the
//!   oldest due entry, attempt the cascade, persist the outcome, return
//!   a [`StepResult`]. Used by tests to drive deterministic scenarios
//!   without spawning a tokio task.
//! - [`RetryWorker::run`] is the production loop: alternate
//!   `step()` + `sleep(poll_interval)` until the supplied
//!   [`tokio::sync::watch::Receiver<bool>`] flips to `true`. Honors
//!   graceful shutdown — finishes the current step before exiting.
//!
//! Phase C1b does NOT spawn the worker. T0.1.10 (vault-app) creates the
//! `watch::channel(false)`, calls `tokio::spawn(worker.run(rx))`, and
//! holds the JoinHandle + sender for shutdown.
//!
//! ## Lockstep + idempotency (ADR-017 / Issue 1)
//!
//! Every step runs both store ops idempotently in sequence:
//! - LanceDB upsert / delete (idempotent because `merge_insert(["id"])`
//!   is a no-op on identical input).
//! - DuckDB graph op (V0.1: no-op for memory cascades; the graph cascade
//!   becomes meaningful at T0.2.2 when the consolidator wires through).
//!
//! Either failure → whole entry reschedules (or dead-letters if attempts
//! are exhausted / the error is permanent). Partial-success state is
//! *not* tracked on the entry — the underlying upserts ARE the source of
//! truth.

#![allow(dead_code)] // RetryWorker is wired by T0.1.10's Application::start.

use std::time::Duration;

#[cfg(test)]
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::{debug, error, instrument, warn};
use uuid::Uuid;

use vault_core::{Boundary, VaultError, VaultResult};

use crate::audit::{ActorKind, AuditEventType, PendingAuditEvent};
use crate::cascading::{CascadePayloadV1, StorageBackend};
use crate::dead_letter::{NewDeadLetter, PAYLOAD_FORMAT_VERSION as DL_PAYLOAD_VERSION};
use crate::retry_queue::{
    is_permanent, CascadeOperation, DeadLetterReason, FailureOutcome, JitterSource, RetryEntry,
    SeededJitter,
};

#[cfg(test)]
use crate::fault_injection;

/// Default polling cadence for [`RetryWorker::run`]. The 1s base of the
/// backoff schedule (see `retry_queue::base_backoff_secs`) means a
/// shorter interval just wastes wakeups on the empty case.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Outcome of [`RetryWorker::step`].
#[derive(Debug, PartialEq, Eq)]
pub enum StepResult {
    /// No entries due; the production loop should sleep until
    /// `poll_interval` elapses.
    Idle,
    /// Worked one entry to success.
    SucceededEntry { id: Uuid, attempts_made: u32 },
    /// Worked one entry to a reschedule (failure, retries remaining).
    Rescheduled {
        id: Uuid,
        next_attempt_at: DateTime<Utc>,
    },
    /// Worked one entry to dead-letter (exhausted or permanent).
    DeadLettered { id: Uuid, reason: DeadLetterReason },
}

/// Cascading-write retry worker.
pub struct RetryWorker {
    backend: StorageBackend,
    jitter: Box<dyn JitterSource>,
    poll_interval: Duration,
    #[cfg(test)]
    fault_injector: Arc<dyn fault_injection::FaultInjector>,
}

impl RetryWorker {
    /// Construct a worker with default poll interval and a system-time
    /// seeded jitter source.
    pub fn new(backend: StorageBackend) -> Self {
        Self::with_jitter(backend, Box::new(SeededJitter::from_system_time()))
    }

    /// Construct a worker with a caller-supplied jitter source. Tests use
    /// this with [`crate::retry_queue::FixedJitter`] for deterministic
    /// schedule arithmetic.
    pub fn with_jitter(backend: StorageBackend, jitter: Box<dyn JitterSource>) -> Self {
        Self {
            backend,
            jitter,
            poll_interval: DEFAULT_POLL_INTERVAL,
            #[cfg(test)]
            fault_injector: Arc::new(fault_injection::NoFault),
        }
    }

    /// Override the polling cadence used by [`Self::run`]. Builder-style.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Test-only: install a [`fault_injection::FaultInjector`] consulted
    /// before every downstream store call. Builder-style.
    #[cfg(test)]
    #[must_use]
    pub fn with_fault_injector(
        mut self,
        injector: Arc<dyn fault_injection::FaultInjector>,
    ) -> Self {
        self.fault_injector = injector;
        self
    }

    /// Run exactly one iteration of the retry loop using the system clock.
    pub async fn step(&mut self) -> VaultResult<StepResult> {
        self.step_at(Utc::now()).await
    }

    /// Run exactly one iteration as if the current time were `now`. Tests
    /// use this to fast-forward past backoff intervals without sleeping.
    #[instrument(skip(self), fields(now = %now))]
    pub async fn step_at(&mut self, now: DateTime<Utc>) -> VaultResult<StepResult> {
        let mut due = self.backend.retry_queue().poll_due(now, 1).await?;
        let Some(entry) = due.pop() else {
            return Ok(StepResult::Idle);
        };

        let attempt_outcome = self.run_cascade(&entry).await;

        match attempt_outcome {
            Ok(()) => {
                self.backend.retry_queue().record_success(entry.id).await?;
                debug!(id = %entry.id, "cascade succeeded");
                Ok(StepResult::SucceededEntry {
                    id: entry.id,
                    attempts_made: entry.attempts_made + 1,
                })
            }
            Err(err) => {
                let permanent = is_permanent(&err);
                let outcome = self
                    .backend
                    .retry_queue()
                    .record_failure(entry.id, &err.to_string(), permanent, &mut *self.jitter)
                    .await?;
                match outcome {
                    FailureOutcome::Rescheduled {
                        next_attempt_at, ..
                    } => {
                        debug!(id = %entry.id, %next_attempt_at, "cascade rescheduled");
                        Ok(StepResult::Rescheduled {
                            id: entry.id,
                            next_attempt_at,
                        })
                    }
                    FailureOutcome::DeadLetter {
                        entry: dead_entry,
                        last_error,
                        reason,
                    } => {
                        self.write_dead_letter(&dead_entry, &last_error, reason)
                            .await?;
                        warn!(id = %entry.id, ?reason, %last_error, "cascade dead-lettered");
                        Ok(StepResult::DeadLettered {
                            id: entry.id,
                            reason,
                        })
                    }
                }
            }
        }
    }

    /// Production loop. Until `cancel` flips to `true`, alternate
    /// `step().await` + sleep(poll_interval). Sleeps interrupt on cancel
    /// so shutdown doesn't wait the full delay.
    pub async fn run(mut self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        loop {
            if *cancel.borrow() {
                break;
            }
            match self.step().await {
                Ok(_) => {}
                Err(e) => {
                    // Per Phase A's "background task lifecycle" — log the
                    // error and keep going. A single failed step doesn't
                    // tear down the whole worker; only repeated panics do
                    // (T0.1.10 wraps `run` in panic-recovery).
                    error!(error = %e, "retry worker step failed; continuing");
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(self.poll_interval) => {}
                _ = cancel.changed() => { break; }
            }
        }
    }

    /// Drive both store-side ops for a single retry entry. Returns the
    /// first error encountered or Ok(()) on full success.
    async fn run_cascade(&self, entry: &RetryEntry) -> VaultResult<()> {
        let payload: CascadePayloadV1 = serde_json::from_value(entry.payload.clone())
            .map_err(|e| VaultError::Storage(format!("decode cascade payload: {e}")))?;

        // Vector-side op.
        self.fault_check_vector()?;
        match entry.operation {
            CascadeOperation::Write | CascadeOperation::Update => {
                let boundary = Boundary::new(payload.boundary.clone())
                    .map_err(|e| VaultError::Storage(format!("payload boundary invalid: {e}")))?;
                self.backend
                    .vector_store()
                    .upsert(&entry.memory_id, &payload.embedding, &boundary)
                    .await?;
            }
            CascadeOperation::Delete => {
                self.backend.vector_store().delete(&entry.memory_id).await?;
            }
        }

        // Graph-side op. V0.1: no-op for memory cascades — entities ship
        // at T0.2.2 (consolidator). The fault hook stays so adversarial
        // tests can drive the graph-failure scenario today.
        self.fault_check_graph()?;
        // No real DuckDB call here yet; deliberately empty.

        Ok(())
    }

    /// Insert a `dead_letter` row + emit a CRITICAL `cascade.dead_letter`
    /// audit event. Both happen outside any with_transaction — the queue
    /// row was already removed by `record_failure` in its own transaction.
    /// The two writes here are independently consistent: if the audit
    /// event lands without the dead_letter row, vault-cli would still see
    /// the underlying `MemoryCreate` event with no follow-up cascade,
    /// which is not silent corruption.
    async fn write_dead_letter(
        &self,
        entry: &RetryEntry,
        last_error: &str,
        reason: DeadLetterReason,
    ) -> VaultResult<()> {
        let new = NewDeadLetter {
            memory_id: entry.memory_id,
            failed_operation: entry.operation,
            failure_reason: last_error.to_string(),
            attempts_made: entry.attempts_made,
            first_failed_at: entry.created_at,
            last_attempted_at: Utc::now(),
            payload_format_version: DL_PAYLOAD_VERSION,
            payload: entry.payload.clone(),
        };
        self.backend.dead_letter().insert(new).await?;

        let reason_str = match reason {
            DeadLetterReason::AttemptsExhausted => "attempts_exhausted",
            DeadLetterReason::Permanent => "permanent",
        };
        let details = format!(
            r#"{{"reason":"{}","operation":"{}","attempts_made":{},"last_error":{}}}"#,
            reason_str,
            entry.operation.as_str(),
            entry.attempts_made,
            json_string(last_error),
        );
        self.backend
            .metadata()
            .append_audit_event(
                PendingAuditEvent::success(AuditEventType::CascadeDeadLetter, ActorKind::System)
                    .error()
                    .with_resource("memory", entry.memory_id.to_string())
                    .with_details_json_inline(details),
            )
            .await?;
        Ok(())
    }

    #[cfg(test)]
    fn fault_check_vector(&self) -> VaultResult<()> {
        fault_injection::into_result(self.fault_injector.vector_decision())
    }
    #[cfg(not(test))]
    fn fault_check_vector(&self) -> VaultResult<()> {
        Ok(())
    }

    #[cfg(test)]
    fn fault_check_graph(&self) -> VaultResult<()> {
        fault_injection::into_result(self.fault_injector.graph_decision())
    }
    #[cfg(not(test))]
    fn fault_check_graph(&self) -> VaultResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PendingAuditEvent inline-details extension. Same shape as the helper in
// `cascading.rs` — kept private here to avoid creating a public
// dependency surface across modules. Trait names differ so they don't
// collide if both modules are imported.
// ---------------------------------------------------------------------------

trait PendingAuditEventDetails {
    fn with_details_json_inline(self, json: String) -> Self;
}

impl PendingAuditEventDetails for PendingAuditEvent {
    fn with_details_json_inline(mut self, json: String) -> Self {
        self.details_json = json;
        self
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"<unserialisable>\"".to_string())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use tempfile::TempDir;

    use vault_core::{Memory, MemoryType, NewMemory};

    use crate::audit::AuditEvent;
    use crate::cascading::{StorageBackend, MAX_RETRY_QUEUE_DEPTH};
    use crate::fault_injection::{AlwaysFailGraph, AlwaysFailVector, NoFault};
    use crate::key::SqlCipherKey;
    use crate::retry_queue::FixedJitter;

    const DIM: usize = 4;

    fn embedding(fill: f32) -> Vec<f32> {
        vec![fill; DIM]
    }

    fn sample_memory(boundary: &str, content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .unwrap()
    }

    async fn make_backend() -> (TempDir, StorageBackend) {
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("retry-worker-test-key");
        let backend = StorageBackend::open(&metadata_path, &vector_dir, &graph_path, key, DIM)
            .await
            .unwrap();
        (tmp, backend)
    }

    fn make_worker(backend: StorageBackend) -> RetryWorker {
        RetryWorker::with_jitter(backend, Box::new(FixedJitter(0.0)))
    }

    fn make_worker_with_fault(
        backend: StorageBackend,
        injector: Arc<dyn fault_injection::FaultInjector>,
    ) -> RetryWorker {
        make_worker(backend).with_fault_injector(injector)
    }

    /// Time after the worst-case enqueue-time delay (next_attempt_at sits
    /// at most 1.25s after enqueue with FixedJitter(0)).
    fn far_future() -> DateTime<Utc> {
        Utc::now() + chrono::Duration::seconds(60 * 60)
    }

    // ------------------------------------------------------------------
    // step() behaviour on idle / clean-success cases
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn step_on_empty_queue_returns_idle() {
        let (_tmp, backend) = make_backend().await;
        let mut w = make_worker(backend);
        let r = w.step().await.unwrap();
        assert_eq!(r, StepResult::Idle);
    }

    #[tokio::test]
    async fn step_on_clean_write_succeeds_and_drains_entry() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "drained");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();
        assert_eq!(backend.retry_queue().len().await.unwrap(), 1);

        let mut w = make_worker(backend.clone());
        let r = w.step_at(far_future()).await.unwrap();
        match r {
            StepResult::SucceededEntry { attempts_made, .. } => {
                assert_eq!(attempts_made, 1);
            }
            other => panic!("expected SucceededEntry, got {other:?}"),
        }
        assert_eq!(backend.retry_queue().len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn step_on_clean_delete_runs_vector_delete_and_drains() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "to be deleted");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();
        // Drain the write cascade.
        let mut w = make_worker(backend.clone());
        w.step_at(far_future()).await.unwrap();

        // Now a delete cascade.
        backend.delete_memory(&m.id).await.unwrap();
        assert_eq!(backend.retry_queue().len().await.unwrap(), 1);

        let r = w.step_at(far_future()).await.unwrap();
        assert!(matches!(r, StepResult::SucceededEntry { .. }));
        assert_eq!(backend.retry_queue().len().await.unwrap(), 0);

        // A subsequent search returns no row for this id.
        let b = Boundary::new("work").unwrap();
        let hits = backend
            .vector_store()
            .search(&embedding(0.2), 100, &[b])
            .await
            .unwrap();
        assert!(
            hits.iter().all(|(id, _)| id != &m.id),
            "vector store should not return the deleted id"
        );
    }

    // ------------------------------------------------------------------
    // step() behaviour under transient vector failure → reschedule path
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn step_with_transient_vector_failure_reschedules() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "transient");
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();

        let mut w = make_worker_with_fault(
            backend.clone(),
            Arc::new(AlwaysFailVector("simulated transient lance io".into())),
        );
        let r = w.step_at(far_future()).await.unwrap();
        match r {
            StepResult::Rescheduled { .. } => {}
            other => panic!("expected Rescheduled, got {other:?}"),
        }
        // Entry still in queue, attempts_made = 1.
        let due = backend
            .retry_queue()
            .poll_due(far_future(), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].attempts_made, 1);
        assert!(due[0].last_error.as_ref().unwrap().contains("simulated"));
    }

    // ------------------------------------------------------------------
    // step() under persistent failure → dead-letter at MAX_ATTEMPTS
    // (Phase A Q5 test 2: persistent LanceDB failure → dead-letter)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn step_with_persistent_vector_failure_dead_letters_at_max_attempts() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "doomed cascade");
        backend.write_memory(&m, &embedding(0.4)).await.unwrap();

        let mut w = make_worker_with_fault(
            backend.clone(),
            Arc::new(AlwaysFailVector("simulated lance io".into())),
        );

        // Drive 8 attempts. Each step fast-forwards past the schedule.
        // FixedJitter(0.0) means no jitter — exact schedule.
        let mut last = StepResult::Idle;
        for i in 0..8 {
            let r = w.step_at(far_future()).await.unwrap();
            if i < 7 {
                assert!(
                    matches!(r, StepResult::Rescheduled { .. }),
                    "attempt {i} should reschedule, got {r:?}"
                );
            }
            last = r;
        }
        match last {
            StepResult::DeadLettered {
                reason: DeadLetterReason::AttemptsExhausted,
                ..
            } => {}
            other => panic!("attempt 8 should dead-letter (exhausted), got {other:?}"),
        }

        // Queue is empty.
        assert_eq!(backend.retry_queue().len().await.unwrap(), 0);

        // Dead-letter row exists with attempts_made = 8.
        let dls = backend.dead_letter().list_unresolved(100).await.unwrap();
        assert_eq!(dls.len(), 1);
        assert_eq!(dls[0].memory_id, m.id);
        assert_eq!(dls[0].attempts_made, 8);
        assert!(dls[0].failure_reason.contains("simulated"));

        // Audit log carries one cascade.dead_letter event with reason
        // "attempts_exhausted" and `error` result.
        let events = backend
            .metadata()
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let dl_events: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::CascadeDeadLetter)
            .collect();
        assert_eq!(dl_events.len(), 1);
        assert_eq!(dl_events[0].result, crate::audit::AuditResult::Error);
        assert!(dl_events[0]
            .details_json
            .contains("\"reason\":\"attempts_exhausted\""));
    }

    // ------------------------------------------------------------------
    // step() under permanent error class → dead-letter on attempt 1
    // (ADR-009 amendment: is_permanent → dead-letter immediately)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn step_with_permanent_classified_error_dead_letters_on_attempt_one() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "schema-mismatch");
        backend.write_memory(&m, &embedding(0.5)).await.unwrap();

        // is_permanent recognises Storage(msg).contains("schema") as permanent.
        let mut w = make_worker_with_fault(
            backend.clone(),
            Arc::new(AlwaysFailVector("schema drift detected".into())),
        );
        let r = w.step_at(far_future()).await.unwrap();
        assert!(matches!(
            r,
            StepResult::DeadLettered {
                reason: DeadLetterReason::Permanent,
                ..
            }
        ));

        let dls = backend.dead_letter().list_unresolved(100).await.unwrap();
        assert_eq!(dls.len(), 1);
        // Attempts-made is 1 (we tried once, classified permanent, dead-letter).
        assert_eq!(dls[0].attempts_made, 1);

        let events = backend
            .metadata()
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let dl_events: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::CascadeDeadLetter)
            .collect();
        assert_eq!(dl_events.len(), 1);
        assert!(dl_events[0]
            .details_json
            .contains("\"reason\":\"permanent\""));
    }

    // ------------------------------------------------------------------
    // step() FIFO per memory_id by sequence_id
    // (Phase A Q5 test 4: concurrent updates serialise by audit seq)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn step_serialises_concurrent_updates_to_same_memory_by_sequence_id() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "v1");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        // Two updates back-to-back. Each gets a fresh audit seq, so
        // sequence_ids are strictly increasing.
        let mut v2 = m.clone();
        v2.content = "v2".into();
        backend.update_memory(&v2, &embedding(0.2)).await.unwrap();
        let mut v3 = m.clone();
        v3.content = "v3".into();
        backend.update_memory(&v3, &embedding(0.3)).await.unwrap();

        // Three retry-queue entries.
        assert_eq!(backend.retry_queue().len().await.unwrap(), 3);

        // Step 3 times — each succeeds. The poll_due ordering by
        // sequence_id ASC is the structural enforcement; we just verify
        // each step drains the lowest-seq entry.
        let due_initial = backend
            .retry_queue()
            .poll_due(far_future(), 100)
            .await
            .unwrap();
        assert_eq!(due_initial.len(), 3);
        let initial_seqs: Vec<i64> = due_initial.iter().map(|e| e.sequence_id).collect();
        // poll_due returns in ASC sequence_id order.
        assert!(
            initial_seqs.windows(2).all(|w| w[0] < w[1]),
            "poll_due must return entries in sequence_id ASC order: got {initial_seqs:?}"
        );

        let mut w = make_worker(backend.clone());
        for _ in 0..3 {
            let r = w.step_at(far_future()).await.unwrap();
            assert!(matches!(r, StepResult::SucceededEntry { .. }));
        }
        assert_eq!(backend.retry_queue().len().await.unwrap(), 0);
    }

    // ------------------------------------------------------------------
    // Idempotent re-run after partial success
    // (a vector op succeeded once, graph failed; retry: vector no-ops, graph succeeds)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn step_idempotent_re_run_after_partial_success() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "partial-then-full");
        backend.write_memory(&m, &embedding(0.6)).await.unwrap();

        // First step with AlwaysFailGraph: vector.upsert succeeds, graph
        // fault-fails → reschedule.
        let mut w_fail = make_worker_with_fault(
            backend.clone(),
            Arc::new(AlwaysFailGraph("simulated duckdb io".into())),
        );
        let r = w_fail.step_at(far_future()).await.unwrap();
        assert!(
            matches!(r, StepResult::Rescheduled { .. }),
            "graph fail on attempt 1 should reschedule, got {r:?}"
        );
        // Entry still queued; vector store ALREADY has the row (idempotency invariant).
        let b = Boundary::new("work").unwrap();
        let hits = backend
            .vector_store()
            .search(&embedding(0.6), 10, std::slice::from_ref(&b))
            .await
            .unwrap();
        assert!(
            hits.iter().any(|(id, _)| id == &m.id),
            "vector upsert should have persisted on attempt 1"
        );

        // Now retry without graph fault: vector.upsert is a no-op
        // (merge_insert by id is idempotent — T0.1.4 invariant), graph
        // no-op succeeds → SucceededEntry.
        let mut w_clean = make_worker_with_fault(backend.clone(), Arc::new(NoFault));
        let r2 = w_clean.step_at(far_future()).await.unwrap();
        assert!(
            matches!(r2, StepResult::SucceededEntry { .. }),
            "second attempt should succeed cleanly, got {r2:?}"
        );
        assert_eq!(backend.retry_queue().len().await.unwrap(), 0);

        // Vector store STILL has exactly one row for this memory id.
        let hits = backend
            .vector_store()
            .search(&embedding(0.6), 10, &[b])
            .await
            .unwrap();
        let count_for_id = hits.iter().filter(|(id, _)| id == &m.id).count();
        assert_eq!(
            count_for_id, 1,
            "merge_insert by id must NOT duplicate on idempotent re-run"
        );
    }

    // ------------------------------------------------------------------
    // run() loop — graceful shutdown via watch::Sender
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn run_exits_on_cancel_signal() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "for run-loop");
        backend.write_memory(&m, &embedding(0.7)).await.unwrap();

        // Tiny poll interval so the loop completes a step + sleep + step
        // quickly. The first step at real "now" is too early (next_at is
        // ~1s in the future), so it'll be Idle. After ~1s, the step
        // succeeds. Then we cancel; loop exits.
        let (tx, rx) = tokio::sync::watch::channel(false);
        let worker = make_worker(backend.clone()).with_poll_interval(Duration::from_millis(50));

        let handle = tokio::spawn(async move { worker.run(rx).await });

        // Wait long enough for the cascade to drain (≥ 1.25s for backoff).
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Send cancel signal.
        tx.send(true).unwrap();

        // The loop should exit promptly (within one poll_interval).
        let join = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("run loop should exit within 5s of cancel");
        join.unwrap();

        // Cascade was drained.
        assert_eq!(backend.retry_queue().len().await.unwrap(), 0);
    }

    // ------------------------------------------------------------------
    // Sanity: writing through StorageBackend keeps queue depth bounded by
    // MAX_RETRY_QUEUE_DEPTH (cascading.rs already pins this — duplicated
    // here as a worker-side smoke test).
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn cap_constant_visible_to_worker_module() {
        // Compile-time link check that the `MAX_RETRY_QUEUE_DEPTH` re-export
        // is reachable from this module's tests (vault-cli uses the same
        // re-export path).
        assert_eq!(MAX_RETRY_QUEUE_DEPTH, 10_000);
    }
}
